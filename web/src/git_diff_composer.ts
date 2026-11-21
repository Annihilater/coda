import { escapeHtml } from "./vt";
import { svgIcon } from "./icons";
import type { Device, HerdrTerminal, PromptSnippet } from "./types";
import { isNeedsYouState } from "./notify";
import { sanitizeBracketedPasteText } from "./terminal_keys";
import { isCustomPromptSnippet } from "./friend_ux";

// ============================================================================
// 1. Remote E2EE Interactive Git Diff Parser, Filter & Modal Renderer
// ============================================================================

export type GitChangeBadge = "M" | "A" | "D" | "R" | "C" | "T" | "U" | "??";

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
  isGitRepo: boolean;
  repoRoot: string | null;
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
    isGitRepo: true,
    repoRoot: null,
    loading: false,
    error: null,
    reqId: null,
  };
}

export function resolveGitDiffFileRoot(
  repoRoot: string | null | undefined,
  fallbackCwd: string
): string {
  const cleanRepo = String(repoRoot || "").trim();
  if (cleanRepo) return cleanRepo;
  const cleanCwd = String(fallbackCwd || "").trim();
  return cleanCwd || ".";
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
  if (clean.includes("T")) return "T";
  return "M";
}

/**
 * Parses `git status --short` / `--porcelain` lines (e.g. ` M src/app.ts`, `A  new.ts`,
 * ` T symlink`, `?? untracked.png`, `R  old.ts -> new.ts`) into structured file descriptors.
 * Only splits `" -> "` on `R`/`C` statuses so regular filenames containing `" -> "` stay intact.
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
    if (line.length > 3 && /^[ MADRCTU?!]{2}$/.test(line.slice(0, 2)) && line[2] === " ") {
      rawCode = line.slice(0, 2);
      restPath = line.slice(3);
    } else {
      const m = line.match(/^\s*([MADRCTU?!]{1,2})\s+(.+)$/);
      if (!m) continue;
      rawCode = m[1];
      restPath = m[2];
    }

    let origPath: string | null = null;
    let finalPath = "";
    const isRenameOrCopy = /[RC]/i.test(rawCode);
    const arrowIdx = isRenameOrCopy ? restPath.indexOf(" -> ") : -1;
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
    case "T":
      return "amber";
    case "??":
      return "";
  }
}

function extractDiffHeaderTargetPath(headerLine: string): string | null {
  if (!headerLine.startsWith("diff --git ")) return null;
  const m = headerLine.match(/^diff --git "?a\/(.+?)"? "?b\/(.+?)"?$/);
  return m?.[2] ? unquoteGitPath(m[2]) : null;
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
  const canOpenSelectedFile = Boolean(
    selectedEntry &&
      selectedEntry.status !== "D" &&
      !selectedEntry.path.endsWith("/")
  );
  const deletedPaths = new Set(
    st.files.filter((f) => f.status === "D").map((f) => f.path)
  );
  const filteredDiff = filterUnifiedDiffByFile(
    st.diffUnified,
    st.selectedFile,
    selectedEntry?.origPath
  );
  const fullStats = parseUnifiedDiffLines(st.diffUnified);
  const activeParsed = st.selectedFile ? parseUnifiedDiffLines(filteredDiff) : fullStats;
  const branchLabel = !st.isGitRepo
    ? tr("Not a Git Repo", "非 Git 仓库")
    : st.branch.trim() || "HEAD";
  const branchBadgeCls = !st.isGitRepo ? "red" : "blue";

  return `
    <div class="modal-mask" id="git-diff-mask">
      <div class="modal-card" style="width:940px;max-width:96vw;">
        <div style="display:flex;justify-content:space-between;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:10px;">
          <div style="display:flex;align-items:center;gap:8px;min-width:0;flex-wrap:wrap;">
            ${svgIcon("gitBranch", 15)}
            <strong style="font-size:15px;">${tr("Remote Git Changes / Diff (E2EE)", "远端 Git 变更与 Diff 审查 (端到端加密)")}</strong>
            <span class="pm-badge ${branchBadgeCls} coda-mono">${escapeHtml(branchLabel)}</span>
            <span class="pm-badge coda-mono"><span style="color:var(--emerald);">+${fullStats.added}</span>&nbsp;<span style="color:var(--rose);">-${fullStats.deleted}</span></span>
          </div>
          <div style="display:flex;align-items:center;gap:6px;">
            ${
              canOpenSelectedFile && selectedEntry
                ? `<button id="btn-gd-open-selected" type="button" class="pm-btn pm-btn-secondary pm-btn-sm" data-gd-open-file="${escapeHtml(selectedEntry.path)}">
                    ${tr("Open File", "打开文件")} &rarr;
                  </button>`
                : ""
            }
            <button id="btn-refresh-git-diff" type="button" class="pm-btn pm-btn-secondary pm-btn-sm">
              ${st.loading ? tr("Refreshing…", "刷新中…") : tr("Refresh", "刷新")}
            </button>
            <button id="btn-close-git-diff" type="button" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
          </div>
        </div>

        <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;flex-wrap:wrap;margin-bottom:10px;">
          <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
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
            ${
              st.repoRoot && st.repoRoot !== st.cwd
                ? `<span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">(${tr("root:", "仓库根：")} ${escapeHtml(st.repoRoot)})</span>`
                : ""
            }
          </div>
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
                ? `<div style="padding:24px;text-align:center;font-size:12px;color:var(--text-tertiary);display:flex;flex-direction:column;align-items:center;gap:10px;">
                    <div>${
                      st.error || !st.isGitRepo
                        ? escapeHtml(
                            st.error ||
                              tr(
                                "Selected directory is not a Git repository or could not be inspected.",
                                "所选目录不是 Git 仓库或无法读取 Git 状态。"
                              )
                          )
                        : selectedEntry?.status === "??"
                          ? tr(
                              `Untracked file "${escapeHtml(selectedEntry.path)}" has no text diff hunks — click Open File to inspect full source.`,
                              `未跟踪文件「${escapeHtml(selectedEntry.path)}」无文本对比块，点击下方按钮即可直接预览完整源码。`
                            )
                          : st.files.length === 0
                            ? tr("Working tree clean — no uncommitted Git changes.", "工作区干净，当前无未提交的 Git 变更。")
                            : tr(
                                "Changed entries have no text diff hunks (e.g. binary, mode-only, or directory) — select a file above to open it.",
                                "当前变更项无文本 Diff 行（如二进制文件、权限变更或目录），可点击上方文件直接打开预览。"
                              )
                    }</div>
                    ${
                      canOpenSelectedFile && selectedEntry
                        ? `<button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" data-gd-open-file="${escapeHtml(selectedEntry.path)}">${tr("Open File", "打开文件")} &rarr;</button>`
                        : ""
                    }
                  </div>`
                : activeParsed.lines
                    .map((ln) => {
                      const headerPath =
                        ln.kind === "meta" ? extractDiffHeaderTargetPath(ln.text) : null;
                      const showJump =
                        Boolean(headerPath) &&
                        !deletedPaths.has(headerPath!) &&
                        !headerPath!.endsWith("/");
                      return `
                <div class="gd-row gd-${ln.kind}">
                  <span class="gd-ln">${ln.oldLine !== null ? ln.oldLine : ""}</span>
                  <span class="gd-ln">${ln.newLine !== null ? ln.newLine : ""}</span>
                  <span class="gd-code" style="display:flex;align-items:center;justify-content:space-between;gap:10px;"><span>${escapeHtml(ln.text)}</span>${
                        showJump
                          ? `<button type="button" class="pm-btn pm-btn-ghost pm-btn-sm" data-gd-open-file="${escapeHtml(headerPath!)}" style="height:20px;padding:0 7px;font-size:10.5px;">${tr("Open File", "打开文件")} &rarr;</button>`
                          : ""
                      }</span>
                </div>`;
                    })
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

export function resolveClipboardImageExtension(
  mimeType?: string | null,
  originalName?: string | null
): string {
  const cleanMime = String(mimeType || "")
    .split(";")[0]
    .trim()
    .toLowerCase();
  switch (cleanMime) {
    case "image/jpeg":
    case "image/jpg":
      return "jpg";
    case "image/webp":
      return "webp";
    case "image/gif":
      return "gif";
    case "image/png":
      return "png";
    case "image/svg+xml":
      return "svg";
    case "image/bmp":
      return "bmp";
    case "image/avif":
      return "avif";
    case "image/heic":
      return "heic";
    case "image/heif":
      return "heif";
  }

  const nameMatch = String(originalName || "")
    .trim()
    .match(/\.([A-Za-z0-9]+)$/);
  if (nameMatch) {
    const ext = nameMatch[1].toLowerCase();
    if (ext === "jpeg" || ext === "jpg") return "jpg";
    if (
      ext === "webp" ||
      ext === "gif" ||
      ext === "png" ||
      ext === "svg" ||
      ext === "bmp" ||
      ext === "avif" ||
      ext === "heic" ||
      ext === "heif"
    ) {
      return ext;
    }
  }
  return "png";
}

/**
 * Generates deterministic `clipboard-<timestamp>.<ext>` filenames matching actual image MIME bytes.
 */
