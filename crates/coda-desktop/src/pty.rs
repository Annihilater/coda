use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::io::Write;
use std::os::fd::RawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Normalized agent / terminal lifecycle state aligned with Herdr OSC 7501 and board columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentLifecycleState {
    NeedsYou,
    Working,
    Done,
    Idle,
}

impl AgentLifecycleState {
    pub fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "needs_you" | "needsyou" | "needs-you" | "blocked" | "waiting" | "attention"
            | "prompt" | "permission" | "error" => Self::NeedsYou,
            "working" | "running" | "busy" | "active" | "thinking" | "streaming" => Self::Working,
            "done" | "completed" | "finished" | "success" | "ready" => Self::Done,
            _ => Self::Idle,
        }
    }

    pub fn color_u32(self) -> u32 {
        match self {
            Self::NeedsYou => 0xf59e0b,
            Self::Working => 0x38bdf8,
            Self::Done => 0x22c55e,
            Self::Idle => 0x525866,
        }
    }
}

/// Parsed OSC 7501 agent status emission from terminal stdout (`\x1b]7501;state=...;task=...\x07`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc7501Status {
    pub state: AgentLifecycleState,
    pub task: Option<String>,
    pub updated_at: Instant,
}

/// Single character cell in the VT screen buffer with SGR styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VtCell {
    pub ch: char,
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
}

impl Default for VtCell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: None,
            bg: None,
            bold: false,
        }
    }
}

/// Byte-range highlight span in a rendered line (guarantees valid UTF-8 char boundaries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalColorSpan {
    pub start: usize,
    pub end: usize,
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
}

/// Single rendered terminal line containing plain UTF-8 text and SGR spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledTerminalLine {
    pub text: String,
    pub spans: Vec<TerminalColorSpan>,
}

/// UI-facing snapshot of an active PTY session.
#[derive(Debug, Clone)]
pub struct PtySnapshot {
    pub lines: Vec<String>,
    pub styled_lines: Vec<StyledTerminalLine>,
    pub alive: bool,
    pub osc_status: Option<Osc7501Status>,
    pub started_at: Instant,
    pub attached_hc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEsc,
}

#[derive(Debug, Clone, Copy, Default)]
struct SgrStyle {
    fg: Option<u32>,
    bg: Option<u32>,
    bold: bool,
    inverse: bool,
}

impl SgrStyle {
    fn to_cell_colors(self) -> (Option<u32>, Option<u32>, bool) {
        if self.inverse {
            let fg = self.bg.or(Some(0x0d0e11));
            let bg = self.fg.or(Some(0xe6edf3));
            (fg, bg, self.bold)
        } else {
            (self.fg, self.bg, self.bold)
        }
    }
}

#[derive(Debug, Clone)]
struct SavedScreenState {
    grid: Vec<Vec<VtCell>>,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: (usize, usize),
    scroll_top: usize,
    scroll_bottom: usize,
    style: SgrStyle,
}

/// Compact VT100/ANSI screen emulator with 16/256/TrueColor SGR, alternate screen, and OSC 7501.
pub struct VtScreenBuffer {
    cols: usize,
    rows: usize,
    grid: Vec<Vec<VtCell>>,
    scrollback: VecDeque<Vec<VtCell>>,
    max_scrollback: usize,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: (usize, usize),
    scroll_top: usize,
    scroll_bottom: usize,
    style: SgrStyle,
    alt_saved: Option<SavedScreenState>,
    parse_state: ParseState,
    csi_buf: String,
    osc_buf: String,
    utf8_pending: Vec<u8>,
    osc_status: Option<Osc7501Status>,
}

