import { escapeHtml } from "./vt";
import { svgIcon } from "./icons";
import type { Device, HerdrTerminal } from "./types";
import { isNeedsYouState } from "./notify";

// ============================================================================
// 1. Remote E2EE Interactive Git Diff Parser, Filter & Modal Renderer
// ============================================================================

export type GitChangeBadge = "M" | "A" | "D" | "R" | "C" | "U" | "??";

export interface GitChangedFile {
  status: GitChangeBadge;
  rawStatus: string;
  path: string;
  origPath: string | null;
}

export interface GitDiffLine {
  kind: "meta" | "hunk" | "add" | "del" | "ctx";
  text: string;
  oldLine: number | null;
  newLine: number | null;
}

export interface GitDiffModalState {
  open: boolean;
  cwd: string;
  branch: string;
  statusShort: string;
  diffUnified: string;
  files: GitChangedFile[];
  selectedFile: string | null;
  loading: boolean;
  error: string | null;
  reqId: string | null;
}

export function createInitialGitDiffState(): GitDiffModalState {
  return {
    open: false,
    cwd: "",
    branch: "",
    statusShort: "",
    diffUnified: "",
    files: [],
    selectedFile: null,
    loading: false,
    error: null,
    reqId: null,
  };
}

function unquoteGitPath(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')) {
    return trimmed
      .slice(1, -1)
      .replace(/\\"/g, '"')
      .replace(/\\\\/g, "\\");
  }
  return trimmed;
}

function classifyStatusBadge(rawCode: string): GitChangeBadge {
  const clean = rawCode.trim().toUpperCase();
  if (clean.includes("?")) return "??";
  if (clean.includes("U")) return "U";
  if (clean.includes("R")) return "R";
  if (clean.includes("C")) return "C";
  if (clean.includes("A")) return "A";
  if (clean.includes("D")) return "D";
  return "M";
}

/**
 * Parses `git status --short` lines (e.g. ` M src/app.ts`, `A  new.ts`, `?? untracked.png`,
 * `R  old.ts -> new.ts`) into structured file descriptors for interactive filter pills.
 */
export function parseGitStatusShort(statusShort: string): GitChangedFile[] {
  if (!statusShort) return [];
  const rawLines = statusShort.split(/\r?\n/);
  const out: GitChangedFile[] = [];
  const seen = new Set<string>();

  for (const rawLine of rawLines) {
    const line = rawLine.replace(/\r$/, "");
    if (!line.trim()) continue;

    let rawCode = "";
    let restPath = "";
    if (line.length > 3 && /^[ MADRCU?!]{2}$/.test(line.slice(0, 2)) && line[2] === " ") {
      rawCode = line.slice(0, 2);
      restPath = line.slice(3);
    } else {
      const m = line.match(/^\s*([MADRCU?!]{1,2})\s+(.+)$/);
      if (!m) continue;
      rawCode = m[1];
      restPath = m[2];
    }

    let origPath: string | null = null;
    let finalPath = "";
    const arrowIdx = restPath.indexOf(" -> ");
    if (arrowIdx >= 0) {
      origPath = unquoteGitPath(restPath.slice(0, arrowIdx));
      finalPath = unquoteGitPath(restPath.slice(arrowIdx + 4));
    } else {
      finalPath = unquoteGitPath(restPath);
    }

    if (!finalPath || seen.has(finalPath)) continue;
    seen.add(finalPath);
    out.push({
      status: classifyStatusBadge(rawCode),
      rawStatus: rawCode.trim() || "M",
      path: finalPath,
      origPath,
    });
  }

  return out;
}