export function buildClipboardImageFilename(
  nowMs = Date.now(),
  seq = 0,
  mimeType?: string | null,
  originalName?: string | null
): string {
  const ext = resolveClipboardImageExtension(mimeType, originalName);
  return seq > 0 ? `clipboard-${nowMs}-${seq}.${ext}` : `clipboard-${nowMs}.${ext}`;
}

export interface ClipboardDataLike {
  getData?: (format: string) => string;
  items?: ArrayLike<{
    kind?: string;
    type?: string;
    getAsFile?: () => File | Blob | null;
  }> | null;
  files?: ArrayLike<File | Blob> | null;
}

/**
 * Extracts pasted image blobs ONLY when `text/plain` is empty, preventing rich-text apps
 * (Excel, Numbers, Word, Keynote, macOS Notes) that place BOTH `text/plain` and `image/png`
 * on the clipboard from losing copied text.
 */
export function extractPureClipboardImages(
  clipboardData: ClipboardDataLike | null | undefined
): Array<File | Blob> {
  if (!clipboardData) return [];
  const plainText =
    String(clipboardData.getData?.("text/plain") || "") ||
    String(clipboardData.getData?.("text") || "");
  if (plainText.length > 0) {
    return [];
  }

  const out: Array<File | Blob> = [];
  const items = clipboardData.items;
  if (items && items.length > 0) {
    for (let i = 0; i < items.length; i += 1) {
      const item = items[i];
      if (item && item.kind === "file" && String(item.type || "").toLowerCase().startsWith("image/")) {
        const blob = item.getAsFile?.();
        if (blob) out.push(blob);
      }
    }
    if (out.length > 0) return out;
  }

  const files = clipboardData.files;
  if (files && files.length > 0) {
    for (let i = 0; i < files.length; i += 1) {
      const f = files[i];
      if (f && String(f.type || "").toLowerCase().startsWith("image/")) {
        out.push(f);
      }
    }
  }
  return out;
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

const KNOWN_TUI_AGENT_NAMES = new Set([
  "claude",
  "claude-code",
  "codex",
  "grok",
  "agy",
  "antigravity",
  "gemini",
  "aider",
  "opencode",
  "goose",
]);

export function isKnownTuiAgentTerminal(term?: HerdrTerminal | null): boolean {
  if (!term) return false;
  const app = String(term.status?.app || "")
    .trim()
    .toLowerCase();
  if (app && KNOWN_TUI_AGENT_NAMES.has(app)) return true;

  const argv0 = String(term.argv?.[0] || "")
    .trim()
    .split(/[\\/]/)
    .pop()
    ?.toLowerCase();
  if (argv0 && KNOWN_TUI_AGENT_NAMES.has(argv0)) return true;

  const rawName = String(term.name || "")
    .trim()
    .toLowerCase();
  if (rawName && KNOWN_TUI_AGENT_NAMES.has(rawName)) return true;
  return false;
}

export function resolveEffectiveBracketedPaste(opts: {
  bracketedPaste?: boolean;
  bracketedPasteExplicit?: boolean | null;
  term?: HerdrTerminal | null;
}): boolean {
  // If `\x1b[?2004l` was explicitly emitted, strictly honor disabled bracketed paste.
  if (opts.bracketedPasteExplicit === false) return false;
  if (opts.bracketedPaste) return true;
  if (isKnownTuiAgentTerminal(opts.term)) return true;
  return false;
}

/**
 * Encodes multi-line Prompt Composer text into a sanitized Bracketed Paste sequence + trailing CR (`\r`).
 * Iteratively strips any `\x1b[200~` / `\x1b[201~` markers so nested `\x1b[\x1b[201~201~` cannot break out.
 */
export function encodeComposerBracketedPasteSubmit(text: string): string {
  return `\x1b[200~${sanitizeBracketedPasteText(text)}\x1b[201~\r`;
}

/**
 * Encodes Prompt Composer submission: wraps in `?2004h` bracketed paste when enabled,
 * or falls back to clean normalized `\n` + trailing `\r` for plain shells/REPLs (`/bin/sh`, `python`, `psql`).
 */
export function encodeComposerSubmit(text: string, useBracketedPaste: boolean): string {
  if (useBracketedPaste) {
    return encodeComposerBracketedPasteSubmit(text);
  }
  const normalized = String(text || "")
    .replace(/\r\n/g, "\n")
    .replace(/[\r\n]+$/, "");
  return `${normalized}\r`;
}

export function renderPromptComposerPanel(opts: {
  open: boolean;
  draft: string;
  isRecordingVoice: boolean;
  isTranscribingVoice: boolean;
  hasSpeechFallback: boolean;
  hasWhisperKey: boolean;
  useBracketedPaste?: boolean;
  snippets?: readonly PromptSnippet[];
  savingSnippetOpen?: boolean;
  savingSnippetTitle?: string;
  savingSnippetBusy?: boolean;
  tr: (en: string, zh: string) => string;
}): string {
  if (!opts.open) return "";
  const { tr } = opts;
  const micBtnClass = opts.isRecordingVoice ? "is-recording" : "";
  const modeBadge =
    opts.useBracketedPaste === false
      ? tr("Direct Stdin", "标准输入")
      : "Bracketed Paste";
  const voiceHint = opts.isTranscribingVoice
    ? tr("Transcribing…", "转写中…")
    : opts.isRecordingVoice
      ? tr("Stop Dictation", "停止听写")
      : opts.hasWhisperKey
        ? tr("Voice (Whisper)", "语音 (Whisper)")
        : opts.hasSpeechFallback
          ? tr("Voice (Live)", "语音免配置听写")
          : tr("Voice", "语音");
  const snippets = opts.snippets || [];

  return `
    <section id="prompt-composer-panel" class="composer-drawer" aria-label="${tr("Multi-line Prompt Composer", "多行 Prompt 编排器")}">
      <div class="composer-head">
        <div style="display:flex;align-items:center;gap:8px;min-width:0;">
          <strong style="font-size:12.5px;">${tr("Prompt Composer", "多行 Prompt 编排器")}</strong>
          <span class="pm-badge coda-mono" style="height:19px;font-size:10px;">${escapeHtml(modeBadge)}</span>
        </div>
        <div style="display:flex;align-items:center;gap:6px;">
          <span class="coda-mono" style="font-size:10.5px;color:var(--text-tertiary);">${tr("⌘/Ctrl+Enter to send", "⌘/Ctrl+Enter 发送")}</span>
          <button type="button" id="btn-composer-close" class="circle-btn" style="width:24px;height:24px;" title="${tr("Collapse composer", "收起编排器")}">${svgIcon("close", 12)}</button>
        </div>
      </div>
      <div class="composer-snippet-strip" role="toolbar" aria-label="${tr("Saved Prompt Snippets", "跨端常用 Prompt 片段")}">
        ${snippets
          .map((s) => {
            const custom = isCustomPromptSnippet(s);
            return `
          <span class="snippet-pill-group">
            <button type="button" class="snippet-pill-btn" data-snippet-insert="${escapeHtml(s.id)}" title="${escapeHtml(s.prompt)}">
              ${escapeHtml(s.title)}
            </button>${
              custom
                ? `<button type="button" class="snippet-pill-del" data-snippet-delete="${escapeHtml(s.id)}" title="${tr("Delete custom snippet", "删除自定义片段")}" aria-label="${tr("Delete snippet", "删除片段")}">&times;</button>`
                : ""
            }
          </span>`;
          })
          .join("")}
        <button type="button" id="btn-composer-save-snippet-toggle" class="snippet-save-trigger">
          + ${tr("Save Snippet", "保存常用片段")}
        </button>
      </div>
      ${
        opts.savingSnippetOpen
          ? `<div class="composer-save-bar" id="composer-save-snippet-bar">
              <input
                id="composer-snippet-title-input"
                class="pm-input"
                style="margin:0;height:28px;font-size:12px;flex:1;min-width:160px;"
                maxlength="60"
                value="${escapeHtml(opts.savingSnippetTitle || "")}"
                placeholder="${tr("Snippet title (max 60 chars)…", "输入片段标题（最多 60 字）…")}"
              />
              <button type="button" id="btn-composer-save-snippet-confirm" class="pm-btn pm-btn-primary pm-btn-sm" style="height:28px;" ${opts.savingSnippetBusy ? "disabled" : ""}>
                ${opts.savingSnippetBusy ? tr("Saving…", "保存中…") : tr("Save", "确认保存")}
              </button>
              <button type="button" id="btn-composer-save-snippet-cancel" class="pm-btn pm-btn-ghost pm-btn-sm" style="height:28px;">
                ${tr("Cancel", "取消")}
              </button>
            </div>`
          : ""
      }
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
        <span class="pm-kbd" title="${tr("Jump to next Needs-You terminal (⌘J / Ctrl+Shift+J)", "跳转至下一个待审批终端 (⌘J / Ctrl+Shift+J)")}">⌘J</span>
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
  hasOnlineDaemon?: boolean;
}): "whisper" | "daemon_whisper" | "browser_speech" | "unavailable" {
  const cleanKey = String(opts.apiKey || "").trim();
  const isRedactedHostKey = cleanKey.startsWith("••");
  const hasRealKey = Boolean(cleanKey && !isRedactedHostKey);
  if (hasRealKey && opts.isSafeBaseUrl && opts.hasMediaRecorder) {
    return "whisper";
  }
  if (isRedactedHostKey && opts.hasMediaRecorder && (opts.hasOnlineDaemon ?? true)) {
    return "daemon_whisper";
  }
  if (opts.hasSpeechRecognition) {
    return "browser_speech";
  }
  return "unavailable";
}

export interface SpeechResultItemLike {
  isFinal?: boolean;
  0?: { transcript?: string };
  length?: number;
}

/**
 * Rebuilds the complete SpeechRecognition transcript from `ev.results[0 .. length - 1]` on each event
 * without accumulating across events, preventing exponential duplication on Safari/iOS WebKit where
 * `ev.resultIndex` stays `0` across subsequent interim/final ticks.
 */
export function rebuildSpeechRecognitionTranscript(
  results: ArrayLike<SpeechResultItemLike> | null | undefined
): { finalText: string; interimText: string; combined: string } {
  if (!results || typeof results.length !== "number") {
    return { finalText: "", interimText: "", combined: "" };
  }
  const finals: string[] = [];
  const interims: string[] = [];
  for (let i = 0; i < results.length; i += 1) {
    const item = results[i];
    const chunk = String(item?.[0]?.transcript || "").trim();
    if (!chunk) continue;
    if (item?.isFinal) {
      finals.push(chunk);
    } else {
      interims.push(chunk);
    }
  }
  const finalText = finals.join(" ").trim();
  const interimText = interims.join(" ").trim();
  const combined = `${finalText} ${interimText}`.trim();
  return { finalText, interimText, combined };
}

export function stepPaletteSelectionIndex(
  currentIdx: number,
  totalCount: number,
  delta: 1 | -1
): number {
  if (totalCount <= 0) return 0;
  const clamped = Math.max(0, Math.min(totalCount - 1, currentIdx | 0));
  return ((clamped + delta) % totalCount + totalCount) % totalCount;
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

  let pathPart = "";
  if (/^(https?|ssh):\/\//i.test(trimmed)) {
    const schemeEnd = trimmed.indexOf("://");
    const afterScheme = trimmed.slice(schemeEnd + 3);
    const slashIdx = afterScheme.indexOf("/");
    if (slashIdx < 0) return "";
    pathPart = afterScheme.slice(slashIdx + 1);
  } else if (trimmed.startsWith("git@")) {
    const colonIdx = trimmed.indexOf(":");
    if (colonIdx < 0) return "";
    pathPart = trimmed.slice(colonIdx + 1);
  } else {
    // Incomplete scheme prefix while user is still typing "h", "ht", "https:", "git", etc.
    if (!trimmed.includes("/")) return "";
    pathPart = trimmed;
  }

  const base = (pathPart.split("/").pop() || "").replace(/\.git$/i, "").trim();
  if (!base || validateSingleFolderName(base) !== null) return "";
  return base;
}

export function computeBootstrapAutoFolder(
  gitUrl: string,
  currentFolder: string,
  manuallyEdited: boolean
): string {
  if (manuallyEdited) return currentFolder;
  return suggestFolderNameFromGitUrl(gitUrl);
}

export function parentAbsolutePath(absPath: string): string {
  const clean = String(absPath || "").trim().replace(/\/+$/, "");
  if (!clean || clean === "/") return "/";
  if (clean.startsWith("~/")) {
    const idx = clean.lastIndexOf("/");
    return idx <= 1 ? "~" : clean.slice(0, idx);
  }
  const idx = clean.lastIndexOf("/");
  if (idx <= 0) return "/";
  return clean.slice(0, idx);
}

export function resolveSpawnPickerInitialRoot(opts: {
  rootOverride?: string;
  spawnCwd?: string;
  activeTermCwd?: string;
  firstRepoRoot?: string;
}): string {
  return (
    String(opts.rootOverride || "").trim() ||
    String(opts.spawnCwd || "").trim() ||
    String(opts.activeTermCwd || "").trim() ||
    String(opts.firstRepoRoot || "").trim() ||
    "~"
  );
}

export function canStepUpSpawnDirectory(rootPath: string, subPath: string): boolean {
  if (Boolean(String(subPath || "").replace(/^\/+|\/+$/g, ""))) return true;
  const cleanRoot = String(rootPath || "").trim().replace(/\/+$/, "");
  if (cleanRoot.startsWith("/") && cleanRoot !== "") return true;
  if (cleanRoot.startsWith("~/") && cleanRoot !== "~") return true;
  return false;
}

export function collectPrunableVtKeys(
  vtKeys: Iterable<string>,
  peerId: string,
  keepAddrs: ReadonlySet<string>
): string[] {
  const prefix = `${peerId}:`;
  const stale: string[] = [];
  for (const key of vtKeys) {
    if (!key.startsWith(prefix)) continue;
    const addr = key.slice(prefix.length);
    if (!keepAddrs.has(addr)) {
      stale.push(key);
    }
  }
  return stale;
}