impl VtScreenBuffer {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(20);
        let rows = rows.max(4);
        Self {
            cols,
            rows,
            grid: vec![vec![VtCell::default(); cols]; rows],
            scrollback: VecDeque::with_capacity(1200),
            max_scrollback: 1200,
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: (0, 0),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            style: SgrStyle::default(),
            alt_saved: None,
            parse_state: ParseState::Ground,
            csi_buf: String::new(),
            osc_buf: String::new(),
            utf8_pending: Vec::with_capacity(4),
            osc_status: None,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let new_cols = cols.max(20);
        let new_rows = rows.max(4);
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }
        let mut next = vec![vec![VtCell::default(); new_cols]; new_rows];
        let copy_rows = self.rows.min(new_rows);
        let copy_cols = self.cols.min(new_cols);
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                next[r][c] = self.grid[r][c];
            }
        }
        self.grid = next;
        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_row = self.cursor_row.min(new_rows - 1);
        self.cursor_col = self.cursor_col.min(new_cols - 1);
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
    }

    pub fn clear_all(&mut self) {
        for row in &mut self.grid {
            row.fill(VtCell::default());
        }
        self.scrollback.clear();
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    pub fn osc_status(&self) -> Option<Osc7501Status> {
        self.osc_status.clone()
    }

    pub fn feed_bytes(&mut self, data: &[u8]) {
        for &byte in data {
            self.feed_byte(byte);
        }
    }

    fn feed_byte(&mut self, byte: u8) {
        match self.parse_state {
            ParseState::Ground => {
                if !self.utf8_pending.is_empty() {
                    if (byte & 0b1100_0000) == 0b1000_0000 {
                        self.utf8_pending.push(byte);
                        if let Ok(s) = std::str::from_utf8(&self.utf8_pending) {
                            let chars: Vec<char> = s.chars().collect();
                            self.utf8_pending.clear();
                            for ch in chars {
                                self.put_char(ch);
                            }
                        } else if self.utf8_pending.len() >= 4 {
                            self.utf8_pending.clear();
                        }
                        return;
                    } else {
                        self.utf8_pending.clear();
                    }
                }
                match byte {
                    0x1b => self.parse_state = ParseState::Escape,
                    b'\r' => self.cursor_col = 0,
                    b'\n' => self.line_feed(),
                    0x08 => {
                        if self.cursor_col > 0 {
                            self.cursor_col -= 1;
                        }
                    }
                    b'\t' => {
                        let next_tab = ((self.cursor_col / 8) + 1) * 8;
                        self.cursor_col = next_tab.min(self.cols - 1);
                    }
                    0x07 | 0x00..=0x06 | 0x0b..=0x1a | 0x1c..=0x1f | 0x7f => {}
                    0x20..=0x7e => self.put_char(byte as char),
                    0x80..=0xbf => {}
                    0xc0..=0xff => self.utf8_pending.push(byte),
                }
            }
            ParseState::Escape => match byte {
                b'[' => {
                    self.csi_buf.clear();
                    self.parse_state = ParseState::Csi;
                }
                b']' => {
                    self.osc_buf.clear();
                    self.parse_state = ParseState::Osc;
                }
                b'M' => {
                    self.reverse_index();
                    self.parse_state = ParseState::Ground;
                }
                b'7' => {
                    self.saved_cursor = (self.cursor_row, self.cursor_col);
                    self.parse_state = ParseState::Ground;
                }
                b'8' => {
                    self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                    self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
                    self.parse_state = ParseState::Ground;
                }
                b'c' => {
                    self.clear_all();
                    self.style = SgrStyle::default();
                    self.parse_state = ParseState::Ground;
                }
                b'(' | b')' | b'*' | b'+' => {
                    self.parse_state = ParseState::Ground;
                }
                _ => self.parse_state = ParseState::Ground,
            },
            ParseState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    let seq = std::mem::take(&mut self.csi_buf);
                    self.dispatch_csi(&seq, byte as char);
                    self.parse_state = ParseState::Ground;
                } else {
                    self.csi_buf.push(byte as char);
                }
            }
            ParseState::Osc => match byte {
                0x07 => {
                    let seq = std::mem::take(&mut self.osc_buf);
                    self.dispatch_osc(&seq);
                    self.parse_state = ParseState::Ground;
                }
                0x1b => self.parse_state = ParseState::OscEsc,
                _ => self.osc_buf.push(byte as char),
            },
            ParseState::OscEsc => {
                if byte == b'\\' {
                    let seq = std::mem::take(&mut self.osc_buf);
                    self.dispatch_osc(&seq);
                }
                self.parse_state = ParseState::Ground;
            }
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.line_feed();
        }
        let (fg, bg, bold) = self.style.to_cell_colors();
        self.grid[self.cursor_row][self.cursor_col] = VtCell { ch, fg, bg, bold };
        self.cursor_col += 1;
    }

    fn line_feed(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up_region(self.scroll_top, self.scroll_bottom, 1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down_region(self.scroll_top, self.scroll_bottom, 1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
    }

    fn scroll_up_region(&mut self, top: usize, bottom: usize, count: usize) {
        let top = top.min(self.rows.saturating_sub(1));
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return;
        }
        for _ in 0..count {
            let removed = self.grid.remove(top);
            if top == 0 && self.alt_saved.is_none() {
                if self.scrollback.len() >= self.max_scrollback {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(removed);
            }
            self.grid.insert(bottom, vec![VtCell::default(); self.cols]);
        }
    }

    fn scroll_down_region(&mut self, top: usize, bottom: usize, count: usize) {
        let top = top.min(self.rows.saturating_sub(1));
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return;
        }
        for _ in 0..count {
            self.grid.remove(bottom);
            self.grid.insert(top, vec![VtCell::default(); self.cols]);
        }
    }

    fn dispatch_osc(&mut self, seq: &str) {
        // Format: 7501;state=working;task=Running cargo test
        let Some(rest) = seq.strip_prefix("7501;") else {
            return;
        };
        let mut state: Option<AgentLifecycleState> = None;
        let mut task: Option<String> = None;
        for part in rest.split(';') {
            if let Some(val) = part.strip_prefix("state=") {
                state = Some(AgentLifecycleState::from_str_lossy(val));
            } else if let Some(val) = part.strip_prefix("task=") {
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    task = Some(trimmed.to_string());
                }
            }
        }
        if let Some(state) = state {
            self.osc_status = Some(Osc7501Status {
                state,
                task,
                updated_at: Instant::now(),
            });
        }
    }

    fn enter_alt_screen(&mut self) {
        if self.alt_saved.is_some() {
            return;
        }
        let blank = vec![vec![VtCell::default(); self.cols]; self.rows];
        let prev_grid = std::mem::replace(&mut self.grid, blank);
        self.alt_saved = Some(SavedScreenState {
            grid: prev_grid,
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
            saved_cursor: self.saved_cursor,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
            style: self.style,
        });
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
    }

    fn leave_alt_screen(&mut self) {
        if let Some(saved) = self.alt_saved.take() {
            self.grid = saved.grid;
            self.cursor_row = saved.cursor_row.min(self.rows - 1);
            self.cursor_col = saved.cursor_col.min(self.cols - 1);
            self.saved_cursor = saved.saved_cursor;
            self.scroll_top = saved.scroll_top.min(self.rows - 1);
            self.scroll_bottom = saved.scroll_bottom.min(self.rows - 1);
            self.style = saved.style;
        }
    }

    fn dispatch_csi(&mut self, params_raw: &str, cmd: char) {
        if let Some(priv_params) = params_raw.strip_prefix('?') {
            let codes: Vec<usize> = priv_params
                .split(';')
                .filter_map(|s| s.parse::<usize>().ok())
                .collect();
            match cmd {
                'h' => {
                    if codes.iter().any(|c| matches!(c, 1049 | 1047 | 47)) {
                        self.enter_alt_screen();
                    }
                }
                'l' => {
                    if codes.iter().any(|c| matches!(c, 1049 | 1047 | 47)) {
                        self.leave_alt_screen();
                    }
                }
                _ => {}
            }
            return;
        }

        let clean = params_raw.trim_start_matches('>');
        let params: Vec<usize> = if clean.is_empty() {
            Vec::new()
        } else {
            clean
                .split(';')
                .map(|s| s.parse::<usize>().unwrap_or(0))
                .collect()
        };
        let p0 = params.first().copied().unwrap_or(0);
        let p0_one = p0.max(1);

        match cmd {
            'm' => self.apply_sgr(&params),
            'A' => self.cursor_row = self.cursor_row.saturating_sub(p0_one),
            'B' => self.cursor_row = (self.cursor_row + p0_one).min(self.rows - 1),
            'C' => self.cursor_col = (self.cursor_col + p0_one).min(self.cols - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(p0_one),
            'G' => self.cursor_col = p0_one.saturating_sub(1).min(self.cols - 1),
            'd' => self.cursor_row = p0_one.saturating_sub(1).min(self.rows - 1),
            'H' | 'f' => {
                let row = params.first().copied().unwrap_or(1).max(1) - 1;
                let col = params.get(1).copied().unwrap_or(1).max(1) - 1;
                self.cursor_row = row.min(self.rows - 1);
                self.cursor_col = col.min(self.cols - 1);
            }
            'J' => {
                let row = self.cursor_row;
                let col = self.cursor_col;
                match p0 {
                    0 => {
                        for c in col..self.cols {
                            self.grid[row][c] = VtCell::default();
                        }
                        for r in (row + 1)..self.rows {
                            self.grid[r].fill(VtCell::default());
                        }
                    }
                    1 => {
                        for r in 0..row {
                            self.grid[r].fill(VtCell::default());
                        }
                        for c in 0..=col.min(self.cols - 1) {
                            self.grid[row][c] = VtCell::default();
                        }
                    }
                    2 | 3 => {
                        for r in 0..self.rows {
                            self.grid[r].fill(VtCell::default());
                        }
                        if p0 == 3 {
                            self.scrollback.clear();
                        }
                    }
                    _ => {}
                }
            }
            'K' => {
                let row = self.cursor_row;
                let col = self.cursor_col;
                match p0 {
                    0 => {
                        for c in col..self.cols {
                            self.grid[row][c] = VtCell::default();
                        }
                    }
                    1 => {
                        for c in 0..=col.min(self.cols - 1) {
                            self.grid[row][c] = VtCell::default();
                        }
                    }
                    2 => self.grid[row].fill(VtCell::default()),
                    _ => {}
                }
            }
            'L' => {
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    self.scroll_down_region(self.cursor_row, self.scroll_bottom, p0_one);
                }
            }
            'M' => {
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    self.scroll_up_region(self.cursor_row, self.scroll_bottom, p0_one);
                }
            }
            'S' => self.scroll_up_region(self.scroll_top, self.scroll_bottom, p0_one),
            'T' => self.scroll_down_region(self.scroll_top, self.scroll_bottom, p0_one),
            'P' => {
                let row = self.cursor_row;
                let col = self.cursor_col;
                let count = p0_one.min(self.cols.saturating_sub(col));
                for _ in 0..count {
                    self.grid[row].remove(col);
                    self.grid[row].push(VtCell::default());
                }
            }
            '@' => {
                let row = self.cursor_row;
                let col = self.cursor_col;
                let count = p0_one.min(self.cols.saturating_sub(col));
                for _ in 0..count {
                    self.grid[row].pop();
                    self.grid[row].insert(col, VtCell::default());
                }
            }
            'X' => {
                let row = self.cursor_row;
                let col = self.cursor_col;
                for c in col..(col + p0_one).min(self.cols) {
                    self.grid[row][c] = VtCell::default();
                }
            }
            'r' => {
                let top = params.first().copied().unwrap_or(1).max(1) - 1;
                let bottom = params
                    .get(1)
                    .copied()
                    .filter(|&b| b > 0)
                    .unwrap_or(self.rows)
                    .min(self.rows)
                    - 1;
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = self.rows - 1;
                }
                self.cursor_row = self.scroll_top;
                self.cursor_col = 0;
            }
            's' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            'u' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
            }
            _ => {}
        }
    }

    fn apply_sgr(&mut self, params: &[usize]) {
        if params.is_empty() {
            self.style = SgrStyle::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.style = SgrStyle::default(),
                1 => self.style.bold = true,
                7 => self.style.inverse = true,
                22 => self.style.bold = false,
                27 => self.style.inverse = false,
                30..=37 => self.style.fg = Some(ansi16_color(params[i] - 30, false)),
                38 => {
                    if let Some((color, consumed)) = parse_extended_color(&params[i + 1..]) {
                        self.style.fg = Some(color);
                        i += consumed;
                    }
                }
                39 => self.style.fg = None,
                40..=47 => self.style.bg = Some(ansi16_color(params[i] - 40, false)),
                48 => {
                    if let Some((color, consumed)) = parse_extended_color(&params[i + 1..]) {
                        self.style.bg = Some(color);
                        i += consumed;
                    }
                }
                49 => self.style.bg = None,
                90..=97 => self.style.fg = Some(ansi16_color(params[i] - 90, true)),
                100..=107 => self.style.bg = Some(ansi16_color(params[i] - 100, true)),
                _ => {}
            }
            i += 1;
        }
    }

    pub fn render_lines(&self, max_lines: usize) -> Vec<String> {
        self.render_styled_lines(max_lines, false)
            .into_iter()
            .map(|l| l.text)
            .collect()
    }

    pub fn render_styled_lines(&self, max_lines: usize, show_cursor: bool) -> Vec<StyledTerminalLine> {
        let mut raw_rows: Vec<(Vec<VtCell>, Option<usize>)> = Vec::new();
        if self.alt_saved.is_none() {
            for sb_row in &self.scrollback {
                raw_rows.push((sb_row.clone(), None));
            }
        }
        for (r_idx, row) in self.grid.iter().enumerate() {
            let cursor_col = if show_cursor && r_idx == self.cursor_row {
                Some(self.cursor_col.min(self.cols - 1))
            } else {
                None
            };
            raw_rows.push((row.clone(), cursor_col));
        }

        while raw_rows.len() > 1 {
            let Some((last_cells, cursor_col)) = raw_rows.last() else {
                break;
            };
            if cursor_col.is_some() || last_cells.iter().any(|c| c.ch != ' ' || c.bg.is_some()) {
                break;
            }
            raw_rows.pop();
        }

        let start = raw_rows.len().saturating_sub(max_lines);
        raw_rows[start..]
            .iter()
            .map(|(cells, cursor_col)| cells_to_styled_line(cells, *cursor_col))
            .collect()
    }
}