function extractDiffBlockPaths(blockLines: string[]): string[] {
  const candidates: string[] = [];
  const pushClean = (raw: string, prefix: "a/" | "b/") => {
    const uq = unquoteGitPath(raw);
    if (!uq || uq === "/dev/null") return;
    const stripped = uq.startsWith(prefix) ? uq.slice(2) : uq;
    if (stripped && !candidates.includes(stripped)) {
      candidates.push(stripped);
    }
  };

  for (let i = 0; i < Math.min(blockLines.length, 10); i += 1) {
    const line = blockLines[i];
    if (line.startsWith("+++ ")) {
      pushClean(line.slice(4), "b/");
    } else if (line.startsWith("--- ")) {
      pushClean(line.slice(4), "a/");
    } else if (line.startsWith("rename to ")) {
      const p = unquoteGitPath(line.slice("rename to ".length));
      if (p && !candidates.includes(p)) candidates.push(p);
    } else if (line.startsWith("rename from ")) {
      const p = unquoteGitPath(line.slice("rename from ".length));
      if (p && !candidates.includes(p)) candidates.push(p);
    }
  }

  const header = blockLines[0] || "";
  const m = header.match(/^diff --git "?a\/(.+?)"? "?b\/(.+?)"?$/);
  if (m) {
    if (m[2] && !candidates.includes(m[2])) candidates.push(m[2]);
    if (m[1] && !candidates.includes(m[1])) candidates.push(m[1]);
  }
  return candidates;
}

/**
 * Filters a multi-file unified git diff down to hunks matching `selectedFile` (or returns full diff when `null`).
 */
export function filterUnifiedDiffByFile(
  diffUnified: string,
  selectedFile: string | null,
  origPath?: string | null
): string {
  if (!diffUnified || !selectedFile) return diffUnified || "";
  const rawLines = diffUnified.replace(/\r\n/g, "\n").split("\n");
  const blocks: string[][] = [];
  let current: string[] | null = null;

  for (const line of rawLines) {
    if (line.startsWith("diff --git ")) {
      if (current) blocks.push(current);
      current = [line];
    } else if (current) {
      current.push(line);
    }
  }
  if (current) blocks.push(current);

  const targets = new Set([selectedFile, ...(origPath ? [origPath] : [])]);
  const matched = blocks.filter((blk) => {
    const paths = extractDiffBlockPaths(blk);
    return paths.some((p) => targets.has(p));
  });

  return matched.map((blk) => blk.join("\n")).join("\n");
}

/**
 * Parses unified diff text into syntax-classified lines with old/new line numbers and `+added / -deleted` totals.
 */
export function parseUnifiedDiffLines(diffText: string): {
  lines: GitDiffLine[];
  added: number;
  deleted: number;
} {
  if (!diffText || !diffText.trim()) {
    return { lines: [], added: 0, deleted: 0 };
  }
  const rawLines = diffText.replace(/\r\n/g, "\n").split("\n");
  if (rawLines.length > 0 && rawLines[rawLines.length - 1] === "") {
    rawLines.pop();
  }

  const out: GitDiffLine[] = [];
  let added = 0;
  let deleted = 0;
  let oldLn = 0;
  let newLn = 0;
  let inHunk = false;

  for (const text of rawLines) {
    if (
      text.startsWith("diff --git ") ||
      text.startsWith("index ") ||
      text.startsWith("--- ") ||
      text.startsWith("+++ ") ||
      text.startsWith("new file mode ") ||
      text.startsWith("deleted file mode ") ||
      text.startsWith("old mode ") ||
      text.startsWith("new mode ") ||
      text.startsWith("similarity index ") ||
      text.startsWith("rename from ") ||
      text.startsWith("rename to ") ||
      text.startsWith("Binary files ")
    ) {
      inHunk = false;
      out.push({ kind: "meta", text, oldLine: null, newLine: null });
      continue;
    }

    const hunkMatch = text.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (hunkMatch) {
      oldLn = Number(hunkMatch[1]);
      newLn = Number(hunkMatch[2]);
      inHunk = true;
      out.push({ kind: "hunk", text, oldLine: null, newLine: null });
      continue;
    }

    if (text.startsWith("+")) {
      added += 1;
      const ln = inHunk ? newLn++ : null;
      out.push({ kind: "add", text, oldLine: null, newLine: ln });
      continue;
    }

    if (text.startsWith("-")) {
      deleted += 1;
      const ln = inHunk ? oldLn++ : null;
      out.push({ kind: "del", text, oldLine: ln, newLine: null });
      continue;
    }

    if (text.startsWith("\\ No newline at end of file")) {
      out.push({ kind: "meta", text, oldLine: null, newLine: null });
      continue;
    }

    const oLn = inHunk ? oldLn++ : null;
    const nLn = inHunk ? newLn++ : null;
    out.push({ kind: "ctx", text, oldLine: oLn, newLine: nLn });
  }

  return { lines: out, added, deleted };
}

