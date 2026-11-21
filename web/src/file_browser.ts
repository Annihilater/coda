import { escapeHtml } from "./vt";
import { svgIcon } from "./icons";

export interface RemoteFileEntry {
  name: string;
  rel_path: string;
  is_dir: boolean;
  size_bytes: number;
}

export interface RemoteFileBrowserState {
  open: boolean;
  rootPath: string;
  subPath: string;
  entries: RemoteFileEntry[];
  selectedRelPath: string | null;
  previewContent: string;
  previewTruncated: boolean;
  loading: boolean;
  error: string | null;
  dirReqId: string | null;
  previewReqId: string | null;
}

export function createInitialFileBrowserState(): RemoteFileBrowserState {
  return {
    open: false,
    rootPath: "",
    subPath: "",
    entries: [],
    selectedRelPath: null,
    previewContent: "",
    previewTruncated: false,
    loading: false,
    error: null,
    dirReqId: null,
    previewReqId: null,
  };
}

export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function parentSubPath(subPath: string): string {
  const clean = subPath.replace(/^\/+|\/+$/g, "");
  if (!clean) return "";
  const idx = clean.lastIndexOf("/");
  return idx < 0 ? "" : clean.slice(0, idx);
}

export function joinRootAndRel(rootPath: string, relPath: string): string {
  const root = rootPath.replace(/\/+$/, "") || "/";
  const rel = relPath.replace(/^\/+/, "");
  if (!rel) return root;
  return root === "/" ? `/${rel}` : `${root}/${rel}`;
}

export function renderRemoteFileBrowserModal(
  st: RemoteFileBrowserState,
  roots: string[],
  tr: (en: string, zh: string) => string
): string {
  if (!st.open) return "";
  const uniqueRoots = Array.from(new Set([st.rootPath, ...roots].filter(Boolean)));
  const canGoUp = Boolean(st.subPath.replace(/^\/+|\/+$/g, ""));

  return `
    <div class="modal-mask" id="file-browser-mask">
      <div class="modal-card" style="width:860px;max-width:96vw;">
        <div style="display:flex;justify-content:space-between;align-items:center;gap:10px;margin-bottom:10px;">
          <div style="display:flex;align-items:center;gap:8px;min-width:0;">
            ${svgIcon("layers", 15)}
            <strong style="font-size:15px;">${tr("Remote Workspace Files (E2EE)", "远端工作区文件浏览器 (端到端加密)")}</strong>
          </div>
          <button id="btn-close-file-browser" type="button" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
        </div>

        <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:10px;">
          ${
            uniqueRoots.length > 1
              ? `<select id="fb-root-select" class="pm-input coda-mono" style="margin:0;width:auto;max-width:340px;height:30px;padding:0 8px;font-size:11.5px;">
                  ${uniqueRoots
                    .map(
                      (r) =>
                        `<option value="${escapeHtml(r)}" ${r === st.rootPath ? "selected" : ""}>${escapeHtml(r)}</option>`
                    )
                    .join("")}
                </select>`
              : `<code class="coda-mono" style="font-size:11.5px;">${escapeHtml(st.rootPath || "~")}</code>`
          }
          ${
            canGoUp
              ? `<button type="button" id="fb-up-btn" class="pm-btn pm-btn-secondary pm-btn-sm">&uarr; ${tr("Up", "上级目录")}</button>`
              : ""
          }
          <span class="coda-mono" style="font-size:11.5px;color:var(--text-tertiary);">/${escapeHtml(st.subPath)}</span>
        </div>

        ${
          st.error
            ? `<div style="padding:8px 10px;border-radius:8px;background:var(--rose-bg);border:1px solid var(--rose-bd);color:var(--rose);font-size:12px;margin-bottom:10px;">${escapeHtml(st.error)}</div>`
            : ""
        }

        <div class="fb-split-grid">
          <div class="fb-list-pane">
            ${
              st.loading && st.entries.length === 0
                ? `<div style="padding:14px;font-size:12px;color:var(--text-tertiary);">${tr("Loading directory…", "正在读取目录…")}</div>`
                : st.entries.length === 0
                  ? `<div style="padding:14px;font-size:12px;color:var(--text-tertiary);">${tr("Empty directory", "目录为空")}</div>`
                  : st.entries
                      .map(
                        (entry) => `
                  <button type="button" class="fb-entry-row ${st.selectedRelPath === entry.rel_path ? "active" : ""}" data-fb-rel="${escapeHtml(entry.rel_path)}" data-fb-dir="${entry.is_dir ? "1" : "0"}">
                    <span class="coda-mono" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${entry.is_dir ? "📁" : "📄"} ${escapeHtml(entry.name)}${entry.is_dir ? "/" : ""}</span>
                    <span class="coda-mono" style="font-size:10.5px;color:var(--text-tertiary);flex-shrink:0;">${entry.is_dir ? "" : formatFileSize(entry.size_bytes)}</span>
                  </button>`
                      )
                      .join("")
            }
          </div>
          <div class="fb-preview-pane">
            ${
              st.selectedRelPath
                ? `<div style="display:flex;justify-content:space-between;align-items:center;padding:6px 10px;border-bottom:1px solid var(--border-primary);font-size:11.5px;">
                    <span class="coda-mono" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${escapeHtml(st.selectedRelPath)}</span>
                    ${st.previewTruncated ? `<span class="pm-badge amber">${tr("Truncated 64KB", "已截断 64KB")}</span>` : ""}
                  </div>
                  <pre class="coda-mono" style="margin:0;padding:10px;font-size:11.5px;line-height:1.5;overflow:auto;flex:1;white-space:pre-wrap;word-break:break-word;">${escapeHtml(
                    st.loading && !st.previewContent
                      ? tr("Loading file preview…", "正在读取文件预览…")
                      : st.previewContent || tr("(Empty file)", "（空文件）")
                  )}</pre>`
                : `<div style="padding:18px;font-size:12px;color:var(--text-tertiary);">${tr("Select a file on the left to inspect its contents over E2EE.", "点击左侧文件即可通过端到端加密通道预览源码内容。")}</div>`
            }
          </div>
        </div>
      </div>
    </div>
  `;
}