fn cells_to_styled_line(cells: &[VtCell], cursor_col: Option<usize>) -> StyledTerminalLine {
    let mut last_non_space = cells
        .iter()
        .rposition(|c| c.ch != ' ' || c.bg.is_some())
        .map(|idx| idx + 1)
        .unwrap_or(0);
    if let Some(c_col) = cursor_col {
        last_non_space = last_non_space.max(c_col + 1).min(cells.len());
    }

    let mut text = String::new();
    let mut spans: Vec<TerminalColorSpan> = Vec::new();

    for (col_idx, cell) in cells[..last_non_space].iter().enumerate() {
        let is_cursor = cursor_col == Some(col_idx);
        let (fg, bg, bold) = if is_cursor {
            (Some(0x0d0e11), Some(0xe6edf3), cell.bold)
        } else {
            (cell.fg, cell.bg, cell.bold)
        };

        let byte_start = text.len();
        text.push(cell.ch);
        let byte_end = text.len();

        if fg.is_some() || bg.is_some() || bold {
            if let Some(prev) = spans.last_mut() {
                if prev.end == byte_start && prev.fg == fg && prev.bg == bg && prev.bold == bold {
                    prev.end = byte_end;
                    continue;
                }
            }
            spans.push(TerminalColorSpan {
                start: byte_start,
                end: byte_end,
                fg,
                bg,
                bold,
            });
        }
    }

    StyledTerminalLine { text, spans }
}

