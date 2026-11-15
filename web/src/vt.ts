export function escapeHtml(str: string): string {
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/**
 * Validates href destinations so untrusted `javascript:` / `data:` URLs cannot execute.
 */
export function safeHref(url: string | null | undefined): string {
  if (!url) return "#";
  const trimmed = String(url).trim();
  if (!trimmed) return "#";
  if (trimmed.startsWith("/") && !trimmed.startsWith("//")) return escapeHtml(trimmed);
  if (trimmed.startsWith("#")) return escapeHtml(trimmed);
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol === "https:" || parsed.protocol === "http:") {
      return escapeHtml(parsed.toString());
    }
  } catch {
    // invalid URL -> fall through
  }
  return "#";
}

export interface VtHerdrStatus {
  state: string;
  kind?: string;
  app?: string;
  title?: string;
  msg?: string;
}

interface VtStyle {
  fg: string | null;
  bg: string | null;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  inverse: boolean;
}

interface VtCell {
  ch: string;
  width: 0 | 1 | 2;
  style: VtStyle;
}

const DEFAULT_STYLE: VtStyle = {
  fg: null,
  bg: null,
  bold: false,
  dim: false,
  italic: false,
  underline: false,
  inverse: false,
};

function cloneStyle(s: VtStyle): VtStyle {
  return { ...s };
}

function sameStyle(a: VtStyle, b: VtStyle): boolean {
  return (
    a.fg === b.fg &&
    a.bg === b.bg &&
    a.bold === b.bold &&
    a.dim === b.dim &&
    a.italic === b.italic &&
    a.underline === b.underline &&
    a.inverse === b.inverse
  );
}

const ANSI_16 = [
  "#18181b", "#ef4444", "#22c55e", "#f59e0b", "#3b82f6", "#a855f7", "#06b6d4", "#e4e4e7",
  "#71717a", "#f87171", "#4ade80", "#fbbf24", "#60a5fa", "#c084fc", "#22d3ee", "#ffffff",
];

function color256(idx: number): string {
  const n = Math.max(0, Math.min(255, idx | 0));
  if (n < 16) return ANSI_16[n];
  if (n >= 232) {
    const v = 8 + (n - 232) * 10;
    return `rgb(${v},${v},${v})`;
  }
  const c = n - 16;
  const rIdx = Math.floor(c / 36);
  const gIdx = Math.floor((c % 36) / 6);
  const bIdx = c % 6;
  const comp = (i: number) => (i === 0 ? 0 : 55 + i * 40);
  return `rgb(${comp(rIdx)},${comp(gIdx)},${comp(bIdx)})`;
}

function charDisplayWidth(ch: string): 1 | 2 {
  const cp = ch.codePointAt(0) ?? 0;
  if (
    (cp >= 0x1100 && cp <= 0x115f) ||
    cp === 0x2329 ||
    cp === 0x232a ||
    (cp >= 0x2e80 && cp <= 0x303e) ||
    (cp >= 0x3040 && cp <= 0xa4cf) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe10 && cp <= 0xfe19) ||
    (cp >= 0xfe30 && cp <= 0xfe6f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1faff) ||
    (cp >= 0x20000 && cp <= 0x3fffd)
  ) {
    return 2;
  }
  return 1;
}

function makeBlankRow(cols: number): VtCell[] {
  const row: VtCell[] = [];
  for (let i = 0; i < cols; i++) {
    row.push({ ch: " ", width: 1, style: { ...DEFAULT_STYLE } });
  }
  return row;
}

type ParserState = "ground" | "esc" | "escIntermediate" | "csi" | "osc" | "oscEsc";

/**
 * Stateful 2D VT100/xterm screen buffer with streaming UTF-8 decoding, carriage-return overwrite,
 * CJK wide characters, alternate screen buffer (`?1049h`/`l`), scrollback, and `OSC 7501` extraction.
 */