function badgeClassForGitStatus(st: GitChangeBadge): string {
  switch (st) {
    case "A":
      return "green";
    case "D":
    case "U":
      return "red";
    case "R":
    case "C":
      return "blue";
    case "M":
      return "amber";
    case "??":
      return "";
  }
}

export function renderGitDiffModal(
  st: GitDiffModalState,
  roots: string[],
  tr: (en: string, zh: string) => string
): string {
  if (!st.open) return "";

  const uniqueRoots = Array.from(new Set([st.cwd, ...roots].filter(Boolean)));
  const selectedEntry = st.selectedFile
    ? st.files.find((f) => f.path === st.selectedFile)
    : undefined;
  const filteredDiff = filterUnifiedDiffByFile(
    st.diffUnified,
    st.selectedFile,
    selectedEntry?.origPath
  );
  const fullStats = parseUnifiedDiffLines(st.diffUnified);
  const activeParsed = st.selectedFile ? parseUnifiedDiffLines(filteredDiff) : fullStats;
  const branchLabel = st.branch.trim() || "HEAD";

  return `
    <div class="modal-mask" id="git-diff-mask">
      <div class="modal-card" style="width:940px;max-width:96vw;">
        <div style="display:flex;justify-content:space-between;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:10px;">
          <div style="display:flex;align-items:center;gap:8px;min-width:0;flex-wrap:wrap;">
            ${svgIcon("gitBranch", 15)}
            <strong style="font-size:15px;">${tr("Remote Git Changes / Diff (E2EE)", "远端 Git 变更与 Diff 审查 (端到端加密)")}</strong>
            <span class="pm-badge blue coda-mono">${escapeHtml(branchLabel)}</span>
            <span class="pm-badge coda-mono"><span style="color:var(--emerald);">+${fullStats.added}</span>&nbsp;<span style="color:var(--rose);">-${fullStats.deleted}</span></span>
          </div>
          <div style="display:flex;align-items:center;gap:6px;">
            <button id="btn-refresh-git-diff" type="button" class="pm-btn pm-btn-secondary pm-btn-sm">
              ${st.loading ? tr("Refreshing…", "刷新中…") : tr("Refresh", "刷新")}
            </button>
            <button id="btn-close-git-diff" type="button" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
          </div>
        </div>

        <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:10px;">
          <span style="font-size:11.5px;color:var(--text-tertiary);">${tr("Workspace:", "工作区：")}</span>
          ${
            uniqueRoots.length > 1
              ? `<select id="gd-cwd-select" class="pm-input coda-mono" style="margin:0;width:auto;max-width:420px;height:30px;padding:0 8px;font-size:11.5px;">
                  ${uniqueRoots
                    .map(
                      (r) =>
                        `<option value="${escapeHtml(r)}" ${r === st.cwd ? "selected" : ""}>${escapeHtml(r)}</option>`
                    )
                    .join("")}
                </select>`
              : `<code class="coda-mono" style="font-size:11.5px;">${escapeHtml(st.cwd || "~")}</code>`
          }
        </div>

        ${
          st.error
            ? `<div style="padding:8px 10px;border-radius:8px;background:var(--rose-bg);border:1px solid var(--rose-bd);color:var(--rose);font-size:12px;margin-bottom:10px;">${escapeHtml(st.error)}</div>`
            : ""
        }

        ${
          st.files.length > 0
            ? `<div class="gd-pill-strip" role="tablist" aria-label="${tr("Changed files", "变更文件列表")}">
                <button type="button" class="gd-file-pill ${st.selectedFile === null ? "active" : ""}" data-gd-file="">
                  <span>${tr("All Files", "全部文件")}</span>
                  <span class="pm-badge coda-mono" style="height:18px;padding:0 6px;font-size:10.5px;">${st.files.length}</span>
                </button>
                ${st.files
                  .map(
                    (f) => `
                  <button type="button" class="gd-file-pill ${st.selectedFile === f.path ? "active" : ""}" data-gd-file="${escapeHtml(f.path)}" title="${escapeHtml(f.origPath ? `${f.origPath} -> ${f.path}` : f.path)}">
                    <span class="pm-badge ${badgeClassForGitStatus(f.status)} coda-mono" style="height:18px;padding:0 5px;font-size:10px;">${escapeHtml(f.status)}</span>
                    <span class="coda-mono" style="max-width:240px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escapeHtml(f.path)}</span>
                  </button>`
                  )
                  .join("")}
              </div>`
            : ""
        }

        <div class="gd-viewport coda-mono">
          ${
            st.loading && activeParsed.lines.length === 0 && st.files.length === 0
              ? `<div style="padding:24px;text-align:center;font-size:12px;color:var(--text-tertiary);">${tr("Fetching encrypted Git status & unified diff…", "正在通过端到端加密通道拉取 Git 状态与统一 Diff…")}</div>`
              : activeParsed.lines.length === 0
                ? `<div style="padding:24px;text-align:center;font-size:12px;color:var(--text-tertiary);">
                    ${
                      selectedEntry?.status === "??"
                        ? tr(
                            `Untracked file "${escapeHtml(selectedEntry.path)}" has no tracked diff hunks yet — open Remote Files to preview full content.`,
                            `未跟踪新文件「${escapeHtml(selectedEntry.path)}」尚无 Git 索引对比块，可在「文件」面板直接预览完整源码。`
                          )
                        : st.files.length === 0
                          ? tr("Working tree clean — no uncommitted Git changes.", "工作区干净，当前无未提交的 Git 变更。")
                          : tr("No unified diff lines for the current selection.", "当前筛选条件下无统一 Diff 内容。")
                    }
                  </div>`
                : activeParsed.lines
                    .map(
                      (ln) => `
                <div class="gd-row gd-${ln.kind}">
                  <span class="gd-ln">${ln.oldLine !== null ? ln.oldLine : ""}</span>
                  <span class="gd-ln">${ln.newLine !== null ? ln.newLine : ""}</span>
                  <span class="gd-code">${escapeHtml(ln.text)}</span>
                </div>`
                    )
                    .join("")
          }
        </div>
      </div>
    </div>
  `;
}