fn parse_extended_color(rest: &[usize]) -> Option<(u32, usize)> {
    match rest.first().copied()? {
        5 => {
            let idx = rest.get(1).copied()?;
            Some((ansi256_color(idx), 2))
        }
        2 => {
            let r = rest.get(1).copied()?.min(255) as u32;
            let g = rest.get(2).copied()?.min(255) as u32;
            let b = rest.get(3).copied()?.min(255) as u32;
            Some(((r << 16) | (g << 8) | b, 4))
        }
        _ => None,
    }
}

fn ansi16_color(idx: usize, bright: bool) -> u32 {
    const NORMAL: [u32; 8] = [
        0x1b1e24, 0xf87171, 0x22c55e, 0xf59e0b, 0x38bdf8, 0xc084fc, 0x2dd4bf, 0xc9d1d9,
    ];
    const BRIGHT: [u32; 8] = [
        0x6e7681, 0xfca5a5, 0x4ade80, 0xfbbf24, 0x7dd3fc, 0xd8b4fe, 0x5eead4, 0xf0f6fc,
    ];
    let i = idx.min(7);
    if bright {
        BRIGHT[i]
    } else {
        NORMAL[i]
    }
}

fn ansi256_color(idx: usize) -> u32 {
    if idx < 8 {
        return ansi16_color(idx, false);
    }
    if idx < 16 {
        return ansi16_color(idx - 8, true);
    }
    if idx < 232 {
        let n = idx - 16;
        let r_i = n / 36;
        let g_i = (n % 36) / 6;
        let b_i = n % 6;
        let chan = |v: usize| -> u32 { if v == 0 { 0 } else { (55 + v * 40) as u32 } };
        return (chan(r_i) << 16) | (chan(g_i) << 8) | chan(b_i);
    }
    let gray = (8 + (idx.saturating_sub(232)) * 10).min(255) as u32;
    (gray << 16) | (gray << 8) | gray
}

