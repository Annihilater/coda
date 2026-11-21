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

interface VtRow {
  cells: VtCell[];
  ver: number;
}

export interface VtDomPatchStats {
  patched: number;
  reused: number;
  total: number;
  skippedProtected: boolean;
}

export interface VtRowNodeLike {
  className: string;
  innerHTML: string;
  getAttribute(name: string): string | null;
  setAttribute(name: string, value: string): void;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  contains?(node: any): boolean;
  remove(): void;
}

export interface VtDomContainerLike {
  innerHTML: string;
  children: ArrayLike<VtRowNodeLike>;
  childElementCount: number;
  getAttribute(name: string): string | null;
  setAttribute(name: string, value: string): void;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  appendChild(node: any): unknown;
  ownerDocument?: {
    createElement(tag: string): VtRowNodeLike;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    createDocumentFragment?(): { appendChild(node: any): unknown };
  } | null;
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

function isZeroWidthChar(cp: number): boolean {
  return (
    cp === 0x200b ||
    cp === 0x200c ||
    cp === 0x200d ||
    cp === 0x2060 ||
    cp === 0xfeff ||
    (cp >= 0x0300 && cp <= 0x036f) ||
    (cp >= 0x1ab0 && cp <= 0x1aff) ||
    (cp >= 0x1dc0 && cp <= 0x1dff) ||
    (cp >= 0x20d0 && cp <= 0x20ff) ||
    (cp >= 0xfe00 && cp <= 0xfe0f) ||
    (cp >= 0xfe20 && cp <= 0xfe2f) ||
    (cp >= 0xe0100 && cp <= 0xe01ef)
  );
}

function charDisplayWidth(ch: string): 0 | 1 | 2 {
  const cp = ch.codePointAt(0) ?? 0;
  if (isZeroWidthChar(cp)) return 0;
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

type ParserState = "ground" | "esc" | "escIntermediate" | "csi" | "osc" | "oscEsc" | "strSkip" | "strSkipEsc";

interface SavedBufferState {
  row: number;
  col: number;
  style: VtStyle;
  scrollTop: number;
  scrollBottom: number;
  savedCursor: { row: number; col: number; style: VtStyle } | null;
}

let globalRowVer = 1;

/**
 * Stateful 2D VT100/xterm screen buffer with streaming UTF-8 decoding, carriage-return overwrite,
 * CJK wide characters, zero-width combining marks, alternate screen buffer (`?1049h`/`l`),
 * DCS/APC/PM/SOS string skip, scrollback, and `OSC 7501` extraction.
 */
export class WebVtTerminal {
  public cols: number;
  public rows: number;
  private maxScrollback: number;
  private primaryGrid: VtRow[];
  private altGrid: VtRow[];
  private primaryScrollback: VtRow[] = [];
  private usingAltScreen = false;
  private cursorRow = 0;
  private cursorCol = 0;
  private savedCursor: { row: number; col: number; style: VtStyle } | null = null;
  private primarySavedOnAlt: SavedBufferState | null = null;
  private scrollTop = 0;
  private scrollBottom: number;
  private pendingWrap = false;
  private style: VtStyle = { ...DEFAULT_STYLE };
  private utf8Decoder = new TextDecoder("utf-8", { fatal: false });
  private parserState: ParserState = "ground";
  private csiBuf = "";
  private oscBuf = "";
  public cursorVisible = true;
  public bracketedPaste = false;
  public bracketedPasteExplicit: boolean | null = null;
  public appCursorKeys = false;
  public latestStatus: VtHerdrStatus | null = null;
  private pendingStatus: VtHerdrStatus | null = null;
  private scrollbackEvictions = 0;
  public windowTitle: string | null = null;

  constructor(cols = 108, rows = 32, maxScrollback = 400) {
    this.cols = Math.max(20, cols);
    this.rows = Math.max(6, rows);
    this.maxScrollback = maxScrollback;
    this.scrollBottom = this.rows - 1;
    this.primaryGrid = this.createEmptyGrid(this.cols, this.rows);
    this.altGrid = this.createEmptyGrid(this.cols, this.rows);
  }

  /** Consumes any newly parsed OSC 7501 status since the previous call so stale NeedsYou never resurrects. */
  consumeLatestStatus(): VtHerdrStatus | null {
    const out = this.pendingStatus;
    this.pendingStatus = null;
    return out;
  }

  getScrollbackEvictions(): number {
    return this.scrollbackEvictions;
  }

  private makeRow(cols: number): VtRow {
    return { cells: makeBlankRow(cols), ver: ++globalRowVer };
  }

  private bumpRow(row: VtRow): void {
    row.ver = ++globalRowVer;
  }

  private createEmptyGrid(cols: number, rows: number): VtRow[] {
    const grid: VtRow[] = [];
    for (let r = 0; r < rows; r++) grid.push(this.makeRow(cols));
    return grid;
  }

  private activeGrid(): VtRow[] {
    return this.usingAltScreen ? this.altGrid : this.primaryGrid;
  }

  reset(): void {
    this.primaryGrid = this.createEmptyGrid(this.cols, this.rows);
    this.altGrid = this.createEmptyGrid(this.cols, this.rows);
    this.primaryScrollback = [];
    this.scrollbackEvictions = 0;
    this.usingAltScreen = false;
    this.cursorRow = 0;
    this.cursorCol = 0;
    this.savedCursor = null;
    this.primarySavedOnAlt = null;
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
    this.pendingWrap = false;
    this.style = { ...DEFAULT_STYLE };
    this.cursorVisible = true;
    this.bracketedPaste = false;
    this.bracketedPasteExplicit = null;
    this.appCursorKeys = false;
    this.latestStatus = null;
    this.pendingStatus = null;
    this.windowTitle = null;
    this.utf8Decoder = new TextDecoder("utf-8", { fatal: false });
    this.parserState = "ground";
    this.csiBuf = "";
    this.oscBuf = "";
  }

  loadSnapshot(text: string): void {
    const prevBracketed = this.bracketedPaste;
    const prevBracketedExplicit = this.bracketedPasteExplicit;
    const prevAppCursorKeys = this.appCursorKeys;
    this.reset();
    if (prevBracketed) {
      this.bracketedPaste = true;
    }
    if (prevBracketedExplicit !== null) {
      this.bracketedPasteExplicit = prevBracketedExplicit;
    }
    if (prevAppCursorKeys) {
      this.appCursorKeys = true;
    }
    // Snapshots use '\n' line separators; normalize bare LF to CRLF while preserving explicit '\r'.
    const normalized = text.replace(/\r?\n/g, "\r\n");
    this.feedText(normalized);
    // Clear edge-triggered pendingStatus after historical snapshot replay so subsequent plain stdout
    // chunks do not re-emit an already-resolved OSC 7501 state.
    this.pendingStatus = null;
  }

  isUsingAltScreen(): boolean {
    return this.usingAltScreen;
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

  private resizeGrid(old: VtRow[], newCols: number, newRows: number): VtRow[] {
    const out: VtRow[] = [];
    const copyRows = Math.min(old.length, newRows);
    for (let r = 0; r < copyRows; r++) {
      const row = this.makeRow(newCols);
      const copyCols = Math.min(old[r].cells.length, newCols);
      for (let c = 0; c < copyCols; c++) {
        row.cells[c] = { ...old[r].cells[c], style: cloneStyle(old[r].cells[c].style) };
      }
      out.push(row);
    }
    while (out.length < newRows) out.push(this.makeRow(newCols));
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

    if (this.parserState === "strSkip") {
      if (cp === 0x07) {
        this.parserState = "ground";
      } else if (cp === 0x1b) {
        this.parserState = "strSkipEsc";
      }
      return;
    }

    if (this.parserState === "strSkipEsc") {
      if (ch === "\\") {
        this.parserState = "ground";
      } else {
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
      } else if (ch === "P" || ch === "_" || ch === "^" || ch === "X") {
        // DCS, APC, PM, SOS string sequences terminated by ST (ESC \)
        this.parserState = "strSkip";
      } else if (ch === "(" || ch === ")" || ch === "*" || ch === "+" || ch === "#") {
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

  private clearWidePartner(grid: VtRow[], row: number, col: number): void {
    const line = grid[row];
    const cell = line?.cells[col];
    if (!line || !cell) return;
    if (cell.width === 2 && col + 1 < this.cols) {
      line.cells[col + 1] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
      this.bumpRow(line);
    } else if (cell.width === 0 && col > 0) {
      line.cells[col - 1] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
      this.bumpRow(line);
    }
  }

  private putGlyph(ch: string): void {
    const w = charDisplayWidth(ch);
    if (w === 0) {
      // Zero-width combining mark / VS16 / ZWJ attaches to previous cell without advancing cursor.
      const grid = this.activeGrid();
      const targetCol = this.pendingWrap ? this.cols - 1 : this.cursorCol - 1;
      if (targetCol >= 0 && targetCol < this.cols) {
        const line = grid[this.cursorRow];
        let baseCol = targetCol;
        if (line.cells[baseCol].width === 0 && baseCol > 0) {
          baseCol -= 1;
        }
        const cell = line.cells[baseCol];
        if (cell.ch.length < 16) {
          cell.ch += ch;
          this.bumpRow(line);
        }
      }
      return;
    }
    if (this.pendingWrap) {
      this.cursorCol = 0;
      this.lineFeed();
      this.pendingWrap = false;
    }
    if (w === 2 && this.cursorCol === this.cols - 1) {
      const grid = this.activeGrid();
      this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
      grid[this.cursorRow].cells[this.cursorCol] = { ch: " ", width: 1, style: cloneStyle(this.style) };
      this.bumpRow(grid[this.cursorRow]);
      this.cursorCol = 0;
      this.lineFeed();
    }

    const grid = this.activeGrid();
    const line = grid[this.cursorRow];
    this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
    line.cells[this.cursorCol] = { ch, width: w, style: cloneStyle(this.style) };
    this.bumpRow(line);

    if (w === 2 && this.cursorCol + 1 < this.cols) {
      this.clearWidePartner(grid, this.cursorRow, this.cursorCol + 1);
      line.cells[this.cursorCol + 1] = { ch: "", width: 0, style: cloneStyle(this.style) };
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
          this.scrollbackEvictions += 1;
        }
      }
      grid.splice(this.scrollBottom, 0, this.makeRow(this.cols));
    }
  }

  private scrollDown(lines: number): void {
    const n = Math.max(1, Math.min(lines, this.scrollBottom - this.scrollTop + 1));
    const grid = this.activeGrid();
    for (let i = 0; i < n; i++) {
      grid.splice(this.scrollBottom, 1);
      grid.splice(this.scrollTop, 0, this.makeRow(this.cols));
    }
  }

  private handleCsi(rawParams: string, cmd: string): void {
    let prefix = "";
    let idxStart = 0;
    if (rawParams.length > 0 && "?><=".includes(rawParams[0])) {
      prefix = rawParams[0];
      idxStart = 1;
    }
    let body = rawParams.slice(idxStart);
    let intermediates = "";
    const interIdx = body.search(/[\x20-\x2f]/);
    if (interIdx >= 0) {
      intermediates = body.slice(interIdx);
      body = body.slice(0, interIdx);
    }

    // Ignore non-`?` private prefixes (`>`, `<`, `=`) and intermediate CSI sequences
    // (e.g. Kitty keyboard `CSI > 1 u`, modifyOtherKeys `CSI > 4 ; 2 m`, DECSCUSR `CSI 2 q`)
    // so they never mis-trigger SGR/cursor/erase commands.
    if (intermediates.length > 0 || (prefix !== "" && prefix !== "?")) {
      return;
    }

    const isPrivate = prefix === "?";
    const parts = body ? body.split(";") : [];
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
          if (m === 1) {
            this.appCursorKeys = true;
          } else if (m === 25) {
            this.cursorVisible = true;
          } else if (m === 2004) {
            this.bracketedPaste = true;
            this.bracketedPasteExplicit = true;
          } else if (m === 1049 || m === 1047 || m === 47) {
            if (!this.usingAltScreen) {
              this.primarySavedOnAlt = {
                row: this.cursorRow,
                col: this.cursorCol,
                style: cloneStyle(this.style),
                scrollTop: this.scrollTop,
                scrollBottom: this.scrollBottom,
                savedCursor: this.savedCursor ? { ...this.savedCursor, style: cloneStyle(this.savedCursor.style) } : null,
              };
            }
            this.usingAltScreen = true;
            this.altGrid = this.createEmptyGrid(this.cols, this.rows);
            this.scrollTop = 0;
            this.scrollBottom = this.rows - 1;
            this.cursorRow = 0;
            this.cursorCol = 0;
            this.savedCursor = null;
            this.pendingWrap = false;
          }
        }
      } else if (cmd === "l") {
        for (const m of modes) {
          if (m === 1) {
            this.appCursorKeys = false;
          } else if (m === 25) {
            this.cursorVisible = false;
          } else if (m === 2004) {
            this.bracketedPaste = false;
            this.bracketedPasteExplicit = false;
          } else if (m === 1049 || m === 1047 || m === 47) {
            this.usingAltScreen = false;
            if (this.primarySavedOnAlt) {
              const saved = this.primarySavedOnAlt;
              this.primarySavedOnAlt = null;
              this.scrollTop = Math.max(0, Math.min(this.rows - 1, saved.scrollTop));
              this.scrollBottom = Math.max(this.scrollTop, Math.min(this.rows - 1, saved.scrollBottom));
              this.savedCursor = saved.savedCursor;
              if (m === 1049) {
                this.cursorRow = Math.min(this.rows - 1, saved.row);
                this.cursorCol = Math.min(this.cols - 1, saved.col);
                this.style = cloneStyle(saved.style);
              }
            } else {
              this.scrollTop = 0;
              this.scrollBottom = this.rows - 1;
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
          const line = grid[this.cursorRow];
          for (let c = this.cursorCol; c < this.cols; c++) line.cells[c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
          this.bumpRow(line);
          for (let r = this.cursorRow + 1; r < this.rows; r++) grid[r] = this.makeRow(this.cols);
        } else if (mode === 1) {
          for (let r = 0; r < this.cursorRow; r++) grid[r] = this.makeRow(this.cols);
          const line = grid[this.cursorRow];
          for (let c = 0; c <= this.cursorCol; c++) line.cells[c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
          this.bumpRow(line);
        } else if (mode === 2 || mode === 3) {
          for (let r = 0; r < this.rows; r++) grid[r] = this.makeRow(this.cols);
          if (mode === 3 && !this.usingAltScreen) this.primaryScrollback = [];
        }
        break;
      }
      case "K": {
        const mode = paramZero(0, 0);
        const line = grid[this.cursorRow];
        if (mode === 0) {
          this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
          for (let c = this.cursorCol; c < this.cols; c++) line.cells[c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
          this.bumpRow(line);
        } else if (mode === 1) {
          for (let c = 0; c <= this.cursorCol; c++) line.cells[c] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
          this.bumpRow(line);
        } else if (mode === 2) {
          grid[this.cursorRow] = this.makeRow(this.cols);
        }
        break;
      }
      case "X": {
        const line = grid[this.cursorRow];
        const n = Math.min(param(0), this.cols - this.cursorCol);
        for (let i = 0; i < n; i++) line.cells[this.cursorCol + i] = { ch: " ", width: 1, style: { ...DEFAULT_STYLE } };
        this.bumpRow(line);
        break;
      }
      case "L": {
        if (this.cursorRow < this.scrollTop || this.cursorRow > this.scrollBottom) break;
        const n = Math.min(param(0), this.scrollBottom - this.cursorRow + 1);
        for (let i = 0; i < n; i++) {
          grid.splice(this.scrollBottom, 1);
          grid.splice(this.cursorRow, 0, this.makeRow(this.cols));
        }
        break;
      }
      case "M": {
        if (this.cursorRow < this.scrollTop || this.cursorRow > this.scrollBottom) break;
        const n = Math.min(param(0), this.scrollBottom - this.cursorRow + 1);
        for (let i = 0; i < n; i++) {
          grid.splice(this.cursorRow, 1);
          grid.splice(this.scrollBottom, 0, this.makeRow(this.cols));
        }
        break;
      }
      case "@": {
        const line = grid[this.cursorRow];
        this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
        const n = Math.min(param(0), this.cols - this.cursorCol);
        for (let i = 0; i < n; i++) {
          line.cells.splice(this.cursorCol, 0, { ch: " ", width: 1, style: { ...DEFAULT_STYLE } });
          line.cells.pop();
        }
        this.bumpRow(line);
        break;
      }
      case "P": {
        const line = grid[this.cursorRow];
        this.clearWidePartner(grid, this.cursorRow, this.cursorCol);
        const n = Math.min(param(0), this.cols - this.cursorCol);
        line.cells.splice(this.cursorCol, n);
        for (let i = 0; i < n; i++) {
          line.cells.push({ ch: " ", width: 1, style: { ...DEFAULT_STYLE } });
        }
        this.bumpRow(line);
        break;
      }
      case "S": this.scrollUp(param(0)); break;
      case "T": this.scrollDown(param(0)); break;
      case "r": {
        const rawTop = param(0, 1) - 1;
        const rawBot = param(1, this.rows) - 1;
        const t = Math.max(0, Math.min(this.rows - 1, rawTop));
        const b = Math.max(0, Math.min(this.rows - 1, rawBot));
        if (t < b) {
          this.scrollTop = t;
          this.scrollBottom = b;
        } else {
          this.scrollTop = 0;
          this.scrollBottom = this.rows - 1;
        }
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
        this.applySgr(body ? body.split(";") : ["0"]);
        break;
    }
  }

  private applySgr(segments: string[]): void {
    if (segments.length === 0) {
      this.style = { ...DEFAULT_STYLE };
      return;
    }
    let i = 0;
    while (i < segments.length) {
      const seg = segments[i];
      if (seg.includes(":")) {
        // ITU T.416 colon-separated extended SGR, e.g. `38:2::R:G:B`, `38:2:R:G:B`, `38:5:N`, `4:3`
        const sub = seg.split(":");
        const mainCode = parseInt(sub[0] || "0", 10);
        if (mainCode === 38 || mainCode === 48) {
          const isFg = mainCode === 38;
          const kind = parseInt(sub[1] || "", 10);
          if (kind === 5 && sub.length >= 3) {
            const color = color256(parseInt(sub[2] || "0", 10) || 0);
            if (isFg) this.style.fg = color; else this.style.bg = color;
          } else if (kind === 2) {
            // 38:2:colorspace:R:G:B (length >= 6) or 38:2:R:G:B (length === 5)
            const off = sub.length >= 6 ? 3 : 2;
            if (sub.length >= off + 3) {
              const r = Math.max(0, Math.min(255, parseInt(sub[off] || "0", 10) || 0));
              const g = Math.max(0, Math.min(255, parseInt(sub[off + 1] || "0", 10) || 0));
              const b = Math.max(0, Math.min(255, parseInt(sub[off + 2] || "0", 10) || 0));
              const color = `rgb(${r},${g},${b})`;
              if (isFg) this.style.fg = color; else this.style.bg = color;
            }
          }
        } else if (mainCode === 4) {
          this.style.underline = sub[1] !== "0";
        }
        i += 1;
        continue;
      }

      const code = seg === "" ? 0 : parseInt(seg, 10);
      if (Number.isNaN(code)) {
        i += 1;
        continue;
      }
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
      else if ((code === 38 || code === 48) && i + 1 < segments.length) {
        const isFg = code === 38;
        const kind = parseInt(segments[i + 1] || "", 10);
        if (kind === 5 && i + 2 < segments.length) {
          const color = color256(parseInt(segments[i + 2] || "0", 10) || 0);
          if (isFg) this.style.fg = color; else this.style.bg = color;
          i += 2;
        } else if (kind === 2 && i + 4 < segments.length) {
          const r = Math.max(0, Math.min(255, parseInt(segments[i + 2] || "0", 10) || 0));
          const g = Math.max(0, Math.min(255, parseInt(segments[i + 3] || "0", 10) || 0));
          const b = Math.max(0, Math.min(255, parseInt(segments[i + 4] || "0", 10) || 0));
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
            this.pendingStatus = obj;
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
        this.pendingStatus = next;
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
      this.pendingStatus = next;
    }
  }

  private visibleRows(includeCursorRow = false): { rows: VtRow[]; cursorVisRow: number } {
    const base = this.usingAltScreen
      ? this.altGrid
      : [...this.primaryScrollback, ...this.primaryGrid];
    const absCursorRow = this.usingAltScreen
      ? this.cursorRow
      : this.primaryScrollback.length + this.cursorRow;
    const minEnd = includeCursorRow ? Math.min(base.length, absCursorRow + 1) : 1;
    let end = base.length;
    while (
      end > minEnd &&
      base[end - 1].cells.every((c) => c.width === 1 && c.ch === " " && c.style.bg === null && !c.style.inverse)
    ) {
      end -= 1;
    }
    return { rows: base.slice(0, end), cursorVisRow: absCursorRow };
  }

  renderPlainText(): string {
    return this.visibleRows(false)
      .rows.map((row) =>
        row.cells
          .filter((c) => c.width !== 0)
          .map((c) => c.ch)
          .join("")
          .replace(/\s+$/, "")
      )
      .join("\n");
  }

  private renderSingleRowHtml(row: VtCell[], cursorColHere: number): string {
    const hasCursorHere = cursorColHere >= 0;
    let lastNonEmpty = row.length - 1;
    while (
      lastNonEmpty >= 0 &&
      !(hasCursorHere && lastNonEmpty === cursorColHere) &&
      row[lastNonEmpty].width === 1 &&
      row[lastNonEmpty].ch === " " &&
      row[lastNonEmpty].style.bg === null &&
      !row[lastNonEmpty].style.inverse
    ) {
      lastNonEmpty -= 1;
    }
    if (lastNonEmpty < 0) {
      return " ";
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
      if (hasCursorHere && c === cursorColHere) {
        flushRun();
        runStyle = null;
        out += `<span class="term-cursor">${escapeHtml(cell.ch || " ")}</span>`;
        continue;
      }
      if (!runStyle || !sameStyle(runStyle, cell.style)) {
        flushRun();
        runStyle = cell.style;
      }
      runText += cell.ch;
    }
    flushRun();
    return out;
  }

  renderHtml(showCursor = true): string {
    const drawCursor = showCursor && this.cursorVisible;
    const { rows, cursorVisRow } = this.visibleRows(drawCursor);
    const linesHtml: string[] = [];
    for (let rIdx = 0; rIdx < rows.length; rIdx++) {
      const cursorColHere = drawCursor && rIdx === cursorVisRow ? this.cursorCol : -1;
      linesHtml.push(this.renderSingleRowHtml(rows[rIdx].cells, cursorColHere));
    }
    return linesHtml.join("\n");
  }

  /**
   * Renders initial `<div class="vt-row" data-vt-key="...">` HTML so the first `rAF`
   * immediately reuses existing row nodes via `patchDom` without rebuilding the DOM.
   */
  renderInitialRowsHtml(showCursor = true): string {
    const drawCursor = showCursor && this.cursorVisible;
    const { rows, cursorVisRow } = this.visibleRows(drawCursor);
    let out = "";
    for (let rIdx = 0; rIdx < rows.length; rIdx++) {
      const row = rows[rIdx];
      const cursorColHere = drawCursor && rIdx === cursorVisRow ? this.cursorCol : -1;
      const key = `${row.ver}:${cursorColHere}`;
      out += `<div class="vt-row" data-vt-key="${key}">${this.renderSingleRowHtml(row.cells, cursorColHere)}</div>`;
    }
    return out;
  }

  /**
   * Row-level dirty-diff DOM patcher (60fps zero-jitter):
   * - Pops evicted scrollback rows from the head in O(1) so streaming stays O(1) after scrollback fills.
   * - Reuses unchanged `<div class="vt-row">` elements without regenerating HTML.
   * - Skips rows overlapping an active text selection (including partial-outside selections via `containsNode`).
   * - Batches newly appended rows through a `DocumentFragment`.
   */
  patchDom(
    container: VtDomContainerLike,
    showCursor = true,
    protectedSelection?: {
      anchorNode?: unknown;
      focusNode?: unknown;
      containsNode?: (node: VtRowNodeLike) => boolean;
    } | null
  ): VtDomPatchStats {
    const drawCursor = showCursor && this.cursorVisible;
    const { rows, cursorVisRow } = this.visibleRows(drawCursor);
    const doc = container.ownerDocument ?? (typeof document !== "undefined" ? (document as unknown as NonNullable<VtDomContainerLike["ownerDocument"]>) : null);

    if (container.getAttribute("data-vt-mode") !== "rows") {
      container.innerHTML = "";
      container.setAttribute("data-vt-mode", "rows");
      container.setAttribute("data-vt-evictions", String(this.scrollbackEvictions));
      container.setAttribute("data-vt-alt", this.usingAltScreen ? "1" : "0");
    }

    let skippedProtected = false;
    const prevEvictionsAttr = container.getAttribute("data-vt-evictions");
    const prevAlt = container.getAttribute("data-vt-alt") === "1";

    // Compute protected row index range FIRST so active selections inside `#term-screen-output`
    // freeze head evictions cleanly without corrupting unshifted row indices.
    let protMin = -1;
    let protMax = -1;
    if (protectedSelection) {
      const { anchorNode, focusNode, containsNode } = protectedSelection;
      if (containsNode) {
        for (let i = 0; i < container.childElementCount; i++) {
          const child = container.children[i];
          if (child && containsNode(child)) {
            if (protMin < 0) protMin = i;
            protMax = i;
          }
        }
      }
      if (protMin < 0 && (anchorNode || focusNode)) {
        if (anchorNode === container || focusNode === container) {
          protMin = 0;
          protMax = Math.max(0, container.childElementCount - 1);
        } else {
          let anchorIdx = -1;
          let focusIdx = -1;
          for (let i = 0; i < container.childElementCount; i++) {
            const child = container.children[i];
            if (!child?.contains) continue;
            if (anchorIdx < 0 && anchorNode && child.contains(anchorNode)) anchorIdx = i;
            if (focusIdx < 0 && focusNode && child.contains(focusNode)) focusIdx = i;
            if ((anchorIdx >= 0 || !anchorNode) && (focusIdx >= 0 || !focusNode)) break;
          }
          if (anchorIdx >= 0 && focusIdx >= 0) {
            protMin = Math.min(anchorIdx, focusIdx);
            protMax = Math.max(anchorIdx, focusIdx);
          } else if (anchorIdx >= 0) {
            protMin = anchorIdx;
            protMax = anchorIdx;
          } else if (focusIdx >= 0) {
            protMin = focusIdx;
            protMax = focusIdx;
          }
        }
      }
    }
    const hasProtectedRows = protMin >= 0;
    const isProtectedIdx = (idx: number): boolean => hasProtectedRows && idx >= protMin && idx <= protMax;

    // V3-23: Pop newly evicted primary scrollback rows from the front so remaining 400 rows keep exact
    // positional alignment and reuse in O(1) instead of rewriting every row.
    if (!this.usingAltScreen && !prevAlt && prevEvictionsAttr !== null) {
      const prevEvictions = Number(prevEvictionsAttr);
      if (Number.isFinite(prevEvictions) && this.scrollbackEvictions > prevEvictions) {
        const delta = this.scrollbackEvictions - prevEvictions;
        if (hasProtectedRows) {
          // Freeze the entire viewport while rows are selected during scrollback eviction so row indices
          // never shift or duplicate; `selectionchange` will catch up in O(1) once selection clears.
          return {
            patched: 0,
            reused: container.childElementCount,
            total: rows.length,
            skippedProtected: true,
          };
        }
        if (delta > 0 && delta < container.childElementCount) {
          for (let i = 0; i < delta; i++) {
            container.children[0]?.remove();
          }
        }
        container.setAttribute("data-vt-evictions", String(this.scrollbackEvictions));
      } else {
        container.setAttribute("data-vt-evictions", String(this.scrollbackEvictions));
      }
    } else if (!hasProtectedRows) {
      container.setAttribute("data-vt-evictions", String(this.scrollbackEvictions));
    }
    container.setAttribute("data-vt-alt", this.usingAltScreen ? "1" : "0");

    // Trim trailing rows when screen shrinks or switches into shorter alt-screen.
    while (container.childElementCount > rows.length) {
      const lastIdx = container.childElementCount - 1;
      const last = container.children[lastIdx];
      if (!last) break;
      if (isProtectedIdx(lastIdx)) {
        skippedProtected = true;
        break;
      }
      last.remove();
    }

    let patched = 0;
    let reused = 0;
    const frag = doc?.createDocumentFragment ? doc.createDocumentFragment() : null;
    let fragCount = 0;

    for (let rIdx = 0; rIdx < rows.length; rIdx++) {
      const row = rows[rIdx];
      const cursorColHere = drawCursor && rIdx === cursorVisRow ? this.cursorCol : -1;
      const nextKey = `${row.ver}:${cursorColHere}`;
      const existing = container.children[rIdx];

      if (existing) {
        if (existing.getAttribute("data-vt-key") === nextKey) {
          reused += 1;
          continue;
        }
        if (isProtectedIdx(rIdx)) {
          reused += 1;
          skippedProtected = true;
          continue;
        }
        existing.innerHTML = this.renderSingleRowHtml(row.cells, cursorColHere);
        existing.setAttribute("data-vt-key", nextKey);
        patched += 1;
      } else if (doc) {
        const div = doc.createElement("div");
        div.className = "vt-row";
        div.setAttribute("data-vt-key", nextKey);
        div.innerHTML = this.renderSingleRowHtml(row.cells, cursorColHere);
        if (frag) {
          frag.appendChild(div);
          fragCount += 1;
        } else {
          container.appendChild(div);
        }
        patched += 1;
      }
    }

    if (frag && fragCount > 0) {
      container.appendChild(frag);
    }

    return { patched, reused, total: rows.length, skippedProtected };
  }
}