// ============================================================================
// 2. Shell Path Escaping & Clipboard Screenshot Upload Helpers
// ============================================================================

/**
 * Safely quotes a remote filesystem path for POSIX shells (`zsh`, `bash`, `sh`) and agent TUIs
 * so spaces, quotes, `$()`, backticks, or metacharacters can never trigger command injection.
 */
export function shellEscapePath(rawPath: string): string {
  const cleaned = String(rawPath || "").replace(/[\r\n\0]/g, "");
  if (!cleaned) return "''";
  if (/^[A-Za-z0-9_./:@+-]+$/.test(cleaned) && !cleaned.startsWith("-")) {
    return cleaned;
  }
  return `'${cleaned.replace(/'/g, "'\\''")}'`;
}

/**
 * Formats an uploaded remote file path with a trailing space for immediate injection via `pty_input`.
 */
export function formatUploadedPathForPty(remotePath: string): string {
  return `${shellEscapePath(remotePath)} `;
}

/**
 * Generates deterministic `clipboard-<timestamp>.png` filenames for pasted screenshot blobs.
 */
export function buildClipboardImageFilename(nowMs = Date.now(), seq = 0): string {
  return seq > 0 ? `clipboard-${nowMs}-${seq}.png` : `clipboard-${nowMs}.png`;
}

// ============================================================================
// 3. Needs-You Capsule Bar, Multi-Line Prompt Composer, Attention Inbox & Speech
// ============================================================================

