export interface HostHealthSnapshot {
  daemon_version: string;
  os: string;
  arch: string;
  cpu_cores: number;
  load_avg_1m?: number | null;
  mem_used_mb?: number | null;
  mem_total_mb?: number | null;
  os_uptime_secs?: number | null;
  daemon_uptime_secs: number;
  herdr_version?: string | null;
  ota_supported: boolean;
}

export interface WebTerminalSearchMatch {
  rowIdx: number;
  start: number;
  end: number;
}

export const MOBILE_QUICK_CHIPS: readonly string[] = [
  "/compact",
  "/clear",
  "git status",
  "git diff",
  "cargo test",
];

/**
 * Case-insensitive search across VT plain-text lines (`WebVtTerminal.renderPlainText()`).
 */
export function findWebTerminalMatches(
  plainText: string,
  query: string
): WebTerminalSearchMatch[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const lines = plainText.split("\n");
  const out: WebTerminalSearchMatch[] = [];
  for (let rowIdx = 0; rowIdx < lines.length; rowIdx += 1) {
    const line = lines[rowIdx];
    if (!line) continue;
    const lower = line.toLowerCase();
    let from = 0;
    while (from + needle.length <= lower.length) {
      const hit = lower.indexOf(needle, from);
      if (hit < 0) break;
      out.push({ rowIdx, start: hit, end: hit + needle.length });
      if (out.length >= 2000) return out;
      from = hit + needle.length;
    }
  }
  return out;
}

/**
 * Extracts validated `http://` and `https://` URLs from a terminal line,
 * stopping cleanly at CJK/non-ASCII characters, quotes, and unbalanced trailing punctuation.
 */
export function extractSafeTerminalUrls(line: string): string[] {
  if (!line || (!line.includes("http://") && !line.includes("https://"))) {
    return [];
  }
  const urls: string[] = [];
  const re = /https?:\/\/[\x21-\x7e]+/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(line)) !== null) {
    let raw = m[0].replace(/["'<>`].*$/, "");
    while (raw.length > 0) {
      const last = raw[raw.length - 1];
      if (last === "." || last === "," || last === ";" || last === ":" || last === "!" || last === "?") {
        raw = raw.slice(0, -1);
        continue;
      }
      if (last === ")") {
        const opens = (raw.match(/\(/g) || []).length;
        const closes = (raw.match(/\)/g) || []).length;
        if (closes > opens) {
          raw = raw.slice(0, -1);
          continue;
        }
      }
      if (last === "]") {
        const opens = (raw.match(/\[/g) || []).length;
        const closes = (raw.match(/\]/g) || []).length;
        if (closes > opens) {
          raw = raw.slice(0, -1);
          continue;
        }
      }
      break;
    }
    try {
      const parsed = new URL(raw);
      if (parsed.protocol === "https:" || parsed.protocol === "http:") {
        urls.push(parsed.toString());
      }
    } catch {
      // ignore malformed URL
    }
  }
  return urls;
}

export function firstSafeTerminalUrlInLine(line: string): string | null {
  const found = extractSafeTerminalUrls(line);
  return found.length > 0 ? found[0] : null;
}

export function buildWebSessionMarkdown(
  title: string,
  cwd: string,
  plainText: string,
  exportedIso = new Date().toISOString()
): string {
  const rawLines = plainText.split("\n");
  let end = rawLines.length;
  while (end > 0 && rawLines[end - 1].trim() === "") {
    end -= 1;
  }
  const body = rawLines
    .slice(0, end)
    .map((l) => l.replace(/\s+$/, ""))
    .join("\n");
  return [
    `# Coda Session Transcript — ${title || "Terminal"}`,
    "",
    `- **Exported**: \`${exportedIso}\``,
    `- **Working Directory**: \`${cwd || "~"}\``,
    `- **Lines**: \`${end}\``,
    "",
    "```text",
    body,
    "```",
    "",
  ].join("\n");
}

export function triggerMarkdownDownload(title: string, markdown: string): void {
  if (typeof document === "undefined" || typeof Blob === "undefined") return;
  const slug =
    String(title || "session")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 36) || "session";
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const blob = new Blob([markdown], { type: "text/markdown;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `coda-${slug}-${stamp}.md`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function formatWebHostHealth(h: HostHealthSnapshot | null | undefined): string {
  if (!h) return "";
  const ver = h.daemon_version.startsWith("v") ? h.daemon_version : `v${h.daemon_version}`;
  const parts: string[] = [ver, `${h.cpu_cores}c`];
  if (typeof h.load_avg_1m === "number" && Number.isFinite(h.load_avg_1m)) {
    parts.push(`load ${h.load_avg_1m.toFixed(2)}`);
  }
  if (
    typeof h.mem_used_mb === "number" &&
    typeof h.mem_total_mb === "number" &&
    h.mem_total_mb > 0
  ) {
    parts.push(`${(h.mem_used_mb / 1024).toFixed(1)}/${(h.mem_total_mb / 1024).toFixed(1)}G`);
  }
  return parts.join(" · ");
}