struct PtySessionHandle {
    master_fd: RawFd,
    child_pid: libc::pid_t,
    cols: u16,
    rows: u16,
    alive: Arc<AtomicBool>,
    detach_on_drop: bool,
    attached_hc: bool,
    started_at: Instant,
    screen: Arc<Mutex<VtScreenBuffer>>,
}

impl Drop for PtySessionHandle {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        if self.master_fd >= 0 {
            if self.detach_on_drop {
                let detach_key = [0x1c_u8];
                unsafe {
                    libc::write(self.master_fd, detach_key.as_ptr().cast(), 1);
                }
            }
            unsafe {
                libc::close(self.master_fd);
            }
            self.master_fd = -1;
        }
        if !self.detach_on_drop && self.child_pid > 0 {
            unsafe {
                libc::kill(self.child_pid, libc::SIGHUP);
            }
        }
    }
}

struct RemoteVtHandle {
    cols: u16,
    rows: u16,
    alive: Arc<AtomicBool>,
    started_at: Instant,
    screen: Arc<Mutex<VtScreenBuffer>>,
}

/// Manages multiple local Unix `openpty` and streamed remote `herdr` terminal sessions wired into GPUI.
pub struct LocalPtyManager {
    sessions: HashMap<String, PtySessionHandle>,
    remote_buffers: HashMap<String, RemoteVtHandle>,
    ui_dirty: Arc<AtomicBool>,
}