export type ApprovalCapsuleAction = "allow" | "reject" | "1" | "2" | "3" | "enter";

export interface ApprovalCapsuleItem {
  action: ApprovalCapsuleAction;
  labelEn: string;
  labelZh: string;
  variant: "primary" | "deny" | "num";
}

export const APPROVAL_CAPSULE_ITEMS: readonly ApprovalCapsuleItem[] = [
  { action: "allow", labelEn: "Allow (y)", labelZh: "允许 (y)", variant: "primary" },
  { action: "reject", labelEn: "Reject (Esc)", labelZh: "拒绝 (Esc)", variant: "deny" },
  { action: "1", labelEn: "1", labelZh: "1", variant: "num" },
  { action: "2", labelEn: "2", labelZh: "2", variant: "num" },
  { action: "3", labelEn: "3", labelZh: "3", variant: "num" },
  { action: "enter", labelEn: "Enter", labelZh: "Enter", variant: "num" },
];

export function encodeApprovalActionBytes(action: ApprovalCapsuleAction): string {
  switch (action) {
    case "allow":
      return "y\r";
    case "reject":
      return "\x1b";
    case "1":
      return "1\r";
    case "2":
      return "2\r";
    case "3":
      return "3\r";
    case "enter":
      return "\r";
  }
}

export function renderNeedsYouCapsuleBar(
  activeTerm: HerdrTerminal | undefined,
  tr: (en: string, zh: string) => string
): string {
  const visible = Boolean(activeTerm && isNeedsYouState(activeTerm.status?.state));
  const summary =
    (activeTerm?.status?.app ? `[${activeTerm.status.app}] ` : "") +
    ((activeTerm?.status?.msg || activeTerm?.status?.title || "").trim() ||
      tr("Agent is waiting for your approval", "AI Agent 正在等待你的交互确认"));

  return `
    <div id="needs-you-bar" class="needs-you-bar ${visible ? "visible" : ""}" role="region" aria-label="${tr("Agent Approval Needed", "Agent 待审批操作")}">
      <div class="needs-you-meta">
        <span class="needs-you-dot"></span>
        <span id="needs-you-msg" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escapeHtml(summary)}</span>
      </div>
      <div class="needs-you-actions">
        ${APPROVAL_CAPSULE_ITEMS.map((item) => {
          const cls =
            item.variant === "primary"
              ? "needs-you-btn primary"
              : item.variant === "deny"
                ? "needs-you-btn deny"
                : "needs-you-btn coda-mono";
          return `<button type="button" class="${cls}" data-approve="${item.action}">${escapeHtml(
            tr(item.labelEn, item.labelZh)
          )}</button>`;
        }).join("")}
      </div>
    </div>
  `;
}

/**
 * Encodes multi-line Prompt Composer text into a sanitized Bracketed Paste sequence + trailing CR (`\r`).
 * Strips any literal `\x1b[201~` inside `text` so pasted content cannot break out of bracketed paste early.
 */
