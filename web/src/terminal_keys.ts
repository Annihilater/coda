export interface TerminalKeyEventLike {
  key: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
  isComposing?: boolean;
}

/**
 * Encodes a DOM KeyboardEvent (or compatible struct) into an xterm/VT100 byte sequence.
 * Returns `null` when the browser shortcut should pass through untouched (e.g. Cmd+C/V/R/L).
 */
export function encodeTerminalKeyEvent(
  e: TerminalKeyEventLike,
  opts: { appCursorKeys?: boolean; imeComposing?: boolean } = {}
): string | null {
  if (e.isComposing || opts.imeComposing) return null;
  // V3-24: Never hijack macOS Cmd+* shortcuts (Cmd+K, Cmd+C/V/R/L, Cmd+Backspace, Cmd+Arrows, etc.).
  if (e.metaKey) {
    return null;
  }
  const lower = e.key.toLowerCase();

  const appCursor = Boolean(opts.appCursorKeys);

  if (e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1) {
    const code = lower.charCodeAt(0);
    if (code >= 97 && code <= 122) {
      return String.fromCharCode(code - 96);
    }
    if (e.key === "[" || e.key === "3") return "\x1b";
    if (e.key === "\\" || e.key === "4") return "\x1c";
    if (e.key === "]" || e.key === "5") return "\x1d";
    if (e.key === "^" || e.key === "6") return "\x1e";
    if (e.key === "_" || e.key === "7") return "\x1f";
  }

  switch (e.key) {
    case "Enter":
      return "\r";
    case "Backspace":
      return "\x7f";
    case "Tab":
      return e.shiftKey ? "\x1b[Z" : "\t";
    case "Escape":
      return "\x1b";
    case "ArrowUp":
      return appCursor ? "\x1bOA" : "\x1b[A";
    case "ArrowDown":
      return appCursor ? "\x1bOB" : "\x1b[B";
    case "ArrowRight":
      return appCursor ? "\x1bOC" : "\x1b[C";
    case "ArrowLeft":
      return appCursor ? "\x1bOD" : "\x1b[D";
    case "Home":
      return "\x1b[H";
    case "End":
      return "\x1b[F";
    case "Insert":
      return "\x1b[2~";
    case "Delete":
      return "\x1b[3~";
    case "PageUp":
      return "\x1b[5~";
    case "PageDown":
      return "\x1b[6~";
    case "F1":
      return "\x1bOP";
    case "F2":
      return "\x1bOQ";
    case "F3":
      return "\x1bOR";
    case "F4":
      return "\x1bOS";
    case "F5":
      return "\x1b[15~";
    case "F6":
      return "\x1b[17~";
    case "F7":
      return "\x1b[18~";
    case "F8":
      return "\x1b[19~";
    case "F9":
      return "\x1b[20~";
    case "F10":
      return "\x1b[21~";
    case "F11":
      return "\x1b[23~";
    case "F12":
      return "\x1b[24~";
    default:
      if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
        return e.altKey ? `\x1b${e.key}` : e.key;
      }
      return null;
  }
}

export function wrapBracketedPaste(text: string, bracketedPaste: boolean): string {
  return bracketedPaste ? `\x1b[200~${text}\x1b[201~` : text;
}