impl LocalPtyManager {
    pub fn new(ui_dirty: Arc<AtomicBool>) -> Self {
        Self {
            sessions: HashMap::new(),
            remote_buffers: HashMap::new(),
            ui_dirty,
        }
    }

    pub fn has_session(&self, id: &str) -> bool {
        self.sessions.contains_key(id) || self.remote_buffers.contains_key(id)
    }

    pub fn ensure_remote_buffer(&mut self, id: &str, cols: u16, rows: u16) {
        if self.sessions.contains_key(id) || self.remote_buffers.contains_key(id) {
            return;
        }
        let cols = cols.max(40);
        let rows = rows.max(12);
        self.remote_buffers.insert(
            id.to_string(),
            RemoteVtHandle {
                cols,
                rows,
                alive: Arc::new(AtomicBool::new(true)),
                started_at: Instant::now(),
                screen: Arc::new(Mutex::new(VtScreenBuffer::new(
                    cols as usize,
                    rows as usize,
                ))),
            },
        );
        self.ui_dirty.store(true, Ordering::SeqCst);
    }

    pub fn feed_remote_output(&self, id: &str, bytes: &[u8]) {
        if let Some(handle) = self.remote_buffers.get(id) {
            if let Ok(mut guard) = handle.screen.lock() {
                guard.feed_bytes(bytes);
            }
            self.ui_dirty.store(true, Ordering::SeqCst);
        }
    }

    pub fn set_remote_alive(&self, id: &str, alive: bool) {
        if let Some(handle) = self.remote_buffers.get(id) {
            handle.alive.store(alive, Ordering::SeqCst);
            self.ui_dirty.store(true, Ordering::SeqCst);
        }
    }

    pub fn remove_remote_buffer(&mut self, id: &str) {
        if self.remote_buffers.remove(id).is_some() {
            self.ui_dirty.store(true, Ordering::SeqCst);
        }
    }

    /// Spawns a local interactive shell inside a real PTY using `libc::openpty`.
    pub fn spawn_session(
        &mut self,
        id: &str,
        cwd: &Path,
        command: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut argv = vec![shell.clone()];
        if let Some(cmd) = command.filter(|c| !c.trim().is_empty()) {
            argv.push("-lc".to_string());
            argv.push(cmd.to_string());
        } else {
            argv.push("-l".to_string());
        }
        self.spawn_pty_program(id, cwd, &shell, &argv, cols, rows, false)
    }

    /// Spawns `hc attach <addr>` inside a real PTY with graceful detach (`0x1c`) on close/drop.
    pub fn spawn_herdr_attach(
        &mut self,
        id: &str,
        cwd: &Path,
        hc_bin: &Path,
        hc_addr: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let program = hc_bin.to_string_lossy().to_string();
        let argv = vec![program.clone(), "attach".to_string(), hc_addr.to_string()];
        self.spawn_pty_program(id, cwd, &program, &argv, cols, rows, true)
    }