export function encodeComposerBracketedPasteSubmit(text: string): string {
  const normalized = String(text || "")
    .replace(/\r\n/g, "\n")
    .replace(/\x1b\[201~/g, "");
  return `\x1b[200~${normalized}\x1b[201~\r`;
}

export function renderPromptComposerPanel(opts: {
  open: boolean;
  draft: string;
  isRecordingVoice: boolean;
  isTranscribingVoice: boolean;
  hasSpeechFallback: boolean;
  hasWhisperKey: boolean;
  tr: (en: string, zh: string) => string;
}): string {
  if (!opts.open) return "";
  const { tr } = opts;
  const micBtnClass = opts.isRecordingVoice ? "is-recording" : "";
  const voiceHint = opts.isTranscribingVoice
    ? tr("Transcribing…", "转写中…")
    : opts.isRecordingVoice
      ? tr("Stop Dictation", "停止听写")
      : opts.hasWhisperKey
        ? tr("Voice (Whisper)", "语音 (Whisper)")
        : opts.hasSpeechFallback
          ? tr("Voice (Live)", "语音免配置听写")
          : tr("Voice", "语音");

  return `
    <section id="prompt-composer-panel" class="composer-drawer" aria-label="${tr("Multi-line Prompt Composer", "多行 Prompt 编排器")}">
      <div class="composer-head">
        <div style="display:flex;align-items:center;gap:8px;min-width:0;">
          <strong style="font-size:12.5px;">${tr("Prompt Composer", "多行 Prompt 编排器")}</strong>
          <span class="pm-badge coda-mono" style="height:19px;font-size:10px;">Bracketed Paste</span>
        </div>
        <div style="display:flex;align-items:center;gap:6px;">
          <span class="coda-mono" style="font-size:10.5px;color:var(--text-tertiary);">${tr("⌘/Ctrl+Enter to send", "⌘/Ctrl+Enter 发送")}</span>
          <button type="button" id="btn-composer-close" class="circle-btn" style="width:24px;height:24px;" title="${tr("Collapse composer", "收起编排器")}">${svgIcon("close", 12)}</button>
        </div>
      </div>
      <textarea
        id="prompt-composer-textarea"
        class="composer-textarea"
        rows="4"
        placeholder="${tr(
          "Compose multi-line instructions, paste screenshots (Ctrl+V / ⌘V), or dictate live…",
          "编写多行复杂指令、直接粘贴剪贴板截图 (Ctrl+V / ⌘V) 或使用语音输入…"
        )}"
      >${escapeHtml(opts.draft)}</textarea>
      <div class="composer-toolbar">
        <div style="display:flex;align-items:center;gap:6px;flex-wrap:wrap;">
          <button type="button" id="btn-composer-attach" class="pm-btn pm-btn-secondary pm-btn-sm">
            ${svgIcon("paperclip", 13)} ${tr("Attach / Screenshot", "上传图片/文件")}
          </button>
          <button type="button" id="btn-composer-voice" class="pm-btn pm-btn-secondary pm-btn-sm ${micBtnClass}">
            ${svgIcon("mic", 13)} <span>${escapeHtml(voiceHint)}</span>
          </button>
          <button type="button" id="btn-composer-clear" class="pm-btn pm-btn-ghost pm-btn-sm">
            ${tr("Clear", "清空")}
          </button>
        </div>
        <button type="button" id="btn-composer-send" class="pm-btn pm-btn-primary pm-btn-sm">
          ${tr("Send Prompt", "发送多行指令")} ${svgIcon("arrowRight", 13)}
        </button>
      </div>
    </section>
  `;
}

export interface NeedsAttentionEntry {
  daemonId: string;
  daemonName: string;
  terminalAddr: string;
  terminalName: string;
  app: string | null;
  message: string;
}

export function collectNeedsAttentionItems(
  devices: readonly Device[],
  terminalsByPeer: ReadonlyMap<string, readonly HerdrTerminal[]>,
  selectedDaemonId: string | null,
  selectedTerminals: readonly HerdrTerminal[]
): NeedsAttentionEntry[] {
  const out: NeedsAttentionEntry[] = [];
  const seen = new Set<string>();

  const appendForDevice = (dev: Device, terms: readonly HerdrTerminal[]) => {
    for (const t of terms) {
      if (!t.alive || !isNeedsYouState(t.status?.state)) continue;
      const key = `${dev.id}:${t.address}`;
      if (seen.has(key)) continue;
      seen.add(key);
      out.push({
        daemonId: dev.id,
        daemonName: dev.name,
        terminalAddr: t.address,
        terminalName: t.name || t.address,
        app: t.status?.app || null,
        message: (t.status?.msg || t.status?.title || "").trim(),
      });
    }
  };

  // Put selected daemon's needs_you sessions first, followed by other online daemons.
  if (selectedDaemonId) {
    const selDev = devices.find((d) => d.id === selectedDaemonId);
    if (selDev) {
      appendForDevice(selDev, selectedTerminals);
    }
  }
  for (const dev of devices) {
    if (!dev.is_online || dev.id === selectedDaemonId) continue;
    const peerTerms = terminalsByPeer.get(dev.id) || [];
    appendForDevice(dev, peerTerms);
  }
  return out;
}

export function pickNextNeedsAttentionItem(
  items: readonly NeedsAttentionEntry[],
  currentDaemonId: string | null,
  currentTerminalAddr: string | null
): NeedsAttentionEntry | null {
  if (items.length === 0) return null;
  const curIdx = items.findIndex(
    (it) => it.daemonId === currentDaemonId && it.terminalAddr === currentTerminalAddr
  );
  if (curIdx < 0) return items[0];
  return items[(curIdx + 1) % items.length];
}

export function renderNeedsAttentionSidebarSection(
  items: readonly NeedsAttentionEntry[],
  activeDaemonId: string | null,
  activeTerminalAddr: string | null,
  tr: (en: string, zh: string) => string
): string {
  if (items.length === 0) return "";
  return `
    <div class="attention-inbox" role="region" aria-label="${tr("Needs Attention", "待处理审批")}">
      <div class="attention-inbox-head">
        <span style="display:inline-flex;align-items:center;gap:6px;font-size:11.5px;font-weight:600;color:var(--amber);">
          <span class="needs-you-dot"></span>
          <span>${tr("Needs Attention", "Needs Attention · 待审批")}</span>
          <span class="pm-badge amber coda-mono" style="height:18px;padding:0 6px;font-size:10.5px;">${items.length}</span>
        </span>
        <span class="pm-kbd" title="${tr("Jump to next Needs-You terminal (⌘J / Alt+J)", "跳转至下一个待审批终端 (⌘J / Alt+J)")}">⌘J</span>
      </div>
      <div style="display:flex;flex-direction:column;gap:5px;margin-top:7px;">
        ${items
          .map((it) => {
            const isActive =
              it.daemonId === activeDaemonId && it.terminalAddr === activeTerminalAddr;
            const subtitle =
              it.message || tr("Waiting for interactive approval", "等待交互审批确认");
            return `
          <button type="button" class="attention-item ${isActive ? "active" : ""}" data-attention-peer="${escapeHtml(it.daemonId)}" data-attention-term="${escapeHtml(it.terminalAddr)}">
            <div style="display:flex;justify-content:space-between;align-items:center;gap:6px;width:100%;">
              <span class="coda-mono" style="font-size:12px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                ${escapeHtml(it.app ? `[${it.app}] ${it.terminalName}` : it.terminalName)}
              </span>
              <span class="coda-mono" style="font-size:10px;color:var(--text-tertiary);flex-shrink:0;">${escapeHtml(it.daemonName)}</span>
            </div>
            <div style="font-size:11px;color:var(--amber);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;width:100%;text-align:left;margin-top:2px;">
              ${escapeHtml(subtitle)}
            </div>
          </button>`;
          })
          .join("")}
      </div>
    </div>
  `;
}

export function resolveVoiceCaptureStrategy(opts: {
  apiKey: string;
  isSafeBaseUrl: boolean;
  hasMediaRecorder: boolean;
  hasSpeechRecognition: boolean;
}): "whisper" | "browser_speech" | "unavailable" {
  const cleanKey = String(opts.apiKey || "").trim();
  const hasRealKey = Boolean(cleanKey && !cleanKey.startsWith("••"));
  if (hasRealKey && opts.isSafeBaseUrl && opts.hasMediaRecorder) {
    return "whisper";
  }
  if (!hasRealKey && opts.hasSpeechRecognition) {
    return "browser_speech";
  }
  if (opts.hasSpeechRecognition) {
    return "browser_speech";
  }
  return "unavailable";
}

// ============================================================================
// 4. Workspace Bootstrapper (`git clone`) & Offline Alert Webhook Helpers
// ============================================================================

export interface NotificationWebhookSettings {
  bark_url: string;
  telegram_bot_token: string;
  telegram_chat_id: string;
  feishu_webhook_url: string;
  discord_webhook_url: string;
  generic_webhook_url: string;
  notify_on_needs_you: boolean;
  notify_on_finished: boolean;
}

export function createDefaultNotificationSettings(): NotificationWebhookSettings {
  return {
    bark_url: "",
    telegram_bot_token: "",
    telegram_chat_id: "",
    feishu_webhook_url: "",
    discord_webhook_url: "",
    generic_webhook_url: "",
    notify_on_needs_you: true,
    notify_on_finished: true,
  };
}

function isAllowedWebhookEndpointUrl(raw: string): boolean {
  const trimmed = raw.trim();
  if (!trimmed) return true;
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol === "https:") return true;
    if (
      parsed.protocol === "http:" &&
      (parsed.hostname === "127.0.0.1" || parsed.hostname === "localhost")
    ) {
      return true;
    }
    return false;
  } catch {
    return false;
  }
}