export class WebVtTerminal {
  public cols: number;
  public rows: number;
  private maxScrollback: number;
  private primaryGrid: VtCell[][];
  private altGrid: VtCell[][];
  private primaryScrollback: VtCell[][] = [];
  private usingAltScreen = false;
  private cursorRow = 0;
  private cursorCol = 0;
  private savedCursor: { row: number; col: number; style: VtStyle } | null = null;
  private scrollTop = 0;
  private scrollBottom: number;
  private pendingWrap = false;
  private style: VtStyle = { ...DEFAULT_STYLE };
  private utf8Decoder = new TextDecoder("utf-8", { fatal: false });
  private parserState: ParserState = "ground";
  private csiBuf = "";
  private oscBuf = "";
  public latestStatus: VtHerdrStatus | null = null;
  public windowTitle: string | null = null;

  constructor(cols = 108, rows = 32, maxScrollback = 400) {
    this.cols = Math.max(20, cols);
    this.rows = Math.max(6, rows);
    this.maxScrollback = maxScrollback;
    this.scrollBottom = this.rows - 1;
    this.primaryGrid = this.createEmptyGrid(this.cols, this.rows);
    this.altGrid = this.createEmptyGrid(this.cols, this.rows);
  }

  private createEmptyGrid(cols: number, rows: number): VtCell[][] {
    const grid: VtCell[][] = [];
    for (let r = 0; r < rows; r++) grid.push(makeBlankRow(cols));
    return grid;
  }

  private activeGrid(): VtCell[][] {
    return this.usingAltScreen ? this.altGrid : this.primaryGrid;
  }

  reset(): void {
    this.primaryGrid = this.createEmptyGrid(this.cols, this.rows);
    this.altGrid = this.createEmptyGrid(this.cols, this.rows);
    this.primaryScrollback = [];
    this.usingAltScreen = false;
    this.cursorRow = 0;
    this.cursorCol = 0;
    this.savedCursor = null;
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
    this.pendingWrap = false;
    this.style = { ...DEFAULT_STYLE };
    this.utf8Decoder = new TextDecoder("utf-8", { fatal: false });
    this.parserState = "ground";
    this.csiBuf = "";
    this.oscBuf = "";
  }

  loadSnapshot(text: string): void {
    this.reset();
    // Snapshots use '\n' line separators; normalize bare LF to CRLF while preserving explicit '\r'.
    const normalized = text.replace(/\r?\n/g, "\r\n");
    this.feedText(normalized);
  }

  feedBytes(bytes: Uint8Array): void {
    if (bytes.length === 0) return;
    const chunk = this.utf8Decoder.decode(bytes, { stream: true });
    if (chunk) this.feedText(chunk);
  }

  feedText(text: string): void {
    for (const ch of text) {
      this.stepChar(ch);
    }
  }

  resize(nextCols: number, nextRows: number): void {
    const newCols = Math.max(20, nextCols);
    const newRows = Math.max(6, nextRows);
    if (newCols === this.cols && newRows === this.rows) return;
    this.primaryGrid = this.resizeGrid(this.primaryGrid, newCols, newRows);
    this.altGrid = this.resizeGrid(this.altGrid, newCols, newRows);
    this.cols = newCols;
    this.rows = newRows;
    this.scrollTop = 0;
    this.scrollBottom = newRows - 1;
    this.cursorRow = Math.max(0, Math.min(newRows - 1, this.cursorRow));
    this.cursorCol = Math.max(0, Math.min(newCols - 1, this.cursorCol));
    this.pendingWrap = false;
  }

  private resizeGrid(old: VtCell[][], newCols: number, newRows: number): VtCell[][] {
    const out: VtCell[][] = [];
    const copyRows = Math.min(old.length, newRows);
    for (let r = 0; r < copyRows; r++) {
      const row = makeBlankRow(newCols);
      const copyCols = Math.min(old[r].length, newCols);
      for (let c = 0; c < copyCols; c++) {
        row[c] = { ...old[r][c], style: cloneStyle(old[r][c].style) };
      }
      out.push(row);
    }
    while (out.length < newRows) out.push(makeBlankRow(newCols));
    return out;
  }