    fn spawn_pty_program(
        &mut self,
        id: &str,
        cwd: &Path,
        program: &str,
        argv: &[String],
        cols: u16,
        rows: u16,
        detach_on_drop: bool,
    ) -> Result<(), String> {
        if self.sessions.contains_key(id) {
            return Ok(());
        }

        let cols = cols.max(40);
        let rows = rows.max(12);
        let mut master_fd: RawFd = -1;
        let mut slave_fd: RawFd = -1;
        let mut win = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let open_rc = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut win,
            )
        };
        if open_rc != 0 {
            return Err(format!("openpty failed: {}", std::io::Error::last_os_error()));
        }

        let prog_c = CString::new(program).map_err(|e| e.to_string())?;
        let cwd_c = CString::new(cwd.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let argv_c: Vec<CString> = argv
            .iter()
            .map(|a| CString::new(a.as_str()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut argv_ptrs: Vec<*const libc::c_char> =
            argv_c.iter().map(|s| s.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        let term_env = CString::new("TERM=xterm-256color").unwrap();
        let colorterm_env = CString::new("COLORTERM=truecolor").unwrap();

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            unsafe {
                libc::close(master_fd);
                libc::close(slave_fd);
            }
            return Err(format!("fork failed: {}", std::io::Error::last_os_error()));
        }

        if pid == 0 {
            unsafe {
                libc::close(master_fd);
                libc::setsid();
                libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0);
                libc::dup2(slave_fd, libc::STDIN_FILENO);
                libc::dup2(slave_fd, libc::STDOUT_FILENO);
                libc::dup2(slave_fd, libc::STDERR_FILENO);
                if slave_fd > 2 {
                    libc::close(slave_fd);
                }
                libc::chdir(cwd_c.as_ptr());
                libc::putenv(term_env.as_ptr() as *mut libc::c_char);
                libc::putenv(colorterm_env.as_ptr() as *mut libc::c_char);
                libc::execvp(prog_c.as_ptr(), argv_ptrs.as_ptr());
                libc::_exit(127);
            }
        }

        unsafe {
            libc::close(slave_fd);
        }

        let reader_fd = unsafe { libc::dup(master_fd) };
        if reader_fd < 0 {
            unsafe {
                libc::close(master_fd);
            }
            return Err("dup(master_fd) failed".to_string());
        }

        let alive = Arc::new(AtomicBool::new(true));
        let screen = Arc::new(Mutex::new(VtScreenBuffer::new(cols as usize, rows as usize)));

        let alive_bg = Arc::clone(&alive);
        let screen_bg = Arc::clone(&screen);
        let ui_dirty = Arc::clone(&self.ui_dirty);

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let n = unsafe { libc::read(reader_fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    break;
                }
                if let Ok(mut guard) = screen_bg.lock() {
                    guard.feed_bytes(&buf[..n as usize]);
                }
                ui_dirty.store(true, Ordering::SeqCst);
            }
            unsafe {
                libc::close(reader_fd);
            }
            let mut status: libc::c_int = 0;
            unsafe {
                libc::waitpid(pid, &mut status, libc::WNOHANG);
            }
            alive_bg.store(false, Ordering::SeqCst);
            ui_dirty.store(true, Ordering::SeqCst);
        });

        self.sessions.insert(
            id.to_string(),
            PtySessionHandle {
                master_fd,
                child_pid: pid,
                cols,
                rows,
                alive,
                detach_on_drop,
                attached_hc: detach_on_drop,
                started_at: Instant::now(),
                screen,
            },
        );
        self.ui_dirty.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn write_input(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        let Some(session) = self.sessions.get(id) else {
            return Err(format!("PTY session '{id}' not found"));
        };
        let mut written = 0usize;
        while written < bytes.len() {
            let rc = unsafe {
                libc::write(
                    session.master_fd,
                    bytes[written..].as_ptr().cast(),
                    bytes.len() - written,
                )
            };
            if rc <= 0 {
                return Err(format!("pty write failed: {}", std::io::Error::last_os_error()));
            }
            written += rc as usize;
        }
        Ok(())
    }

    pub fn send_command_line(&self, id: &str, command: &str) -> Result<(), String> {
        let mut payload = command.as_bytes().to_vec();
        if !payload.ends_with(b"\n") && !payload.ends_with(b"\r") {
            payload.push(b'\r');
        }
        self.write_input(id, &payload)
    }

    pub fn send_sigint(&self, id: &str) -> Result<(), String> {
        self.write_input(id, &[0x03])
    }

    pub fn clear_buffer(&self, id: &str) {
        if let Some(session) = self.sessions.get(id) {
            if let Ok(mut guard) = session.screen.lock() {
                guard.clear_all();
            }
            self.ui_dirty.store(true, Ordering::SeqCst);
        } else if let Some(remote) = self.remote_buffers.get(id) {
            if let Ok(mut guard) = remote.screen.lock() {
                guard.clear_all();
            }
            self.ui_dirty.store(true, Ordering::SeqCst);
        }
    }

    pub fn resize(&mut self, id: &str, cols: u16, rows: u16) {
        let cols = cols.max(40);
        let rows = rows.max(12);
        if let Some(session) = self.sessions.get_mut(id) {
            if session.cols == cols && session.rows == rows {
                return;
            }
            session.cols = cols;
            session.rows = rows;
            let win = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            unsafe {
                libc::ioctl(session.master_fd, libc::TIOCSWINSZ as _, &win);
            }
            if let Ok(mut guard) = session.screen.lock() {
                guard.resize(cols as usize, rows as usize);
            }
            return;
        }
        if let Some(remote) = self.remote_buffers.get_mut(id) {
            if remote.cols == cols && remote.rows == rows {
                return;
            }
            remote.cols = cols;
            remote.rows = rows;
            if let Ok(mut guard) = remote.screen.lock() {
                guard.resize(cols as usize, rows as usize);
            }
        }
    }

    pub fn snapshot_all(&self, max_lines: usize) -> HashMap<String, PtySnapshot> {
        let mut out = HashMap::new();
        for (id, session) in &self.sessions {
            let alive = session.alive.load(Ordering::SeqCst);
            out.insert(
                id.clone(),
                build_pty_snapshot(
                    &session.screen,
                    alive,
                    session.started_at,
                    session.attached_hc,
                    max_lines,
                ),
            );
        }
        for (id, remote) in &self.remote_buffers {
            let alive = remote.alive.load(Ordering::SeqCst);
            out.insert(
                id.clone(),
                build_pty_snapshot(
                    &remote.screen,
                    alive,
                    remote.started_at,
                    true,
                    max_lines,
                ),
            );
        }
        out
    }

    pub fn kill_session(&mut self, id: &str) {
        if let Some(mut handle) = self.sessions.remove(id) {
            handle.detach_on_drop = false;
            drop(handle);
        }
        self.remote_buffers.remove(id);
        let _ = std::io::stdout().flush();
        self.ui_dirty.store(true, Ordering::SeqCst);
    }
}

