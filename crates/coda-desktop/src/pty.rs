use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::io::Write;
use std::os::fd::RawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_CSI_BUF_BYTES: usize = 256;
const MAX_OSC_BUF_BYTES: usize = 4096;

/// Display cell width (`0` combining/control, `2` CJK/Fullwidth/Emoji, `1` narrow).
pub fn char_display_width(ch: char) -> usize {
    let cp = ch as u32;
    if ch.is_control()
        || matches!(
            cp,
            0x0300..=0x036F
                | 0x1AB0..=0x1AFF
                | 0x1DC0..=0x1DFF
                | 0x200B..=0x200F
                | 0x202A..=0x202E
                | 0x2060..=0x206F
                | 0x20D0..=0x20FF
                | 0xFE00..=0xFE0F
                | 0xFE20..=0xFE2F
                | 0xFEFF
                | 0xE0100..=0xE01EF
        )
    {
        return 0;
    }
    if matches!(
        cp,
        0x1100..=0x115F
            | 0x2329..=0x232A
            | 0x2E80..=0x303E
            | 0x3040..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF01..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFF
    ) {
        return 2;
    }
    1
}

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
    /// True for the trailing placeholder cell of a width-2 CJK/Emoji glyph.
    pub continuation: bool,
}

impl Default for VtCell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: None,
            bg: None,
            bold: false,
            continuation: false,
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
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Ground,
    Escape,
    EscCharset,
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
    osc_buf: Vec<u8>,
    osc_overflow: bool,
    utf8_pending: Vec<u8>,
    osc_status: Option<Osc7501Status>,
    generation: u64,
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
            osc_buf: Vec::new(),
            osc_overflow: false,
            utf8_pending: Vec::with_capacity(4),
            osc_status: None,
            generation: 1,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
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
            // Drop dangling trailing wide half if resize cut between wide char & continuation.
            if copy_cols > 0 {
                if next[r][copy_cols - 1].continuation {
                    next[r][copy_cols - 1] = VtCell::default();
                } else if char_display_width(next[r][copy_cols - 1].ch) == 2 {
                    next[r][copy_cols - 1] = VtCell::default();
                }
            }
        }
        self.grid = next;
        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_row = self.cursor_row.min(new_rows - 1);
        self.cursor_col = self.cursor_col.min(new_cols - 1);
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn clear_all(&mut self) {
        for row in &mut self.grid {
            row.fill(VtCell::default());
        }
        self.scrollback.clear();
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn osc_status(&self) -> Option<Osc7501Status> {
        self.osc_status.clone()
    }

    pub fn feed_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        for &byte in data {
            self.feed_byte(byte);
        }
        self.generation = self.generation.wrapping_add(1);
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
                    self.osc_overflow = false;
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
                b'(' | b')' | b'*' | b'+' | b'#' => {
                    self.parse_state = ParseState::EscCharset;
                }
                _ => self.parse_state = ParseState::Ground,
            },
            ParseState::EscCharset => {
                // Consume the charset/line-drawing designator byte (e.g., 'B', '0', '8').
                self.parse_state = ParseState::Ground;
            }
            ParseState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    let seq = std::mem::take(&mut self.csi_buf);
                    self.dispatch_csi(&seq, byte as char);
                    self.parse_state = ParseState::Ground;
                } else if byte == 0x1b {
                    self.csi_buf.clear();
                    self.parse_state = ParseState::Escape;
                } else if self.csi_buf.len() < MAX_CSI_BUF_BYTES {
                    self.csi_buf.push(byte as char);
                } else {
                    self.csi_buf.clear();
                    self.parse_state = ParseState::Ground;
                }
            }
            ParseState::Osc => match byte {
                0x07 => {
                    let raw = std::mem::take(&mut self.osc_buf);
                    if !self.osc_overflow {
                        let seq = String::from_utf8_lossy(&raw).into_owned();
                        self.dispatch_osc(&seq);
                    }
                    self.osc_overflow = false;
                    self.parse_state = ParseState::Ground;
                }
                0x1b => self.parse_state = ParseState::OscEsc,
                _ => {
                    if !self.osc_overflow && self.osc_buf.len() < MAX_OSC_BUF_BYTES {
                        self.osc_buf.push(byte);
                    } else {
                        self.osc_overflow = true;
                        self.osc_buf.clear();
                    }
                }
            },
            ParseState::OscEsc => {
                if byte == b'\\' {
                    let raw = std::mem::take(&mut self.osc_buf);
                    if !self.osc_overflow {
                        let seq = String::from_utf8_lossy(&raw).into_owned();
                        self.dispatch_osc(&seq);
                    }
                    self.osc_overflow = false;
                }
                self.parse_state = ParseState::Ground;
            }
        }
    }

    fn clear_cell_preserving_wide(&mut self, row: usize, col: usize) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        if self.grid[row][col].continuation && col > 0 {
            self.grid[row][col - 1] = VtCell::default();
        } else if col + 1 < self.cols && self.grid[row][col + 1].continuation {
            self.grid[row][col + 1] = VtCell::default();
        }
        self.grid[row][col] = VtCell::default();
    }

    fn put_char(&mut self, ch: char) {
        let width = char_display_width(ch);
        if width == 0 {
            return;
        }
        let (fg, bg, bold) = self.style.to_cell_colors();
        if width == 2 && self.cols >= 2 {
            if self.cursor_col + 1 >= self.cols {
                if self.cursor_col < self.cols {
                    self.clear_cell_preserving_wide(self.cursor_row, self.cursor_col);
                }
                self.cursor_col = 0;
                self.line_feed();
            }
            self.clear_cell_preserving_wide(self.cursor_row, self.cursor_col);
            self.clear_cell_preserving_wide(self.cursor_row, self.cursor_col + 1);
            self.grid[self.cursor_row][self.cursor_col] = VtCell {
                ch,
                fg,
                bg,
                bold,
                continuation: false,
            };
            self.grid[self.cursor_row][self.cursor_col + 1] = VtCell {
                ch: ' ',
                fg,
                bg,
                bold,
                continuation: true,
            };
            self.cursor_col += 2;
        } else {
            if self.cursor_col >= self.cols {
                self.cursor_col = 0;
                self.line_feed();
            }
            self.clear_cell_preserving_wide(self.cursor_row, self.cursor_col);
            self.grid[self.cursor_row][self.cursor_col] = VtCell {
                ch,
                fg,
                bg,
                bold,
                continuation: false,
            };
            self.cursor_col += 1;
        }
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
        // Format: 7501;state=working;task=Running cargo test (also accepts title= / msg=)
        let Some(rest) = seq.strip_prefix("7501;") else {
            return;
        };
        let mut state: Option<AgentLifecycleState> = None;
        let mut task: Option<String> = None;
        for part in rest.split(';') {
            if let Some(val) = part.strip_prefix("state=") {
                state = Some(AgentLifecycleState::from_str_lossy(val));
            } else if let Some(val) = part
                .strip_prefix("task=")
                .or_else(|| part.strip_prefix("title="))
                .or_else(|| part.strip_prefix("msg="))
            {
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
                            self.clear_cell_preserving_wide(row, c);
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
                            self.clear_cell_preserving_wide(row, c);
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
                            self.clear_cell_preserving_wide(row, c);
                        }
                    }
                    1 => {
                        for c in 0..=col.min(self.cols - 1) {
                            self.clear_cell_preserving_wide(row, c);
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
                    self.clear_cell_preserving_wide(row, c);
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
            if cursor_col.is_some()
                || last_cells
                    .iter()
                    .any(|c| !c.continuation && (c.ch != ' ' || c.bg.is_some()))
            {
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
        .rposition(|c| !c.continuation && (c.ch != ' ' || c.bg.is_some()))
        .map(|idx| {
            if idx + 1 < cells.len() && cells[idx + 1].continuation {
                idx + 2
            } else {
                idx + 1
            }
        })
        .unwrap_or(0);
    if let Some(c_col) = cursor_col {
        last_non_space = last_non_space.max(c_col + 1).min(cells.len());
    }

    let mut text = String::new();
    let mut spans: Vec<TerminalColorSpan> = Vec::new();

    for (col_idx, cell) in cells[..last_non_space].iter().enumerate() {
        if cell.continuation {
            continue;
        }
        let is_cursor = cursor_col == Some(col_idx)
            || (cursor_col == Some(col_idx + 1)
                && cells.get(col_idx + 1).is_some_and(|n| n.continuation));
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

#[derive(Clone)]
struct CachedSnapshot {
    generation: u64,
    max_lines: usize,
    alive: bool,
    snapshot: PtySnapshot,
}

enum PtyWriteCmd {
    Bytes(Vec<u8>),
    DetachAndExit,
}

fn set_fd_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

fn write_all_fd_blocking(fd: RawFd, data: &[u8]) -> Result<(), String> {
    let mut written = 0usize;
    let mut eagain_retries = 0u32;
    while written < data.len() {
        let rc = unsafe {
            libc::write(
                fd,
                data[written..].as_ptr().cast(),
                data.len() - written,
            )
        };
        if rc > 0 {
            written += rc as usize;
            eagain_retries = 0;
            continue;
        }
        let err = std::io::Error::last_os_error();
        let raw = err.raw_os_error().unwrap_or_default();
        if raw == libc::EINTR {
            continue;
        }
        if (raw == libc::EAGAIN || raw == libc::EWOULDBLOCK) && eagain_retries < 40 {
            eagain_retries += 1;
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            unsafe {
                libc::poll(&mut pfd, 1, 50);
            }
            continue;
        }
        return Err(format!("pty write failed: {err}"));
    }
    Ok(())
}

fn resolve_executable_in_path(program: &str, path_env: &str) -> String {
    if program.contains('/') {
        return program.to_string();
    }
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    for dir in path_env.split(':').filter(|d| !d.is_empty()) {
        let cand = Path::new(dir).join(program);
        if let Ok(meta) = std::fs::metadata(&cand) {
            if meta.is_file() {
                #[cfg(unix)]
                if meta.permissions().mode() & 0o111 != 0 {
                    return cand.to_string_lossy().to_string();
                }
                #[cfg(not(unix))]
                return cand.to_string_lossy().to_string();
            }
        }
    }
    program.to_string()
}

fn build_child_env_cstrings(path_env: &str) -> Result<Vec<CString>, String> {
    let mut envs = Vec::new();
    for (k, v) in std::env::vars() {
        if matches!(
            k.as_str(),
            "TERM" | "COLORTERM" | "LANG" | "LC_ALL" | "PATH"
        ) {
            continue;
        }
        if !k.contains('=') && !k.contains('\0') && !v.contains('\0') {
            if let Ok(c) = CString::new(format!("{k}={v}")) {
                envs.push(c);
            }
        }
    }
    envs.push(CString::new("TERM=xterm-256color").map_err(|e| e.to_string())?);
    envs.push(CString::new("COLORTERM=truecolor").map_err(|e| e.to_string())?);
    envs.push(CString::new("LANG=en_US.UTF-8").map_err(|e| e.to_string())?);
    envs.push(CString::new("LC_ALL=en_US.UTF-8").map_err(|e| e.to_string())?);
    envs.push(CString::new(format!("PATH={path_env}")).map_err(|e| e.to_string())?);
    Ok(envs)
}

fn try_reap_pid(pid_slot: &Arc<Mutex<Option<libc::pid_t>>>, nohang: bool) -> bool {
    let Ok(mut guard) = pid_slot.lock() else {
        return true;
    };
    let Some(pid) = *guard else {
        return true;
    };
    if pid <= 0 {
        *guard = None;
        return true;
    }
    let flags = if nohang { libc::WNOHANG } else { 0 };
    let mut status: libc::c_int = 0;
    loop {
        let rc = unsafe { libc::waitpid(pid, &mut status, flags) };
        if rc < 0 {
            let raw = std::io::Error::last_os_error().raw_os_error();
            if raw == Some(libc::EINTR) {
                continue;
            }
            // ECHILD or already reaped.
            *guard = None;
            return true;
        }
        if rc == 0 {
            return false;
        }
        *guard = None;
        return true;
    }
}

struct PtySessionHandle {
    master_fd: RawFd,
    pid_slot: Arc<Mutex<Option<libc::pid_t>>>,
    writer_tx: Option<SyncSender<PtyWriteCmd>>,
    cols: u16,
    rows: u16,
    alive: Arc<AtomicBool>,
    detach_on_drop: bool,
    attached_hc: bool,
    started_at: Instant,
    screen: Arc<Mutex<VtScreenBuffer>>,
    snapshot_cache: Mutex<Option<CachedSnapshot>>,
}

impl Drop for PtySessionHandle {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        let tx = self.writer_tx.take();
        if self.detach_on_drop {
            if let Some(ref sender) = tx {
                if sender.try_send(PtyWriteCmd::DetachAndExit).is_err() && self.master_fd >= 0 {
                    let _ = write_all_fd_blocking(self.master_fd, &[0x1c_u8]);
                }
            } else if self.master_fd >= 0 {
                let _ = write_all_fd_blocking(self.master_fd, &[0x1c_u8]);
            }
        }
        drop(tx);

        let pid_opt = self.pid_slot.lock().ok().and_then(|g| *g);
        if !self.detach_on_drop {
            if let Some(pid) = pid_opt.filter(|&p| p > 0) {
                unsafe {
                    if libc::kill(-pid, libc::SIGHUP) != 0 {
                        libc::kill(pid, libc::SIGHUP);
                    }
                }
            }
        }
        if self.master_fd >= 0 {
            unsafe {
                libc::close(self.master_fd);
            }
            self.master_fd = -1;
        }
        if let Some(pid) = pid_opt.filter(|&p| p > 0) {
            let pid_slot = Arc::clone(&self.pid_slot);
            let detach_on_drop = self.detach_on_drop;
            std::thread::spawn(move || {
                let max_polls = if detach_on_drop { 60 } else { 25 };
                for _ in 0..max_polls {
                    if try_reap_pid(&pid_slot, true) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                if let Ok(g) = pid_slot.lock() {
                    if *g == Some(pid) {
                        unsafe {
                            let sig = if detach_on_drop { libc::SIGHUP } else { libc::SIGKILL };
                            if libc::kill(-pid, sig) != 0 {
                                libc::kill(pid, sig);
                            }
                            if detach_on_drop {
                                std::thread::sleep(Duration::from_millis(100));
                                if libc::kill(-pid, libc::SIGKILL) != 0 {
                                    libc::kill(pid, libc::SIGKILL);
                                }
                            }
                        }
                    }
                }
                let _ = try_reap_pid(&pid_slot, false);
            });
        }
    }
}

struct RemoteVtHandle {
    cols: u16,
    rows: u16,
    alive: Arc<AtomicBool>,
    started_at: Instant,
    screen: Arc<Mutex<VtScreenBuffer>>,
    snapshot_cache: Mutex<Option<CachedSnapshot>>,
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
                snapshot_cache: Mutex::new(None),
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
        set_fd_cloexec(master_fd);
        set_fd_cloexec(slave_fd);

        let path_env = crate::config::augmented_gui_path_string();
        let resolved_prog = resolve_executable_in_path(program, &path_env);
        let prog_c = CString::new(resolved_prog).map_err(|e| e.to_string())?;
        let cwd_c = CString::new(cwd.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let argv_c: Vec<CString> = argv
            .iter()
            .map(|a| CString::new(a.as_str()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut argv_ptrs: Vec<*const libc::c_char> =
            argv_c.iter().map(|s| s.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        let env_c = build_child_env_cstrings(&path_env)?;
        let mut env_ptrs: Vec<*const libc::c_char> =
            env_c.iter().map(|s| s.as_ptr()).collect();
        env_ptrs.push(std::ptr::null());

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
                libc::execve(prog_c.as_ptr(), argv_ptrs.as_ptr(), env_ptrs.as_ptr());
                libc::_exit(127);
            }
        }

        unsafe {
            libc::close(slave_fd);
        }

        let reader_fd = unsafe { libc::fcntl(master_fd, libc::F_DUPFD_CLOEXEC, 0) };
        if reader_fd < 0 {
            unsafe {
                libc::kill(-pid, libc::SIGHUP);
                libc::close(master_fd);
            }
            let pid_slot = Arc::new(Mutex::new(Some(pid)));
            let _ = try_reap_pid(&pid_slot, false);
            return Err("dup(master_fd) failed".to_string());
        }
        let writer_fd = unsafe { libc::fcntl(master_fd, libc::F_DUPFD_CLOEXEC, 0) };
        if writer_fd < 0 {
            unsafe {
                libc::kill(-pid, libc::SIGHUP);
                libc::close(reader_fd);
                libc::close(master_fd);
            }
            let pid_slot = Arc::new(Mutex::new(Some(pid)));
            let _ = try_reap_pid(&pid_slot, false);
            return Err("dup(writer_fd) failed".to_string());
        }

        let alive = Arc::new(AtomicBool::new(true));
        let screen = Arc::new(Mutex::new(VtScreenBuffer::new(cols as usize, rows as usize)));
        let pid_slot = Arc::new(Mutex::new(Some(pid)));

        let alive_bg = Arc::clone(&alive);
        let screen_bg = Arc::clone(&screen);
        let pid_slot_reader = Arc::clone(&pid_slot);
        let ui_dirty = Arc::clone(&self.ui_dirty);

        let (writer_tx, writer_rx) = mpsc::sync_channel::<PtyWriteCmd>(512);
        std::thread::spawn(move || {
            while let Ok(cmd) = writer_rx.recv() {
                match cmd {
                    PtyWriteCmd::Bytes(bytes) => {
                        if write_all_fd_blocking(writer_fd, &bytes).is_err() {
                            break;
                        }
                    }
                    PtyWriteCmd::DetachAndExit => {
                        let _ = write_all_fd_blocking(writer_fd, &[0x1c_u8]);
                        break;
                    }
                }
            }
            unsafe {
                libc::close(writer_fd);
            }
        });

        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let n = unsafe { libc::read(reader_fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    let raw = std::io::Error::last_os_error().raw_os_error();
                    if raw == Some(libc::EINTR) {
                        continue;
                    }
                    break;
                }
                if n == 0 {
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
            for _ in 0..10 {
                if try_reap_pid(&pid_slot_reader, true) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            alive_bg.store(false, Ordering::SeqCst);
            ui_dirty.store(true, Ordering::SeqCst);
        });

        self.sessions.insert(
            id.to_string(),
            PtySessionHandle {
                master_fd,
                pid_slot,
                writer_tx: Some(writer_tx),
                cols,
                rows,
                alive,
                detach_on_drop,
                attached_hc: detach_on_drop,
                started_at: Instant::now(),
                screen,
                snapshot_cache: Mutex::new(None),
            },
        );
        self.ui_dirty.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub fn write_input(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        let Some(session) = self.sessions.get(id) else {
            return Err(format!("PTY session '{id}' not found"));
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let Some(ref tx) = session.writer_tx else {
            return Err(format!("PTY writer closed for '{id}'"));
        };
        match tx.try_send(PtyWriteCmd::Bytes(bytes.to_vec())) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(format!("PTY write queue full for '{id}'")),
            Err(TrySendError::Disconnected(_)) => Err(format!("PTY writer disconnected for '{id}'")),
        }
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
                build_pty_snapshot_cached(
                    &session.screen,
                    &session.snapshot_cache,
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
                build_pty_snapshot_cached(
                    &remote.screen,
                    &remote.snapshot_cache,
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

fn build_pty_snapshot_cached(
    screen: &Arc<Mutex<VtScreenBuffer>>,
    cache: &Mutex<Option<CachedSnapshot>>,
    alive: bool,
    started_at: Instant,
    attached_hc: bool,
    max_lines: usize,
) -> PtySnapshot {
    let Ok(guard) = screen.lock() else {
        return PtySnapshot {
            lines: Vec::new(),
            styled_lines: Vec::new(),
            alive,
            osc_status: None,
            started_at,
            attached_hc,
            generation: 0,
        };
    };
    let generation = guard.generation();
    if let Ok(c) = cache.lock() {
        if let Some(ref hit) = *c {
            if hit.generation == generation && hit.max_lines == max_lines && hit.alive == alive {
                return hit.snapshot.clone();
            }
        }
    }
    let styled_lines = guard.render_styled_lines(max_lines, alive);
    let osc_status = guard.osc_status();
    drop(guard);
    let lines = styled_lines.iter().map(|l| l.text.clone()).collect();
    let snap = PtySnapshot {
        lines,
        styled_lines,
        alive,
        osc_status,
        started_at,
        attached_hc,
        generation,
    };
    if let Ok(mut c) = cache.lock() {
        *c = Some(CachedSnapshot {
            generation,
            max_lines,
            alive,
            snapshot: snap.clone(),
        });
    }
    snap
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