export function validateNotificationWebhookSettings(
  cfg: NotificationWebhookSettings
): string | null {
  const urlFields: Array<[string, string]> = [
    ["Bark URL", cfg.bark_url],
    ["Feishu Webhook URL", cfg.feishu_webhook_url],
    ["Discord Webhook URL", cfg.discord_webhook_url],
    ["Generic Webhook URL", cfg.generic_webhook_url],
  ];
  for (const [label, val] of urlFields) {
    if (!isAllowedWebhookEndpointUrl(val)) {
      return `${label} must be a valid https:// URL`;
    }
  }
  const hasTgToken = Boolean(cfg.telegram_bot_token.trim());
  const hasTgChat = Boolean(cfg.telegram_chat_id.trim());
  if (hasTgToken !== hasTgChat) {
    return "Telegram requires both Bot Token and Chat ID";
  }
  return null;
}

export function validateSingleFolderName(rawName: string): string | null {
  const name = String(rawName || "").trim();
  if (!name) return "Folder name is required";
  if (
    name === "." ||
    name === ".." ||
    name.includes("/") ||
    name.includes("\\") ||
    name.includes("..") ||
    name.includes("\0") ||
    name.startsWith("-")
  ) {
    return "Folder name must be a single safe directory name (no /, .., or leading -)";
  }
  return null;
}