fn build_pty_snapshot(
    screen: &Arc<Mutex<VtScreenBuffer>>,
    alive: bool,
    started_at: Instant,
    attached_hc: bool,
    max_lines: usize,
) -> PtySnapshot {
    let (styled_lines, osc_status) = screen
        .lock()
        .map(|g| (g.render_styled_lines(max_lines, alive), g.osc_status()))
        .unwrap_or_default();
    let lines = styled_lines.iter().map(|l| l.text.clone()).collect();
    PtySnapshot {
        lines,
        styled_lines,
        alive,
        osc_status,
        started_at,
        attached_hc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_vt_buffer_snapshot_and_osc7501() {
        let dirty = Arc::new(AtomicBool::new(false));
        let mut mgr = LocalPtyManager::new(Arc::clone(&dirty));
        let id = "remote:dev1:t1";
        mgr.ensure_remote_buffer(id, 100, 30);
        assert!(mgr.has_session(id));
        mgr.feed_remote_output(
            id,
            b"\x1b[38;2;56;189;248mlinux@openeuler:~$\x1b[0m\x1b]7501;state=working;task=Building crates\x07",
        );
        assert!(dirty.swap(false, Ordering::SeqCst));

        let snaps = mgr.snapshot_all(40);
        let snap = snaps.get(id).expect("remote snapshot missing");
        assert!(snap.attached_hc);
        assert!(snap.alive);
        assert!(snap.lines.iter().any(|l| l.contains("linux@openeuler:~$")));
        let osc = snap.osc_status.as_ref().expect("expected OSC 7501");
        assert_eq!(osc.state, AgentLifecycleState::Working);
        assert_eq!(osc.task.as_deref(), Some("Building crates"));

        mgr.set_remote_alive(id, false);
        assert!(!mgr.snapshot_all(40)[id].alive);
        mgr.remove_remote_buffer(id);
        assert!(!mgr.has_session(id));
    }
}
