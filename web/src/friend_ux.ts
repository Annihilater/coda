import { escapeHtml } from "./vt";
import { svgIcon } from "./icons";
import type {
  AdminUserSummary,
  AppRoute,
  PromptSnippet,
  TransportMode,
  VapidPublicKeyInfo,
} from "./types";

export {
  type ParsedSwPushPayload,
  normalizeSwNotificationTargetUrl,
  parseSwPushPayload,
  shouldSwCacheStaticAsset,
} from "./sw_shared";

// ============================================================================
// 1. W3C VAPID Web Push & iOS PWA Helpers
// ============================================================================

export const IOS_PWA_LOCK_SCREEN_PUSH_TIP_EN =
  "Tip: On iPhone, tap Share → Add to Home Screen first to enable iOS lock-screen push.";
export const IOS_PWA_LOCK_SCREEN_PUSH_TIP_ZH =
  "提示：在 iPhone 上请先轻点「分享 → 添加到主屏幕」，即可开启 iOS 锁屏推送。";

/**
 * Converts a URL-safe base64 (`base64url`) VAPID P-256 public key into a `Uint8Array<ArrayBuffer>`
 * suitable for `PushManager.subscribe({ userVisibleOnly: true, applicationServerKey })`.
 */
export function vapidBase64UrlToUint8Array(base64Url: string): Uint8Array<ArrayBuffer> {
  const cleaned = String(base64Url || "").trim();
  if (!cleaned) {
    throw new Error("VAPID public key is empty");
  }
  if (!/^[A-Za-z0-9+/_=-]+$/.test(cleaned)) {
    throw new Error("Invalid characters in VAPID public key");
  }
  const unpadded = cleaned.replace(/=+$/, "").replace(/-/g, "+").replace(/_/g, "/");
  if (!unpadded || unpadded.length % 4 === 1) {
    throw new Error("Malformed base64url VAPID public key length");
  }
  const padded = unpadded + "=".repeat((4 - (unpadded.length % 4)) % 4);
  const rawBinary = atob(padded);
  const out = new Uint8Array(new ArrayBuffer(rawBinary.length));
  for (let i = 0; i < rawBinary.length; i += 1) {
    out[i] = rawBinary.charCodeAt(i);
  }
  return out;
}

/**
 * Encodes an `ArrayBuffer` or `Uint8Array` into unpadded RFC 4648 `base64url`.
 */
