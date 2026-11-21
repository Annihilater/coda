import { fromB64 } from "./crypto";
import { escapeHtml } from "./vt";
import { svgIcon } from "./icons";

export const WEB_QUICK_PREVIEW_PORTS: readonly number[] = [3000, 5173, 8000, 8080, 4321];

export interface PortPreviewState {
  open: boolean;
  portInput: string;
  pathInput: string;
  viewTab: "source" | "render";
  loading: boolean;
  statusCode: number | null;
  contentType: string;
  bodyText: string;
  rawHtml: string;
  truncated: boolean;
  headers: Array<[string, string]>;
  error: string | null;
  reqId: string | null;
}

export function createInitialPortPreviewState(): PortPreviewState {
  return {
    open: false,
    portInput: "3000",
    pathInput: "/",
    viewTab: "source",
    loading: false,
    statusCode: null,
    contentType: "",
    bodyText: "",
    rawHtml: "",
    truncated: false,
    headers: [],
    error: null,
    reqId: null,
  };
}

export function parseWebPreviewPortAndPath(
  portRaw: string,
  pathRaw: string
): { port: number; path: string } | { error: string } {
  const n = Number(String(portRaw || "").trim());
  if (!Number.isInteger(n) || n < 1024 || n > 65535) {
    return { error: "Port must be between 1024 and 65535" };
  }
  if (n === 24443 || n === 24444 || n === 5432 || n === 6379) {
    return { error: `Port ${n} is reserved and blocked from preview` };
  }
  const trimmed = String(pathRaw || "").trim();
  const cleanPath = !trimmed ? "/" : trimmed.startsWith("/") ? trimmed : `/${trimmed}`;
  const qIdx = cleanPath.indexOf("?");
  const pathBeforeQuery = qIdx >= 0 ? cleanPath.slice(0, qIdx) : cleanPath;
  if (/[\r\n\0\s]/.test(cleanPath) || cleanPath.startsWith("//") || pathBeforeQuery.includes("://")) {
    return { error: "Invalid preview request path" };
  }
  return { port: n, path: cleanPath };
}

export function decodePortPreviewPayload(
  bodyB64: string,
  contentType: string,
  remoteTruncated = false
): { bodyText: string; rawHtml: string; truncated: boolean } {
  try {
    const bytes = fromB64(bodyB64 || "");
    const ct = String(contentType || "").toLowerCase();
    if (
      ct.startsWith("image/") ||
      ct.startsWith("audio/") ||
      ct.startsWith("video/") ||
      ct.includes("octet-stream")
    ) {
      return {
        bodyText: `(Binary payload: ${bytes.length} bytes, Content-Type: ${contentType})`,
        rawHtml: "",
        truncated: Boolean(remoteTruncated),
      };
    }
    const text = new TextDecoder("utf-8", { fatal: false }).decode(bytes);
    const isHtml = ct.includes("text/html");
    const overCap = text.length > 131_072;
    const truncated = Boolean(remoteTruncated) || overCap;
    if (ct.includes("json")) {
      try {
        const pretty = JSON.stringify(JSON.parse(text), null, 2);
        return {
          bodyText:
            pretty.length > 131_072 ? `${pretty.slice(0, 131_072)}\n… [truncated at 128 KB]` : pretty,
          rawHtml: "",
          truncated: truncated || pretty.length > 131_072,
        };
      } catch {
        // fall through to plain text
      }
    }
    return {
      bodyText: overCap ? `${text.slice(0, 131_072)}\n… [truncated at 128 KB]` : text,
      // Never inject plain-text truncation markers into iframe HTML `srcdoc`!
      rawHtml: isHtml ? text.slice(0, 524_288) : "",
      truncated,
    };
  } catch {
    return { bodyText: "", rawHtml: "", truncated: Boolean(remoteTruncated) };
  }
}

export function decodePortPreviewBody(bodyB64: string, contentType: string): string {
  return decodePortPreviewPayload(bodyB64, contentType, false).bodyText;
}