  private stepChar(ch: string): void {
    const cp = ch.codePointAt(0) ?? 0;

    if (this.parserState === "osc") {
      if (cp === 0x07) {
        this.finishOsc();
        this.parserState = "ground";
      } else if (cp === 0x1b) {
        this.parserState = "oscEsc";
      } else {
        if (this.oscBuf.length < 8192) this.oscBuf += ch;
      }
      return;
    }

    if (this.parserState === "oscEsc") {
      if (ch === "\\") {
        this.finishOsc();
        this.parserState = "ground";
      } else {
        this.oscBuf = "";
        this.parserState = "esc";
        this.stepChar(ch);
      }
      return;
    }

    if (this.parserState === "escIntermediate") {
      if (cp >= 0x30 && cp <= 0x7e) {
        this.parserState = "ground";
      }
      return;
    }

    if (this.parserState === "esc") {
      if (ch === "[") {
        this.csiBuf = "";
        this.parserState = "csi";
      } else if (ch === "]") {
        this.oscBuf = "";
        this.parserState = "osc";
      } else if (ch === "(" || ch === ")" || ch === "*" || ch === "+") {
        this.parserState = "escIntermediate";
      } else if (ch === "7") {
        this.savedCursor = { row: this.cursorRow, col: this.cursorCol, style: cloneStyle(this.style) };
        this.parserState = "ground";
      } else if (ch === "8") {
        if (this.savedCursor) {
          this.cursorRow = Math.min(this.rows - 1, this.savedCursor.row);
          this.cursorCol = Math.min(this.cols - 1, this.savedCursor.col);
          this.style = cloneStyle(this.savedCursor.style);
        }
        this.parserState = "ground";
      } else if (ch === "D") {
        this.lineFeed();
        this.parserState = "ground";
      } else if (ch === "E") {
        this.cursorCol = 0;
        this.lineFeed();
        this.parserState = "ground";
      } else if (ch === "M") {
        this.reverseIndex();
        this.parserState = "ground";
      } else if (ch === "c") {
        this.reset();
      } else {
        this.parserState = "ground";
      }
      return;
    }

    if (this.parserState === "csi") {
      if (cp === 0x1b) {
        this.parserState = "esc";
        return;
      }
      if (cp >= 0x40 && cp <= 0x7e) {
        this.handleCsi(this.csiBuf, ch);
        this.parserState = "ground";
      } else {
        if (this.csiBuf.length < 256) this.csiBuf += ch;
      }
      return;
    }

    // Ground state
    if (cp === 0x1b) {
      this.parserState = "esc";
      return;
    }
    if (cp === 0x0d) {
      this.cursorCol = 0;
      this.pendingWrap = false;
      return;
    }
    if (cp === 0x0a || cp === 0x0b || cp === 0x0c) {
      this.lineFeed();
      this.pendingWrap = false;
      return;
    }
    if (cp === 0x08) {
      if (this.pendingWrap) {
        this.pendingWrap = false;
      } else if (this.cursorCol > 0) {
        this.cursorCol -= 1;
      }
      return;
    }
    if (cp === 0x09) {
      this.pendingWrap = false;
      const nextStop = Math.min(this.cols - 1, (Math.floor(this.cursorCol / 8) + 1) * 8);
      this.cursorCol = nextStop;
      return;
    }
    if (cp < 0x20 || cp === 0x7f) return;

    this.putGlyph(ch);
  }

  private clearWidePartner(grid: VtCell[][], row: number, col: number): void {
    const cell = grid[row]?.[col];
    if (!cell) return;
    if (cell.width === 2 && col + 1 < this.cols) {
      grid[row][col + 1] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
    } else if (cell.width === 0 && col > 0) {
      grid[row][col - 1] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
    }
  }