/**
 * Client-side defense-in-depth validation matching daemon's `BootstrapWorkspace` git RCE guard:
 * allows ONLY empty, `https://...`, `ssh://...`, or SCP-style `git@<host>:<path>`.
 */
export function validateGitCloneUrlClient(rawUrl: string): string | null {
  const trimmed = String(rawUrl || "").trim();
  if (!trimmed) return null;
  if (trimmed.startsWith("-") || /[\s\0\r\n]/.test(trimmed)) {
    return "Invalid Git repository URL";
  }
  if (trimmed.startsWith("https://") || trimmed.startsWith("ssh://")) {
    try {
      const parsed = new URL(trimmed);
      if (!parsed.hostname) return "Git URL is missing hostname";
      return null;
    } catch {
      return "Malformed Git repository URL";
    }
  }
  if (/^git@[A-Za-z0-9._-]+:[A-Za-z0-9._/-]+$/.test(trimmed) && !trimmed.includes("..")) {
    return null;
  }
  return "Only https://, ssh://, or git@host:org/repo URLs are allowed";
}

export function suggestFolderNameFromGitUrl(rawUrl: string): string {
  const trimmed = String(rawUrl || "").trim().replace(/\/+$/, "");
  if (!trimmed) return "";
  const lastColon = trimmed.startsWith("git@") ? trimmed.split(":").pop() || "" : trimmed;
  const base = (lastColon.split("/").pop() || "").replace(/\.git$/i, "").trim();
  if (!base || validateSingleFolderName(base) !== null) return "";
  return base;
}

export function parentAbsolutePath(absPath: string): string {
  const clean = String(absPath || "").trim().replace(/\/+$/, "");
  if (!clean || clean === "/") return "/";
  const idx = clean.lastIndexOf("/");
  if (idx <= 0) return "/";
  return clean.slice(0, idx);
}