export function detectClientRenderedSpaHtml(html: string): boolean {
  if (!html) return false;
  const sample = html.slice(0, 32_768);
  if (
    sample.includes("/@vite/client") ||
    sample.includes("/_next/static/") ||
    sample.includes("__NEXT_DATA__")
  ) {
    return true;
  }
  const hasModuleScript = /<script\b[^>]*type=["']?module["']?/i.test(sample);
  const hasEmptyMountRoot = /<div\s+id=["']?(root|app|__next)["']?\s*><\/div>/i.test(sample);
  return hasModuleScript && hasEmptyMountRoot;
}

export function renderPortPreviewModal(
  st: PortPreviewState,
  hostName: string,
  tr: (en: string, zh: string) => string
): string {
  if (!st.open) return "";
  const isHtml = st.contentType.toLowerCase().includes("text/html");
  const htmlForIframe = st.rawHtml || st.bodyText.replace(/\n… \[truncated at 128 KB\]$/, "");
  const isSpaHtml = isHtml && detectClientRenderedSpaHtml(htmlForIframe);
  const redirectHeader = (st.headers || []).find(([k]) => k.toLowerCase() === "location")?.[1]?.trim() || "";
  let followableRedirectPath = "";
  if (redirectHeader) {
    if (redirectHeader.startsWith("/") && !redirectHeader.startsWith("//")) {
      followableRedirectPath = redirectHeader;
    } else {
      try {
        const parsedLoc = new URL(redirectHeader);
        if (parsedLoc.hostname === "127.0.0.1" || parsedLoc.hostname === "localhost") {
          followableRedirectPath = `${parsedLoc.pathname || "/"}${parsedLoc.search || ""}`;
        }
      } catch {
        // non-URL relative path
      }
    }
  }
  const statusBadgeClass =
    st.statusCode === null
      ? ""
      : st.statusCode >= 200 && st.statusCode < 300
        ? "green"
        : st.statusCode >= 300 && st.statusCode < 400
          ? "blue"
          : "";

  return `
    <div class="modal-mask" id="port-preview-mask">
      <div class="modal-card" style="width:860px;max-width:96vw;">
        <div style="display:flex;justify-content:space-between;align-items:center;gap:10px;margin-bottom:10px;">
          <div style="display:flex;align-items:center;gap:8px;min-width:0;">
            ${svgIcon("globe", 15)}
            <strong style="font-size:15px;">${tr(`Remote Port Preview · ${escapeHtml(hostName)}`, `远端本地端口加密预览 · ${escapeHtml(hostName)}`)}</strong>
          </div>
          <button id="btn-close-port-preview" type="button" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
        </div>

        <form id="port-preview-form" style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:8px;">
          <span class="coda-mono" style="font-size:12px;color:var(--text-tertiary);">http://127.0.0.1:</span>
          <input id="pp-port-input" class="pm-input coda-mono" style="margin:0;width:88px;height:32px;padding:0 8px;" value="${escapeHtml(st.portInput)}" placeholder="3000" autocomplete="off" />
          <input id="pp-path-input" class="pm-input coda-mono" style="margin:0;flex:1;min-width:140px;height:32px;padding:0 8px;" value="${escapeHtml(st.pathInput)}" placeholder="/ or /api/health" autocomplete="off" />
          <button type="submit" class="pm-btn pm-btn-primary pm-btn-sm" style="height:32px;">
            ${st.loading ? tr("Fetching…", "请求中…") : tr("Fetch over E2EE", "加密探测")}
          </button>
        </form>

        <div class="quick-chip-strip" style="margin-bottom:10px;">
          ${WEB_QUICK_PREVIEW_PORTS.map(
            (p) =>
              `<button type="button" class="quick-chip-btn" data-pp-port="${p}" style="${
                st.portInput.trim() === String(p) ? "border-color:var(--signal-blue);color:var(--text-primary);" : ""
              }">:${p}</button>`
          ).join("")}
        </div>

        ${
          st.error
            ? `<div style="padding:8px 10px;border-radius:8px;background:var(--rose-bg);border:1px solid var(--rose-bd);color:var(--rose);font-size:12px;margin-bottom:10px;">${escapeHtml(st.error)}</div>`
            : ""
        }

        ${
          st.statusCode !== null
            ? `<div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:8px;">
                <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
                  <span class="pm-badge ${statusBadgeClass} coda-mono">HTTP ${st.statusCode}</span>
                  <span class="coda-mono" style="font-size:11.5px;color:var(--text-tertiary);">${escapeHtml(st.contentType)}</span>
                  ${st.truncated ? `<span class="pm-badge amber">${tr("Truncated", "已截断")}</span>` : ""}
                  ${
                    redirectHeader
                      ? `<span class="coda-mono" style="font-size:11px;color:var(--signal-blue);">Location: ${escapeHtml(redirectHeader)}</span>`
                      : ""
                  }
                  ${
                    followableRedirectPath
                      ? `<button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" data-pp-follow-path="${escapeHtml(followableRedirectPath)}" style="height:24px;padding:0 8px;font-size:11px;">${tr("Follow Redirect", "跟随重定向")} &rarr;</button>`
                      : ""
                  }
                </div>
                ${
                  isHtml
                    ? `<div style="display:flex;gap:6px;">
                        <button type="button" class="pm-btn ${st.viewTab === "source" ? "pm-btn-primary" : "pm-btn-secondary"} pm-btn-sm" data-pp-tab="source">${tr("Source", "源码")}</button>
                        <button type="button" class="pm-btn ${st.viewTab === "render" ? "pm-btn-primary" : "pm-btn-secondary"} pm-btn-sm" data-pp-tab="render">${tr("Sandboxed HTML", "沙箱渲染")}</button>
                      </div>`
                    : ""
                }
              </div>
              ${
                isSpaHtml && st.viewTab === "render"
                  ? `<div style="padding:7px 10px;border-radius:8px;background:var(--signal-blue-bg);border:1px solid var(--signal-blue-bd);color:var(--text-secondary);font-size:11.5px;margin-bottom:8px;">
                      ${tr(
                        "Client-rendered SPA detected (Vite / Next.js / React): zero-trust iframe sandbox blocks JavaScript & external module bundles. Inspect HTML under Source, or open a live loopback tunnel in Coda Desktop for full interactive HMR.",
                        "检测到前端单页应用 (Vite / Next.js / React)：零信任沙箱已隔离脚本执行与子资源请求。可切回「源码」检查 HTML，或在 Coda 桌面端开启本地隧道体验完整交互。"
                      )}
                    </div>`
                  : ""
              }
              <div style="border:1px solid var(--border-primary);border-radius:9px;background:var(--bg-elevated);height:360px;max-height:58dvh;overflow:auto;display:flex;flex-direction:column;">
                ${
                  isHtml && st.viewTab === "render"
                    ? `<iframe sandbox="" referrerpolicy="no-referrer" srcdoc="${escapeHtml(htmlForIframe)}" style="border:none;width:100%;height:100%;background:#ffffff;border-radius:8px;" title="Sandboxed Remote Preview"></iframe>`
                    : `<pre class="coda-mono" style="margin:0;padding:12px;font-size:11.5px;line-height:1.5;white-space:pre-wrap;word-break:break-word;">${escapeHtml(st.bodyText || tr("(Empty response body)", "（响应体为空）"))}</pre>`
                }
              </div>`
            : `<div style="padding:20px;border:1px dashed var(--border-primary);border-radius:9px;font-size:12px;color:var(--text-tertiary);text-align:center;">
                ${tr("Enter any unprivileged port (e.g. :3000, :5173, :8000) to inspect a remote dev server over E2EE without opening firewall ports.", "输入远端开发机监听的本地端口（如 :3000、:5173、:8000），无需开放云安全组即可通过端到端加密通道查看响应。")}
              </div>`
        }
      </div>
    </div>
  `;
}