  private putGlyph(ch: string): void {
    const w = charDisplayWidth(ch);
    if (this.pendingWrap) {
      this.cursorCol = 0;
      this.lineFeed();
      this.pendingWrap = false;
    }
    if (w === 2 && this.cursorCol === this.cols - 1) {
      const grid = this.activeGrid();
      this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
      grid[this.cursorRow][this.cursorCol] = { ch: " ", width: 1, style: cloneStyle(this.style) };
      this.cursorCol = 0;
      this.lineFeed();
    }

    const grid = this.activeGrid();
    this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
    grid[this.cursorRow][this.cursorCol] = { ch, width: w, style: cloneStyle(this.style) };

    if (w === 2 && this.cursorCol + 1 < this.cols) {
      this.clearWidePartner(grid, this.cursorRow, this.cursorCol + 1);
      grid[this.cursorRow][this.cursorCol + 1] = { ch: "", width: 0, style: cloneStyle(this.style) };
      if (this.cursorCol + 2 >= this.cols) {
        this.pendingWrap = true;
      } else {
        this.cursorCol += 2;
      }
    } else if (this.cursorCol + 1 >= this.cols) {
      this.pendingWrap = true;
    } else {
      this.cursorCol += 1;
    }
  }

  private lineFeed(): void {
    if (this.cursorRow >= this.scrollBottom) {
      this.scrollUp(1);
      this.cursorRow = this.scrollBottom;
    } else {
      this.cursorRow = Math.min(this.rows - 1, this.cursorRow + 1);
    }
  }

  private reverseIndex(): void {
    if (this.cursorRow <= this.scrollTop) {
      this.scrollDown(1);
      this.cursorRow = this.scrollTop;
    } else {
      this.cursorRow = Math.max(0, this.cursorRow - 1);
    }
  }

  private scrollUp(lines: number): void {
    const n = Math.max(1, Math.min(lines, this.scrollBottom - this.scrollTop + 1));
    const grid = this.activeGrid();
    for (let i = 0; i < n; i++) {
      const evicted = grid.splice(this.scrollTop, 1)[0];
      if (!this.usingAltScreen && this.scrollTop === 0 && evicted) {
        this.primaryScrollback.push(evicted);
        if (this.primaryScrollback.length > this.maxScrollback) {
          this.primaryScrollback.shift();
        }
      }
      grid.splice(this.scrollBottom, 0, makeBlankRow(this.cols));
    }
  }

  private scrollDown(lines: number): void {
    const n = Math.max(1, Math.min(lines, this.scrollBottom - this.scrollTop + 1));
    const grid = this.activeGrid();
    for (let i = 0; i < n; i++) {
      grid.splice(this.scrollBottom, 1);
      grid.splice(this.scrollTop, 0, makeBlankRow(this.cols));
    }
  }