export function arrayBufferToBase64Url(
  buf: ArrayBuffer | Uint8Array | null | undefined
): string {
  if (!buf) return "";
  const bytes = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (bytes.length === 0) return "";
  let binary = "";
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export interface PushSubscriptionLike {
  endpoint?: string;
  options?: {
    applicationServerKey?: ArrayBuffer | ArrayBufferView | null;
  };
  unsubscribe?: () => Promise<boolean>;
  getKey?: (name: "p256dh" | "auth") => ArrayBuffer | null;
  toJSON?: () => {
    endpoint?: string;
    keys?: Record<string, string | undefined>;
  };
}

/**
 * Returns `true` when an existing browser `PushSubscription` has an `applicationServerKey`
 * that differs from the current coordinator VAPID public key (e.g. after server key rotation),
 * requiring `pushSub.unsubscribe()` before re-subscribing.
 */
export function isPushSubscriptionVapidKeyMismatch(
  existingKey: ArrayBuffer | ArrayBufferView | null | undefined,
  expectedServerKey: Uint8Array
): boolean {
  if (existingKey === undefined) {
    return false;
  }
  if (existingKey === null) {
    return expectedServerKey.byteLength > 0;
  }
  const existingBytes = ArrayBuffer.isView(existingKey)
    ? new Uint8Array(existingKey.buffer, existingKey.byteOffset, existingKey.byteLength)
    : new Uint8Array(existingKey);
  if (existingBytes.byteLength !== expectedServerKey.byteLength) {
    return true;
  }
  for (let i = 0; i < existingBytes.byteLength; i += 1) {
    if (existingBytes[i] !== expectedServerKey[i]) {
      return true;
    }
  }
  return false;
}

export const SNIPPETS_CACHE_STORAGE_KEY = "coda_snippets_cache";
export const ONBOARDING_DISMISSED_STORAGE_PREFIX = "coda_onboarding_dismissed";

export function onboardingDismissedStorageKey(userId: string | null | undefined): string {
  const cleanId = String(userId || "").trim();
  return cleanId
    ? `${ONBOARDING_DISMISSED_STORAGE_PREFIX}:${cleanId}`
    : ONBOARDING_DISMISSED_STORAGE_PREFIX;
}

export interface StorageRemoveLike {
  removeItem(key: string): void;
}

export interface SignedOutAccountLocalState {
  webPushThisDeviceSubscribed: false;
  webPushEndpoint: "";
  webPushCount: 0;
  promptSnippets: PromptSnippet[];
  onboardingDismissed: false;
  onboardingCopiedInstall: false;
}

/**
 * Clears account-scoped local cache entries (`coda_snippets_cache`) and returns clean reset values
 * for Web Push, Saved Snippets, and Onboarding state on sign-out.
 */
export function clearAccountScopedSignOutState(
  storage?: StorageRemoveLike | null
): SignedOutAccountLocalState {
  try {
    storage?.removeItem(SNIPPETS_CACHE_STORAGE_KEY);
  } catch {
    // ignore storage errors
  }
  return {
    webPushThisDeviceSubscribed: false,
    webPushEndpoint: "",
    webPushCount: 0,
    promptSnippets: [...DEFAULT_PROMPT_SNIPPETS],
    onboardingDismissed: false,
    onboardingCopiedInstall: false,
  };
}

/**
 * Best-effort W3C Web Push teardown executed BEFORE clearing/revoking the bearer auth token
 * on Sign Out / Sign Out All: unsubscribes on the server first (while bearer token is valid)
 * AND unsubscribes the browser `PushSubscription` so neither tenant alerts nor UI state leak.
 */
export async function teardownWebPushOnSignOut(opts: {
  getSubscription: () => Promise<PushSubscriptionLike | null | undefined>;
  knownEndpoint?: string | null;
  hasAuthToken: boolean;
  unsubscribeServer: (endpoint: string) => Promise<unknown>;
}): Promise<{
  endpoint: string;
  unsubscribedServer: boolean;
  unsubscribedBrowser: boolean;
  webPushThisDeviceSubscribed: false;
  webPushEndpoint: "";
  webPushCount: 0;
}> {
  let pushSub: PushSubscriptionLike | null | undefined = null;
  try {
    pushSub = await opts.getSubscription();
  } catch {
    pushSub = null;
  }
  const endpoint = String(pushSub?.endpoint || opts.knownEndpoint || "").trim();
  let unsubscribedServer = false;
  let unsubscribedBrowser = false;

  if (opts.hasAuthToken && endpoint) {
    try {
      await opts.unsubscribeServer(endpoint);
      unsubscribedServer = true;
    } catch {
      // best-effort while bearer token is still valid
    }
  }

  if (pushSub && typeof pushSub.unsubscribe === "function") {
    try {
      unsubscribedBrowser = Boolean(await pushSub.unsubscribe());
    } catch {
      unsubscribedBrowser = false;
    }
  }

  return {
    endpoint,
    unsubscribedServer,
    unsubscribedBrowser,
    webPushThisDeviceSubscribed: false,
    webPushEndpoint: "",
    webPushCount: 0,
  };
}

/**
 * Extracts `endpoint`, `p256dh` (`base64url`), and `auth` (`base64url`) from a W3C `PushSubscription`.
 */
export function extractPushSubscriptionKeys(sub: PushSubscriptionLike | null | undefined): {
  endpoint: string;
  p256dh: string;
  auth: string;
} {
  if (!sub) {
    throw new Error("Missing PushSubscription");
  }
  const json = typeof sub.toJSON === "function" ? sub.toJSON() : undefined;
  const endpoint = String(sub.endpoint || json?.endpoint || "").trim();
  if (!endpoint) {
    throw new Error("PushSubscription is missing endpoint URL");
  }

  let p256dh = "";
  let auth = "";
  if (typeof sub.getKey === "function") {
    p256dh = arrayBufferToBase64Url(sub.getKey("p256dh"));
    auth = arrayBufferToBase64Url(sub.getKey("auth"));
  }
  if (!p256dh && json?.keys?.p256dh) {
    p256dh = arrayBufferToBase64Url(vapidBase64UrlToUint8Array(json.keys.p256dh));
  }
  if (!auth && json?.keys?.auth) {
    auth = arrayBufferToBase64Url(vapidBase64UrlToUint8Array(json.keys.auth));
  }
  if (!p256dh || !auth) {
    throw new Error("PushSubscription is missing p256dh or auth encryption keys");
  }
  return { endpoint, p256dh, auth };
}

/**
 * Detects iOS / iPadOS Safari or WebKit when NOT running as a Home Screen standalone Web App.
 */
export function isIosNonStandaloneBrowser(
  userAgent: string,
  isStandalone: boolean,
  maxTouchPoints = 0
): boolean {
  if (isStandalone) return false;
  const ua = String(userAgent || "");
  if (/\b(iPhone|iPad|iPod)\b/i.test(ua)) {
    return true;
  }
  if (/\bMacintosh\b/i.test(ua) && maxTouchPoints > 1) {
    return true;
  }
  return false;
}

export function detectBrowserIosNonStandalone(): boolean {
  if (typeof window === "undefined" || typeof navigator === "undefined") {
    return false;
  }
  const standalone = Boolean(
    window.matchMedia?.("(display-mode: standalone)")?.matches ||
      (navigator as unknown as { standalone?: boolean }).standalone === true
  );
  return isIosNonStandaloneBrowser(
    navigator.userAgent || "",
    standalone,
    navigator.maxTouchPoints || 0
  );
}

export function renderWebPushSettingsCard(opts: {
  vapid: VapidPublicKeyInfo;
  vapidLoaded: boolean;
  webPushCount: number;
  thisDeviceSubscribed: boolean;
  busy: boolean;
  pushSupported: boolean;
  iosNeedsHomeScreenTip: boolean;
  tr: (en: string, zh: string) => string;
}): string {
  const { tr } = opts;
  const statusBadge = opts.thisDeviceSubscribed
    ? `<span class="pm-badge green"><span class="pm-status-dot"></span>${tr("Active on This Device", "当前设备已开启")}${opts.webPushCount > 0 ? ` (${opts.webPushCount})` : ""}</span>`
    : opts.webPushCount > 0
      ? `<span class="pm-badge blue">${tr(`${opts.webPushCount} Active Device(s)`, `${opts.webPushCount} 台设备已订阅`)}</span>`
      : `<span class="pm-badge">${tr("0 Active", "未订阅")}</span>`;

  const canSubscribe =
    opts.pushSupported && (!opts.vapidLoaded || Boolean(opts.vapid.enabled && opts.vapid.public_key));

  return `
    <div class="web-push-card">
      <div style="display:flex;justify-content:space-between;align-items:flex-start;gap:10px;flex-wrap:wrap;">
        <div style="min-width:0;flex:1;">
          <div style="font-size:13px;font-weight:600;display:flex;align-items:center;gap:7px;flex-wrap:wrap;">
            <span>🔔 ${tr("Zero-Config Lock-Screen Web Push (W3C VAPID)", "零配置锁屏系统推送 (W3C VAPID)")}</span>
            ${statusBadge}
          </div>
          <div style="font-size:11.5px;color:var(--text-secondary);line-height:1.55;margin-top:4px;">
            ${tr(
              "Receive encrypted lock-screen alerts on Desktop Browser & iOS Home Screen PWA even after closing all Coda tabs — zero Bark/Telegram setup required.",
              "关闭全部浏览器标签页后，当远端 Agent 需要审批或任务完成时仍可直达桌面与 iOS 主屏幕 PWA 锁屏通知，无需配置第三方 Webhook。"
            )}
          </div>
        </div>
      </div>

      ${
        opts.iosNeedsHomeScreenTip
          ? `<div class="ios-pwa-tip">
              ${escapeHtml(tr(IOS_PWA_LOCK_SCREEN_PUSH_TIP_EN, IOS_PWA_LOCK_SCREEN_PUSH_TIP_ZH))}
            </div>`
          : ""
      }

      ${
        opts.vapidLoaded && !opts.vapid.enabled
          ? `<div style="font-size:11.5px;color:var(--text-tertiary);">
              ${tr(
                "Server VAPID Web Push is unavailable in stateless relay-only mode.",
                "当前服务端运行在无数据库纯中继模式，暂未启用 VAPID Web Push。"
              )}
            </div>`
          : !opts.pushSupported
            ? `<div style="font-size:11.5px;color:var(--text-tertiary);">
                ${tr(
                  "This browser context does not expose Service Worker PushManager (use HTTPS / localhost or add to iOS Home Screen).",
                  "当前浏览器环境不支持 Service Worker PushManager（请使用 HTTPS / localhost，或在 iPhone 上添加到主屏幕后打开）。"
                )}
              </div>`
            : ""
      }

      <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
        ${
          opts.thisDeviceSubscribed
            ? `
          <button type="button" data-enable-web-push="1" class="pm-btn pm-btn-secondary pm-btn-sm" ${opts.busy ? "disabled" : ""}>
            ${svgIcon("check", 13)} ${tr("Re-sync Lock-Screen Push", "重新同步锁屏推送")}
          </button>
          <button type="button" data-disable-web-push="1" class="pm-btn pm-btn-ghost pm-btn-sm" style="color:var(--rose);" ${opts.busy ? "disabled" : ""}>
            ${tr("Disable on This Device", "在当前设备停用")}
          </button>
        `
            : `
          <button type="button" data-enable-web-push="1" class="pm-btn pm-btn-primary pm-btn-sm" ${opts.busy || !canSubscribe ? "disabled" : ""}>
            🔔 ${
              opts.busy
                ? tr("Subscribing…", "正在开启锁屏推送…")
                : tr(
                    "Enable Lock-Screen Push (Browser / iOS PWA)",
                    "一键开启锁屏推送 (浏览器 / iOS PWA)"
                  )
            }
          </button>
        `
        }
        <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">
          ${tr(`Subscribed endpoints: ${opts.webPushCount}`, `当前账号已订阅设备：${opts.webPushCount}`)}
        </span>
      </div>
    </div>
  `;
}

// ============================================================================
// 2. Invite Deep-Links, Password Reset Token Parsing & Admin Friends Directory
// ============================================================================

/**
 * Extracts a clean `rst_...` reset token from either a raw token string OR a full/partial
 * `/reset-password?token=rst_...` URL pasted by a friend from WeChat / IM.
 */
export function extractResetTokenFromInput(rawInput: string): string {
  const trimmed = String(rawInput || "").trim();
  if (!trimmed) return "";

  const urlCandidateMatch = trimmed.match(/https?:\/\/[^\s"'<>]+/i);
  if (urlCandidateMatch) {
    try {
      const parsed = new URL(urlCandidateMatch[0]);
      const t = (parsed.searchParams.get("token") || "").trim();
      if (t) return t;
    } catch {
      // continue to query / token regex fallbacks
    }
  }

  const queryMatch = trimmed.match(/(?:^|[?&])token=([A-Za-z0-9._~%-]+)/);
  if (queryMatch?.[1]) {
    try {
      const decoded = decodeURIComponent(queryMatch[1]).trim();
      if (decoded) return decoded;
    } catch {
      return queryMatch[1].trim();
    }
  }

  const rstMatch = trimmed.match(/\brst_[A-Za-z0-9_-]{8,}\b/);
  if (rstMatch?.[0]) {
    return rstMatch[0];
  }

  // A bare URL or path without `?token=...` is not a valid reset token
  if (/^https?:\/\//i.test(trimmed) || trimmed.startsWith("/")) {
    return "";
  }

  return trimmed;
}

export function buildInviteRegistrationUrl(baseUrl: string, code: string): string {
  const cleanBase = String(baseUrl || "").trim().replace(/\/+$/, "");
  const cleanCode = String(code || "").trim();
  return `${cleanBase}/register?invite=${encodeURIComponent(cleanCode)}`;
}

/**
 * Parses `window.location.pathname` + `window.location.search` into the target `AppRoute`
 * plus any deep-linked `inviteCode` (`/register?invite=...` or `/?invite=...`),
 * `resetToken` (`/reset-password?token=...`), and `activationCode` (`/activate?code=...`).
 */
export function parseLocationRouteAndParams(
  pathname: string,
  search: string
): {
  route: AppRoute;
  inviteCode: string;
  resetToken: string;
  activationCode: string;
} {
  const cleanPath = String(pathname || "/").replace(/\/+$/, "") || "/";
  const params = new URLSearchParams(String(search || ""));
  const inviteCode = (params.get("invite") || params.get("invite_code") || "").trim();
  const resetToken = extractResetTokenFromInput(params.get("token") || "");
  const activationCode = (params.get("code") || "").trim().toUpperCase();

  let route: AppRoute = "home";
  if (cleanPath === "/reset-password") {
    route = "reset-password";
  } else if (cleanPath === "/login") {
    route = "login";
  } else if (cleanPath === "/register") {
    route = "register";
  } else if (cleanPath === "/activate") {
    route = "activate";
  } else if (cleanPath === "/console") {
    route = "console";
  } else if (cleanPath === "/" && inviteCode.length > 0) {
    route = "register";
  } else if (cleanPath === "/" && activationCode.length > 0) {
    route = "activate";
  }

  return {
    route,
    inviteCode,
    resetToken,
    activationCode,
  };
}

export function validateResetPasswordInputs(
  rawToken: string,
  newPassword: string,
  confirmPassword: string,
  tr: (en: string, zh: string) => string
): { ok: true; token: string; newPassword: string } | { ok: false; error: string } {
  const token = extractResetTokenFromInput(rawToken);
  if (!token) {
    return {
      ok: false,
      error: tr(
        "Please paste the one-time reset token or reset link from the admin.",
        "请粘贴管理员发送的一次性重置链接或 rst_... 令牌。"
      ),
    };
  }
  const pw = String(newPassword || "");
  const pwBytes = new TextEncoder().encode(pw).length;
  if (pwBytes < 8) {
    return {
      ok: false,
      error: tr(
        "New password must be at least 8 characters.",
        "新密码长度至少需要 8 位字符。"
      ),
    };
  }
  if (pwBytes > 256) {
    return {
      ok: false,
      error: tr(
        "New password must not exceed 256 bytes.",
        "新密码长度不能超过 256 字节。"
      ),
    };
  }
  if (pw !== String(confirmPassword || "")) {
    return {
      ok: false,
      error: tr("New password and confirmation do not match.", "两次输入的新密码不一致。"),
    };
  }
  return { ok: true, token, newPassword: pw };
}

export function validateNewPasswordLength(
  password: string,
  tr: (en: string, zh: string) => string
): { ok: true } | { ok: false; error: string } {
  const pwBytes = new TextEncoder().encode(String(password || "")).length;
  if (pwBytes < 8) {
    return {
      ok: false,
      error: tr(
        "Password must be at least 8 characters.",
        "密码长度至少需要 8 位字符。"
      ),
    };
  }
  if (pwBytes > 256) {
    return {
      ok: false,
      error: tr(
        "Password must not exceed 256 bytes.",
        "密码长度不能超过 256 字节。"
      ),
    };
  }
  return { ok: true };
}

export function formatAdminLastLogin(
  lastLoginAt: string | null | undefined,
  tr: (en: string, zh: string) => string
): string {
  const raw = String(lastLoginAt || "").trim();
  if (!raw) return tr("Never", "从未登录");
  return raw.replace("T", " ").slice(0, 16);
}

export function renderAdminFriendsDirectorySection(opts: {
  users: readonly AdminUserSummary[];
  busyUserId: string | null;
  latestResetLink: { handle: string; resetUrl: string } | null;
  tr: (en: string, zh: string) => string;
}): string {
  const { tr } = opts;
  return `
    <div style="border-top:1px solid var(--border-primary);padding-top:12px;margin-bottom:14px;">
      <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;margin-bottom:8px;">
        <strong style="font-size:12.5px;">${tr(`Friends Directory (${opts.users.length})`, `好友与成员目录 (${opts.users.length})`)}</strong>
        <span class="pm-badge" style="font-size:10.5px;">${tr("1-Click Reset Link · 24h Single-Use", "一键重置链接 · 24h 单次有效")}</span>
      </div>

      ${
        opts.latestResetLink
          ? `<div class="coda-mono" style="background:var(--bg-elevated);border:1px dashed var(--emerald);padding:8px 10px;border-radius:8px;font-size:11px;margin-bottom:8px;display:flex;align-items:center;justify-content:space-between;gap:8px;flex-wrap:wrap;">
              <div style="min-width:0;flex:1;word-break:break-all;">
                <span style="color:var(--emerald);font-weight:600;">@${escapeHtml(opts.latestResetLink.handle)}:</span>
                <span>${escapeHtml(opts.latestResetLink.resetUrl)}</span>
              </div>
              <button type="button" id="btn-copy-latest-reset-link" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 8px;font-size:11px;">
                ${svgIcon("copy", 12)} ${tr("Copy", "复制")}
              </button>
            </div>`
          : ""
      }

      <div class="admin-friends-list">
        ${
          opts.users.length === 0
            ? `<div style="padding:10px;font-size:11.5px;color:var(--text-tertiary);text-align:center;">${tr("Loading registered members…", "正在加载已注册成员…")}</div>`
            : opts.users
                .map((u) => {
                  const isBusy = opts.busyUserId === u.id;
                  const onlineCls = u.online_devices > 0 ? "green" : "";
                  return `
            <div class="admin-friend-row">
              <div style="min-width:0;flex:1;">
                <div style="display:flex;align-items:center;gap:6px;flex-wrap:wrap;font-size:12px;">
                  <strong class="coda-mono">@${escapeHtml(u.handle)}</strong>
                  <span style="color:var(--text-secondary);">${escapeHtml(u.display_name || u.handle)}</span>
                  ${u.is_admin ? `<span class="pm-badge blue" style="height:18px;padding:0 6px;font-size:10px;">Admin</span>` : ""}
                  <span class="pm-badge ${onlineCls} coda-mono" style="height:18px;padding:0 6px;font-size:10px;">
                    ${u.online_devices}/${u.total_devices} ${tr("devices", "设备")}
                  </span>
                </div>
                <div class="coda-mono" style="font-size:10.5px;color:var(--text-tertiary);margin-top:2px;">
                  ${tr("Last login:", "最近登录：")} ${escapeHtml(formatAdminLastLogin(u.last_login_at, tr))}
                </div>
              </div>
              <button
                type="button"
                class="pm-btn pm-btn-secondary pm-btn-sm"
                style="height:26px;padding:0 9px;font-size:11px;flex-shrink:0;"
                data-admin-reset-user="${escapeHtml(u.id)}"
                data-admin-reset-handle="${escapeHtml(u.handle)}"
                ${isBusy ? "disabled" : ""}
              >
                ${svgIcon("key", 12)} ${isBusy ? tr("Issuing…", "生成中…") : tr("Copy Reset Link", "复制重置链接")}
              </button>
            </div>`;
                })
                .join("")
        }
      </div>
    </div>
  `;
}

// ============================================================================
// 3. Cross-Device Saved Prompt Snippets & 3-Step Friend Onboarding Card
// ============================================================================

export const MAX_PROMPT_SNIPPETS = 24;
export const MAX_SNIPPET_TITLE_CHARS = 60;
export const MAX_SNIPPET_PROMPT_CHARS = 4000;

export const DEFAULT_PROMPT_SNIPPETS: readonly PromptSnippet[] = [
  {
    id: "s1",
    title: "Run Tests & Fix",
    prompt:
      "Run the full test suite, fix any failures or compiler warnings, and summarize what changed.",
  },
  {
    id: "s2",
    title: "Review Git Diff",
    prompt:
      "Review the current git diff carefully for bugs, edge cases, or regressions before committing.",
  },
  {
    id: "s3",
    title: "Continue",
    prompt: "Continue with the next steps until everything is complete and verified.",
  },
  {
    id: "s4",
    title: "Clean Commit",
    prompt: "Stage and commit the current changes with a concise conventional commit message.",
  },
];

const STARTER_SNIPPET_IDS = new Set(["s1", "s2", "s3", "s4"]);

export function isCustomPromptSnippet(snippet: Pick<PromptSnippet, "id">): boolean {
  return !STARTER_SNIPPET_IDS.has(String(snippet.id || "").trim());
}

export function deriveSnippetTitleFromPrompt(prompt: string): string {
  const firstLine = String(prompt || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line.length > 0);
  if (!firstLine) return "";
  const compact = firstLine.replace(/\s+/g, " ");
  return compact.length <= 36 ? compact : `${compact.slice(0, 35).trimEnd()}…`;
}

export function validatePromptSnippet(input: {
  id?: string;
  title: string;
  prompt: string;
}): { ok: true; snippet: PromptSnippet } | { ok: false; error: string } {
  const id =
    String(input.id || "").trim() ||
    `snip-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
  const title = String(input.title || "").trim();
  const prompt = String(input.prompt || "").trim();

  if (!title) {
    return { ok: false, error: "Snippet title is required" };
  }
  if (Array.from(title).length > MAX_SNIPPET_TITLE_CHARS || title.length > MAX_SNIPPET_TITLE_CHARS) {
    return {
      ok: false,
      error: `Snippet title must be at most ${MAX_SNIPPET_TITLE_CHARS} characters`,
    };
  }
  if (!prompt) {
    return { ok: false, error: "Snippet prompt is required" };
  }
  if (
    Array.from(prompt).length > MAX_SNIPPET_PROMPT_CHARS ||
    prompt.length > MAX_SNIPPET_PROMPT_CHARS
  ) {
    return {
      ok: false,
      error: `Snippet prompt must be at most ${MAX_SNIPPET_PROMPT_CHARS} characters`,
    };
  }
  return {
    ok: true,
    snippet: { id, title, prompt },
  };
}

export function normalizePromptSnippetsList(raw: unknown): PromptSnippet[] {
  if (!Array.isArray(raw)) return [];
  const out: PromptSnippet[] = [];
  const seenIds = new Set<string>();

  for (const item of raw) {
    if (out.length >= MAX_PROMPT_SNIPPETS) break;
    if (!item || typeof item !== "object") continue;
    const rec = item as Record<string, unknown>;
    const rawId = typeof rec.id === "string" ? rec.id.trim() : "";
    const rawTitle = typeof rec.title === "string" ? rec.title : "";
    const rawPrompt = typeof rec.prompt === "string" ? rec.prompt : "";
    if (!rawId || seenIds.has(rawId)) continue;
    const validated = validatePromptSnippet({
      id: rawId,
      title: rawTitle,
      prompt: rawPrompt,
    });
    if (validated.ok) {
      seenIds.add(validated.snippet.id);
      out.push(validated.snippet);
    }
  }
  return out;
}

export function insertSnippetIntoDraft(currentDraft: string, snippetPrompt: string): string {
  const cleanSnippet = String(snippetPrompt || "").trim();
  if (!cleanSnippet) return currentDraft;
  const existing = String(currentDraft || "");
  if (!existing.trim()) {
    return cleanSnippet;
  }
  const sep = existing.endsWith("\n") ? "" : "\n";
  return `${existing}${sep}${cleanSnippet}`;
}

export function renderFriendOnboardingCard(opts: {
  installCmd: string;
  copiedInstall: boolean;
  thisDevicePushSubscribed: boolean;
  webPushCount: number;
  webPushBusy: boolean;
  iosNeedsHomeScreenTip: boolean;
  userHandle: string;
  tr: (en: string, zh: string) => string;
}): string {
  const { tr } = opts;
  return `
    <section class="friend-onboarding-card" id="friend-onboarding-card" aria-label="${tr("3-Step Quickstart", "三步快速上手")}">
      <div style="display:flex;justify-content:space-between;align-items:flex-start;gap:10px;">
        <div>
          <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
            <span class="pm-badge green"><span class="pm-status-dot"></span>${tr("Welcome", "欢迎加入")} @${escapeHtml(opts.userHandle)}</span>
            <strong style="font-size:14px;">${tr("3-Step Quickstart · Connect Your First AI Host", "三步快速上手 · 接入你的第一台 AI 开发机")}</strong>
          </div>
          <div style="font-size:12px;color:var(--text-secondary);margin-top:4px;">
            ${tr(
              "Pair any macOS workstation or Linux server in 30 seconds — zero inbound ports, full E2EE.",
              "30 秒绑定你的 Mac 或 Linux 服务器，零公网端口暴露，全程端到端零知识加密。"
            )}
          </div>
        </div>
        <button type="button" id="btn-onboarding-dismiss" class="circle-btn" style="width:26px;height:26px;flex-shrink:0;" title="${tr("Dismiss quickstart card", "收起新手引导卡片")}">${svgIcon("close", 12)}</button>
      </div>

      <div class="onboarding-steps-grid">
        <div class="onboarding-step-box">
          <div style="display:flex;align-items:center;gap:7px;">
            <span class="pm-badge blue coda-mono">Step 1</span>
            <strong style="font-size:12.5px;">${tr("Copy 1-line install command to bind your Mac/Linux host", "复制一行安装命令绑定你的 Mac / Linux 主机")}</strong>
          </div>
          <div class="coda-mono" style="font-size:11px;padding:7px 9px;border-radius:7px;background:var(--bg-canvas);border:1px solid var(--border-primary);color:var(--emerald);overflow-x:auto;white-space:nowrap;">
            $ ${escapeHtml(opts.installCmd)}
          </div>
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            <button type="button" id="btn-onboarding-copy-install" class="pm-btn pm-btn-primary pm-btn-sm">
              ${opts.copiedInstall ? svgIcon("check", 12) : svgIcon("copy", 12)}
              ${opts.copiedInstall ? tr("Copied!", "已复制命令") : tr("Copy Install Command", "复制安装命令")}
            </button>
            <button type="button" id="btn-onboarding-open-devices" class="pm-btn pm-btn-secondary pm-btn-sm">
              ${svgIcon("key", 12)} ${tr("Approve Code", "输入设备码")}
            </button>
          </div>
        </div>

        <div class="onboarding-step-box">
          <div style="display:flex;align-items:center;gap:7px;">
            <span class="pm-badge blue coda-mono">Step 2</span>
            <strong style="font-size:12.5px;">${tr("Launch Claude Code / Codex / Grok / AGY", "一键拉起 Claude Code / Codex / Grok / AGY")}</strong>
          </div>
          <div style="font-size:11.5px;color:var(--text-secondary);line-height:1.55;">
            ${tr(
              "Once your host is online, spawn persistent herdr-core sessions for Claude Code, Codex, Grok CLI, Antigravity (AGY), or Gemini CLI with E2EE Git Diff & Port Preview.",
              "主机上线后，点击右侧「+」即可在远端常驻运行 Claude Code、Codex、Grok CLI、Antigravity (AGY) 或 Gemini CLI，合盖断网不中断。"
            )}
          </div>
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            <button type="button" id="btn-onboarding-launch-agent" class="pm-btn pm-btn-secondary pm-btn-sm">
              ${svgIcon("plus", 12)} ${tr("Launch Agent (+)", "新建 Agent 会话")}
            </button>
          </div>
        </div>

        <div class="onboarding-step-box">
          <div style="display:flex;align-items:center;gap:7px;">
            <span class="pm-badge blue coda-mono">Step 3</span>
            <strong style="font-size:12.5px;">${tr("Enable 1-Click Lock-Screen Push", "开启一键锁屏推送通知")}</strong>
          </div>
          <div style="font-size:11.5px;color:var(--text-secondary);line-height:1.55;">
            ${tr(
              "Get notified on your lock screen the moment a background agent pauses for approval (Needs-You) or finishes a build.",
              "当后台 Agent 等待交互审批 (Needs-You) 或编译完成时，第一时间收到手机/电脑锁屏提醒。"
            )}
          </div>
          ${
            opts.iosNeedsHomeScreenTip
              ? `<div class="ios-pwa-tip" style="margin:0;">
                  ${escapeHtml(tr(IOS_PWA_LOCK_SCREEN_PUSH_TIP_EN, IOS_PWA_LOCK_SCREEN_PUSH_TIP_ZH))}
                </div>`
              : ""
          }
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            ${
              opts.thisDevicePushSubscribed
                ? `<span class="pm-badge green" style="height:30px;padding:0 10px;"><span class="pm-status-dot"></span>${tr("Lock-Screen Push Active", "锁屏推送已开启")}</span>`
                : `<button type="button" data-enable-web-push="1" class="pm-btn pm-btn-primary pm-btn-sm" ${opts.webPushBusy ? "disabled" : ""}>
                    🔔 ${opts.webPushBusy ? tr("Enabling…", "开启中…") : tr("Enable Lock-Screen Push", "一键开启锁屏推送")}
                  </button>`
            }
            <button type="button" id="btn-onboarding-open-alerts" class="pm-btn pm-btn-ghost pm-btn-sm">
              ${tr("Webhooks", "更多推送设置")} &rarr;
            </button>
          </div>
        </div>
      </div>
    </section>
  `;
}

// ============================================================================
// 4. Console Truthful Empty State, Connection Indicators & Clipboard Images
// ============================================================================

export interface ClipboardItemLike {
  readonly types: readonly string[];
  getType(type: string): Promise<Blob>;
}

/**
 * Extracts image blobs (`image/*`) from `navigator.clipboard.read()` items when `readText()` is empty.
 */
export async function extractImagesFromClipboardItems(
  items: readonly ClipboardItemLike[] | null | undefined
): Promise<Blob[]> {
  if (!items || items.length === 0) return [];
  const out: Blob[] = [];
  for (const item of items) {
    if (!item || !Array.isArray(item.types)) continue;
    const imgType = item.types.find((t) =>
      String(t || "")
        .trim()
        .toLowerCase()
        .startsWith("image/")
    );
    if (!imgType || typeof item.getType !== "function") continue;
    try {
      const blob = await item.getType(imgType);
      if (blob && blob.size > 0) {
        out.push(blob);
      }
    } catch {
      // ignore unreadable representation
    }
  }
  return out;
}

export interface ConsoleConnectionVisualState {
  tone: "online" | "connecting" | "offline" | "idle";
  icon: "zap" | "cloud";
  sidebarColorCss: string;
  sidebarLabel: string;
  railBorderCss: string;
  railDotBgCss: string;
  railDotShadowCss: string;
  railTitle: string;
  railTransportBadgeHtml: string;
  footerTransport: string;
}

export function deriveConsoleConnectionVisualState(opts: {
  devicesCount: number;
  selectedDaemon: { name: string; is_online: boolean } | null;
  relayWsOpen: boolean;
  directWsOpen: boolean;
  transportMode: TransportMode;
  rttMs: number | null;
  tr: (en: string, zh: string) => string;
}): ConsoleConnectionVisualState {
  const { tr } = opts;
  if (opts.devicesCount === 0) {
    return {
      tone: "idle",
      icon: "cloud",
      sidebarColorCss: "var(--text-tertiary)",
      sidebarLabel: tr("No hosts paired", "暂未绑定主机"),
      railBorderCss: "var(--border-primary)",
      railDotBgCss: "var(--text-quaternary)",
      railDotShadowCss: "none",
      railTitle: tr(
        "No hosts paired yet — bind a Mac or Linux host to start",
        "尚未绑定主机 — 绑定 Mac 或 Linux 主机后即可开始"
      ),
      railTransportBadgeHtml: '<span style="color:var(--text-quaternary);">IDLE</span>',
      footerTransport: tr("No host paired", "暂未绑定主机"),
    };
  }

  if (!opts.selectedDaemon) {
    return {
      tone: "idle",
      icon: "cloud",
      sidebarColorCss: "var(--text-tertiary)",
      sidebarLabel: tr("No host selected", "未选择主机"),
      railBorderCss: "var(--border-primary)",
      railDotBgCss: "var(--text-quaternary)",
      railDotShadowCss: "none",
      railTitle: tr("No host selected", "未选择主机"),
      railTransportBadgeHtml: '<span style="color:var(--text-quaternary);">—</span>',
      footerTransport: tr("No host selected", "未选择主机"),
    };
  }

  if (!opts.selectedDaemon.is_online) {
    return {
      tone: "offline",
      icon: "cloud",
      sidebarColorCss: "var(--text-tertiary)",
      sidebarLabel: tr("Host offline", "主机离线"),
      railBorderCss: "var(--border-primary)",
      railDotBgCss: "var(--text-quaternary)",
      railDotShadowCss: "none",
      railTitle: tr(
        `Host ${opts.selectedDaemon.name} is currently offline`,
        `主机 ${opts.selectedDaemon.name} 当前处于离线状态`
      ),
      railTransportBadgeHtml: '<span style="color:var(--text-quaternary);">OFF</span>',
      footerTransport: tr(
        `Host ${opts.selectedDaemon.name} offline`,
        `主机 ${opts.selectedDaemon.name} 离线`
      ),
    };
  }

  if (opts.transportMode === "lan_ws" && opts.directWsOpen) {
    const rttSuffix = opts.rttMs !== null ? ` (${opts.rttMs}ms)` : "";
    return {
      tone: "online",
      icon: "zap",
      sidebarColorCss: "var(--emerald)",
      sidebarLabel: `${tr("LAN Direct", "局域网直连")}${rttSuffix}`,
      railBorderCss: "var(--emerald-bd)",
      railDotBgCss: "var(--emerald)",
      railDotShadowCss: "0 0 8px var(--emerald)",
      railTitle: tr(
        `Connected to ${opts.selectedDaemon.name} via LAN Direct${rttSuffix}`,
        `已通过局域网直连主机 ${opts.selectedDaemon.name}${rttSuffix}`
      ),
      railTransportBadgeHtml: '<span style="color:var(--emerald);">LAN</span>',
      footerTransport: opts.rttMs !== null ? `${opts.rttMs}ms RTT · LAN Direct` : "LAN Direct :24444",
    };
  }

  if (opts.relayWsOpen) {
    const rttSuffix = opts.rttMs !== null ? ` (${opts.rttMs}ms)` : "";
    return {
      tone: "online",
      icon: "cloud",
      sidebarColorCss: "var(--emerald)",
      sidebarLabel: `${tr("Smart Relay", "智能中继")}${rttSuffix}`,
      railBorderCss: "var(--emerald-bd)",
      railDotBgCss: "var(--emerald)",
      railDotShadowCss: "0 0 8px var(--emerald)",
      railTitle: tr(
        `Connected to ${opts.selectedDaemon.name} via E2EE Smart Relay${rttSuffix}`,
        `已通过端到端加密智能中继连接 ${opts.selectedDaemon.name}${rttSuffix}`
      ),
      railTransportBadgeHtml: '<span style="color:var(--emerald);">WSS</span>',
      footerTransport: opts.rttMs !== null ? `${opts.rttMs}ms RTT` : "Relay WSS :443",
    };
  }

  return {
    tone: "connecting",
    icon: "cloud",
    sidebarColorCss: "var(--amber)",
    sidebarLabel: tr("Connecting relay…", "正在连接中继…"),
    railBorderCss: "var(--amber-bd)",
    railDotBgCss: "var(--amber)",
    railDotShadowCss: "none",
    railTitle: tr(
      `Connecting E2EE relay to ${opts.selectedDaemon.name}…`,
      `正在连接至 ${opts.selectedDaemon.name} 的加密中继…`
    ),
    railTransportBadgeHtml: '<span style="color:var(--amber);">WSS</span>',
    footerTransport: tr("Connecting relay…", "正在连接中继…"),
  };
}

export function formatConsoleHeaderSubtitle(opts: {
  devicesCount: number;
  selectedDaemon: { name: string; is_online: boolean } | null;
  activeCwd?: string | null;
  activeBranch?: string | null;
  tr: (en: string, zh: string) => string;
}): string {
  const { tr } = opts;
  const parts: string[] = [];
  const cwd = String(opts.activeCwd || "").trim();
  const branch = String(opts.activeBranch || "").trim();
  if (cwd) {
    parts.push(escapeHtml(cwd));
  }
  if (branch) {
    parts.push(escapeHtml(branch));
  }
  if (opts.devicesCount === 0) {
    parts.push(escapeHtml(tr("No hosts paired", "暂未绑定主机")));
  } else if (opts.selectedDaemon) {
    const statusSuffix = opts.selectedDaemon.is_online ? "" : ` (${tr("offline", "离线")})`;
    parts.push(escapeHtml(`${opts.selectedDaemon.name}${statusSuffix}`));
  } else {
    parts.push(escapeHtml(tr("Unselected", "未选择主机")));
  }
  return parts.join(" &middot; ");
}

export function renderConsoleEmptyStateHtml(opts: {
  devicesCount: number;
  selectedDaemon: { name: string; is_online: boolean } | null;
  activeTerminalName?: string | null;
  relayWsOpen: boolean;
  directWsOpen: boolean;
  transportMode: TransportMode;
  rttMs: number | null;
  installCmd: string;
  tr: (en: string, zh: string) => string;
}): string {
  const { tr } = opts;
  if (opts.devicesCount === 0) {
    const safeCmd = escapeHtml(opts.installCmd);
    return tr(
      `  No remote hosts paired to this account yet.\n  Run the 1-line install command on your Mac or Linux host to bind your first host:\n    $ ${safeCmd}\n  Then approve its device code in Settings or /activate.`,
      `  当前账号尚未绑定任何远端主机。\n  请在你的 Mac 或 Linux 主机运行以下一行安装命令绑定首台主机：\n    $ ${safeCmd}\n  随后在「设置」或 /activate 页面批准设备码即可接入。`
    );
  }

  if (!opts.selectedDaemon) {
    return escapeHtml(
      tr(
        "  Select a paired host from the left sidebar to view or launch terminals.",
        "  请从左侧边栏选择一台已绑定主机以查看或启动终端。"
      )
    );
  }

  const safeHost = escapeHtml(opts.selectedDaemon.name);
  if (!opts.selectedDaemon.is_online) {
    return tr(
      `  Host ${safeHost} is currently offline.\n  Start coda-daemon (or open Coda.app in Host mode) on that machine to reconnect over E2EE.`,
      `  主机 ${safeHost} 当前处于离线状态。\n  请在该机器上启动 coda-daemon（或打开 Coda.app 开启主机模式）以恢复端到端加密连接。`
    );
  }

  const activeTerm = String(opts.activeTerminalName || "").trim();
  if (activeTerm) {
    const safeTerm = escapeHtml(activeTerm);
    return tr(
      `  Attaching to ${safeTerm} on ${safeHost} over E2EE…`,
      `  正在通过端到端加密连接 ${safeHost} 上的终端会话 ${safeTerm}…`
    );
  }

  const isConnected =
    (opts.transportMode === "lan_ws" && opts.directWsOpen) || opts.relayWsOpen;
  const transportLine = isConnected
    ? `● ${safeHost} · ${
        opts.transportMode === "lan_ws" && opts.directWsOpen
          ? tr("LAN Direct", "局域网直连")
          : tr("E2EE Smart Relay", "端到端加密中继")
      }${opts.rttMs !== null ? ` · RTT ${opts.rttMs}ms` : ""}`
    : tr("○ Connecting E2EE relay…", "○ 正在建立端到端加密中继连接…");

  return tr(
    `  Host ${safeHost} is online (0 active terminals).\n  Press + to launch Claude Code, Codex, Grok, AGY, or Shell.\n\n  ${transportLine}`,
    `  主机 ${safeHost} 在线（当前暂无活跃终端）。\n  请按右侧「+」启动 Claude Code、Codex、Grok、AGY 或 Shell。\n\n  ${transportLine}`
  );
}
