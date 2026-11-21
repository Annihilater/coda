export interface ParsedSwPushPayload {
  title: string;
  body: string;
  url: string;
  tag: string;
}

/**
 * Normalizes a notification click destination so push payloads can only navigate
 * to safe relative paths (e.g. `/console`) or same-origin URLs — never `javascript:` or external origins.
 */
export function normalizeSwNotificationTargetUrl(
  rawUrl: unknown,
  swOrigin?: string
): string {
  const trimmed = typeof rawUrl === "string" ? rawUrl.trim() : "";
  if (!trimmed) return "/console";
  if (trimmed.startsWith("/") && !trimmed.startsWith("//")) {
    return trimmed;
  }
  if (swOrigin) {
    try {
      const parsed = new URL(trimmed);
      if (parsed.origin === swOrigin) {
        return `${parsed.pathname}${parsed.search}${parsed.hash}` || "/console";
      }
    } catch {
      // fall through to safe default
    }
  }
  return "/console";
}

/**
 * Determines whether the Service Worker `fetch` listener should intercept and cache a request.
 * Strictly NEVER intercepts non-GET, cross-origin, `/api/`, `/v1/`, `/download/`, `/install.sh`, or `/sw.js`.
 * Caches hashed bundle (`/app.*.js`) and static icons (`/logo.svg`, `/apple-touch-icon.png`) for instant PWA startup.
 */
export function shouldSwCacheStaticAsset(
  requestUrl: string,
  swOrigin: string,
  method = "GET"
): boolean {
  if (String(method || "GET").toUpperCase() !== "GET") {
    return false;
  }
  let url: URL;
  try {
    url = new URL(requestUrl);
  } catch {
    return false;
  }
  if (url.origin !== swOrigin) {
    return false;
  }
  const p = url.pathname;
  if (
    p === "/api" ||
    p.startsWith("/api/") ||
    p === "/v1" ||
    p.startsWith("/v1/") ||
    p === "/download" ||
    p.startsWith("/download/") ||
    p === "/install.sh" ||
    p === "/sw.js"
  ) {
    return false;
  }
  if (/^\/app\.[A-Za-z0-9_-]+\.js$/.test(p)) {
    return true;
  }
  if (p === "/logo.svg" || p === "/apple-touch-icon.png") {
    return true;
  }
  return false;
}

/**
 * Parses incoming W3C Web Push payload (`{ title, body, url, tag }` JSON with plain-text `.text()` fallback).
 */
export function parseSwPushPayload(
  rawText: string | null | undefined,
  swOrigin?: string
): ParsedSwPushPayload {
  const raw = typeof rawText === "string" ? rawText : "";
  const trimmed = raw.trim();
  if (!trimmed) {
    return {
      title: "Coda Agent Alert",
      body: "",
      url: "/console",
      tag: "coda-alert",
    };
  }
  try {
    const parsed = JSON.parse(trimmed);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const rec = parsed as Record<string, unknown>;
      const title =
        typeof rec.title === "string" && rec.title.trim()
          ? rec.title.trim()
          : "Coda Agent Alert";
      const body = typeof rec.body === "string" ? rec.body : "";
      const tag =
        typeof rec.tag === "string" && rec.tag.trim() ? rec.tag.trim() : "coda-alert";
      const url = normalizeSwNotificationTargetUrl(rec.url, swOrigin);
      return { title, body, url, tag };
    }
    if (typeof parsed === "string") {
      return {
        title: "Coda Agent Alert",
        body: parsed,
        url: "/console",
        tag: "coda-alert",
      };
    }
  } catch {
    // Fallback to plain text when push payload is not JSON
  }
  return {
    title: "Coda Agent Alert",
    body: raw,
    url: "/console",
    tag: "coda-alert",
  };
}