  private handleCsi(rawParams: string, cmd: string): void {
    const isPrivate = rawParams.startsWith("?");
    const clean = (isPrivate ? rawParams.slice(1) : rawParams).replace(/^[<=>!]+/, "");
    const parts = clean ? clean.split(";") : [];
    const param = (idx: number, def = 1) => {
      const n = parseInt(parts[idx] || "", 10);
      return Number.isFinite(n) && n > 0 ? n : def;
    };
    const paramZero = (idx: number, def = 0) => {
      const n = parseInt(parts[idx] || "", 10);
      return Number.isFinite(n) && n >= 0 ? n : def;
    };

    if (isPrivate) {
      const modes = parts.map((p) => parseInt(p, 10)).filter((n) => Number.isFinite(n));
      if (cmd === "h") {
        for (const m of modes) {
          if (m === 1049 || m === 1047 || m === 47) {
            if (m === 1049) {
              this.savedCursor = { row: this.cursorRow, col: this.cursorCol, style: cloneStyle(this.style) };
            }
            this.usingAltScreen = true;
            this.altGrid = this.createEmptyGrid(this.cols, this.rows);
            this.cursorRow = 0;
            this.cursorCol = 0;
            this.pendingWrap = false;
          }
        }
      } else if (cmd === "l") {
        for (const m of modes) {
          if (m === 1049 || m === 1047 || m === 47) {
            this.usingAltScreen = false;
            if (m === 1049 && this.savedCursor) {
              this.cursorRow = Math.min(this.rows - 1, this.savedCursor.row);
              this.cursorCol = Math.min(this.cols - 1, this.savedCursor.col);
              this.style = cloneStyle(this.savedCursor.style);
            }
            this.pendingWrap = false;
          }
        }
      }
      return;
    }

    this.pendingWrap = false;
    const grid = this.activeGrid();
    switch (cmd) {
      case "A": this.cursorRow = Math.max(this.scrollTop, this.cursorRow - param(0)); break;
      case "B": this.cursorRow = Math.min(this.scrollBottom, this.cursorRow + param(0)); break;
      case "C": this.cursorCol = Math.min(this.cols - 1, this.cursorCol + param(0)); break;
      case "D": this.cursorCol = Math.max(0, this.cursorCol - param(0)); break;
      case "E": this.cursorRow = Math.min(this.scrollBottom, this.cursorRow + param(0)); this.cursorCol = 0; break;
      case "F": this.cursorRow = Math.max(this.scrollTop, this.cursorRow - param(0)); this.cursorCol = 0; break;
      case "G": this.cursorCol = Math.max(0, Math.min(this.cols - 1, param(0) - 1)); break;
      case "d": this.cursorRow = Math.max(0, Math.min(this.rows - 1, param(0) - 1)); break;
      case "H":
      case "f":
        this.cursorRow = Math.max(0, Math.min(this.rows - 1, param(0) - 1));
        this.cursorCol = Math.max(0, Math.min(this.cols - 1, param(1) - 1));
        break;
      case "J": {
        const mode = paramZero(0, 0);
        if (mode === 0) {
          for (let c = this.cursorCol; c < this.cols; c++) grid[this.cursorRow][c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
          for (let r = this.cursorRow + 1; r < this.rows; r++) grid[r] = makeBlankRow(this.cols);
        } else if (mode === 1) {
          for (let r = 0; r < this.cursorRow; r++) grid[r] = makeBlankRow(this.cols);
          for (let c = 0; c <= this.cursorCol; c++) grid[this.cursorRow][c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
        } else if (mode === 2 || mode === 3) {
          for (let r = 0; r < this.rows; r++) grid[r] = makeBlankRow(this.cols);
          if (mode === 3 && !this.usingAltScreen) this.primaryScrollback = [];
        }
        break;
      }
      case "K": {
        const mode = paramZero(0, 0);
        if (mode === 0) {
          this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
          for (let c = this.cursorCol; c < this.cols; c++) grid[this.cursorRow][c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
        } else if (mode === 1) {
          for (let c = 0; c <= this.cursorCol; c++) grid[this.cursorRow][c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
        } else if (mode === 2) {
          grid[this.cursorRow] = makeBlankRow(this.cols);
        }
        break;
      }
      case "X": {
        const n = Math.min(param(0), this.cols - this.cursorCol);
        for (let i = 0; i < n; i++) grid[this.cursorRow][this.cursorCol + i] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
        break;
      }
      case "L": {
        const n = Math.min(param(0), this.scrollBottom - this.cursorRow + 1);
        for (let i = 0; i < n; i++) {
          grid.splice(this.scrollBottom, 1);
          grid.splice(this.cursorRow, 0, makeBlankRow(this.cols));
        }
        break;
      }
      case "M": {
        const n = Math.min(param(0), this.scrollBottom - this.cursorRow + 1);
        for (let i = 0; i < n; i++) {
          grid.splice(this.cursorRow, 1);
          grid.splice(this.scrollBottom, 0, makeBlankRow(this.cols));
        }
        break;
      }
      case "@": {
        const n = Math.min(param(0), this.cols - this.cursorCol);
        for (let i = 0; i < n; i++) {
          grid[this.cursorRow].splice(this.cursorCol, 0, { ch: " ", width: 1, style: { ...DEFAULT_STYLE } });
          grid[this.cursorRow].pop();
        }
        break;
      }
      case "P": {
        const n = Math.min(param(0), this.cols - this.cursorCol);
        grid[this.cursorRow].splice(this.cursorCol, n);
        for (let i = 0; i < n; i++) {
          grid[this.cursorRow].push({ ch: " ", width: 1, style: { ...DEFAULT_STYLE } });
        }
        break;
      }
      case "S": this.scrollUp(param(0)); break;
      case "T": this.scrollDown(param(0)); break;
      case "r": {
        const t = Math.max(0, Math.min(this.rows - 2, param(0, 1) - 1));
        const b = Math.max(t + 1, Math.min(this.rows - 1, param(1, this.rows) - 1));
        this.scrollTop = t;
        this.scrollBottom = b;
        this.cursorRow = 0;
        this.cursorCol = 0;
        break;
      }
      case "s": this.savedCursor = { row: this.cursorRow, col: this.cursorCol, style: cloneStyle(this.style) }; break;
      case "u":
        if (this.savedCursor) {
          this.cursorRow = Math.min(this.rows - 1, this.savedCursor.row);
          this.cursorCol = Math.min(this.cols - 1, this.savedCursor.col);
          this.style = cloneStyle(this.savedCursor.style);
        }
        break;
      case "m":
        this.applySgr(clean ? clean.split(/[;:]/) : ["0"]);
        break;
    }
  }

  private applySgr(tokens: string[]): void {
    const nums = tokens.map((t) => (t === "" ? 0 : parseInt(t, 10))).filter((n) => !Number.isNaN(n));
    if (nums.length === 0) {
      this.style = { ...DEFAULT_STYLE };
      return;
    }
    let i = 0;
    while (i < nums.length) {
      const code = nums[i];
      if (code === 0) {
        this.style = { ...DEFAULT_STYLE };
      } else if (code === 1) this.style.bold = true;
      else if (code === 2) this.style.dim = true;
      else if (code === 3) this.style.italic = true;
      else if (code === 4) this.style.underline = true;
      else if (code === 7) this.style.inverse = true;
      else if (code === 22) { this.style.bold = false; this.style.dim = false; }
      else if (code === 23) this.style.italic = false;
      else if (code === 24) this.style.underline = false;
      else if (code === 27) this.style.inverse = false;
      else if (code >= 30 && code <= 37) this.style.fg = ANSI_16[code - 30];
      else if (code === 39) this.style.fg = null;
      else if (code >= 90 && code <= 97) this.style.fg = ANSI_16[8 + code - 90];
      else if (code >= 40 && code <= 47) this.style.bg = ANSI_16[code - 40];
      else if (code === 49) this.style.bg = null;
      else if (code >= 100 && code <= 107) this.style.bg = ANSI_16[8 + code - 100];
      else if ((code === 38 || code === 48) && i + 1 < nums.length) {
        const isFg = code === 38;
        const kind = nums[i + 1];
        if (kind === 5 && i + 2 < nums.length) {
          const color = color256(nums[i + 2]);
          if (isFg) this.style.fg = color; else this.style.bg = color;
          i += 2;
        } else if (kind === 2 && i + 4 < nums.length) {
          const r = Math.max(0, Math.min(255, nums[i + 2]));
          const g = Math.max(0, Math.min(255, nums[i + 3]));
          const b = Math.max(0, Math.min(255, nums[i + 4]));
          const color = `rgb(${r},${g},${b})`;
          if (isFg) this.style.fg = color; else this.style.bg = color;
          i += 4;
        }
      }
      i += 1;
    }
  }

  private finishOsc(): void {
    const raw = this.oscBuf;
    this.oscBuf = "";
    const sepIdx = raw.search(/[;:]/);
    if (sepIdx < 0) return;
    const code = raw.slice(0, sepIdx);
    const body = raw.slice(sepIdx + 1);
    if (code === "0" || code === "2") {
      this.windowTitle = body;
      return;
    }
    if (code === "7501") {
      if (body.trim().startsWith("{")) {
        try {
          const obj = JSON.parse(body) as VtHerdrStatus;
          if (obj && typeof obj.state === "string") {
            this.latestStatus = obj;
          }
          return;
        } catch {
          // fall through to kv parsing
        }
      }
      const next: VtHerdrStatus = { state: "working" };
      if (raw[sepIdx] === ":" && body.includes(":")) {
        const idx = body.indexOf(":");
        next.state = body.slice(0, idx).trim();
        next.msg = body.slice(idx + 1).trim();
        this.latestStatus = next;
        return;
      }
      for (const kv of body.split(";")) {
        const eq = kv.indexOf("=");
        if (eq > 0) {
          const k = kv.slice(0, eq).trim();
          const v = kv.slice(eq + 1).trim();
          if (k === "state") next.state = v;
          else if (k === "kind") next.kind = v;
          else if (k === "app") next.app = v;
          else if (k === "title") next.title = v;
          else if (k === "msg") next.msg = v;
        }
      }
      this.latestStatus = next;
    }
  }

  private visibleRows(): VtCell[][] {
    const base = this.usingAltScreen
      ? this.altGrid
      : [...this.primaryScrollback, ...this.primaryGrid];
    let end = base.length;
    while (end > 1 && base[end - 1].every((c) => c.width === 1 && c.ch === " " && c.style.bg === null)) {
      end -= 1;
    }
    return base.slice(0, end);
  }

  renderPlainText(): string {
    return this.visibleRows()
      .map((row) =>
        row
          .filter((c) => c.width !== 0)
          .map((c) => c.ch)
          .join("")
          .replace(/\s+$/, "")
      )
      .join("\n");
  }

  renderHtml(): string {
    const rows = this.visibleRows();
    const linesHtml: string[] = [];
    for (const row of rows) {
      let lastNonEmpty = row.length - 1;
      while (lastNonEmpty >= 0 && row[lastNonEmpty].width === 1 && row[lastNonEmpty].ch === " " && row[lastNonEmpty].style.bg === null && !row[lastNonEmpty].style.inverse) {
        lastNonEmpty -= 1;
      }
      if (lastNonEmpty < 0) {
        linesHtml.push(" ");
        continue;
      }
      let out = "";
      let runStyle: VtStyle | null = null;
      let runText = "";
      const flushRun = () => {
        if (!runText) return;
        const escaped = escapeHtml(runText);
        if (!runStyle || sameStyle(runStyle, DEFAULT_STYLE)) {
          out += escaped;
        } else {
          const css: string[] = [];
          const fg = runStyle.inverse ? (runStyle.bg || "var(--bg-surface)") : runStyle.fg;
          const bg = runStyle.inverse ? (runStyle.fg || "var(--text-primary)") : runStyle.bg;
          if (fg) css.push(`color:${fg}`);
          if (bg) css.push(`background-color:${bg}`);
          if (runStyle.bold) css.push("font-weight:600");
          if (runStyle.dim) css.push("opacity:0.72");
          if (runStyle.italic) css.push("font-style:italic");
          if (runStyle.underline) css.push("text-decoration:underline");
          out += css.length > 0 ? `<span style="${css.join(";")}">${escaped}</span>` : escaped;
        }
        runText = "";
      };

      for (let c = 0; c <= lastNonEmpty; c++) {
        const cell = row[c];
        if (cell.width === 0) continue;
        if (!runStyle || !sameStyle(runStyle, cell.style)) {
          flushRun();
          runStyle = cell.style;
        }
        runText += cell.ch;
      }
      flushRun();
      linesHtml.push(out);
    }
    return linesHtml.join("\n");
  }
}
