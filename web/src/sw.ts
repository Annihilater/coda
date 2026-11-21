import {
  normalizeSwNotificationTargetUrl,
  parseSwPushPayload,
  shouldSwCacheStaticAsset,
} from "./sw_shared";

interface SwWindowClientLike {
  url: string;
  focus?: () => Promise<SwWindowClientLike>;
  navigate?: (url: string) => Promise<SwWindowClientLike | null>;
}

interface SwClientsLike {
  claim: () => Promise<void>;
  matchAll: (opts?: {
    type?: "window" | "worker" | "sharedworker" | "all";
    includeUncontrolled?: boolean;
  }) => Promise<SwWindowClientLike[]>;
  openWindow?: (url: string) => Promise<SwWindowClientLike | null>;
}

interface SwGlobalLike {
  location: { origin: string };
  clients: SwClientsLike;
  registration: {
    showNotification: (
      title: string,
      options?: {
        body?: string;
        icon?: string;
        badge?: string;
        tag?: string;
        data?: { url: string };
      }
    ) => Promise<void>;
  };
  skipWaiting: () => Promise<void>;
  addEventListener: (type: string, listener: (event: any) => void) => void;
}

const CACHE_NAME = "coda-static-__CODA_SW_CACHE_VERSION__";
const swSelf = self as unknown as SwGlobalLike;

swSelf.addEventListener("install", (event: { waitUntil: (p: Promise<unknown>) => void }) => {
  event.waitUntil(swSelf.skipWaiting());
});

swSelf.addEventListener("activate", (event: { waitUntil: (p: Promise<unknown>) => void }) => {
  event.waitUntil(
    (async () => {
      try {
        if (typeof caches !== "undefined") {
          const keys = await caches.keys();
          await Promise.all(
            keys.filter((k) => k !== CACHE_NAME).map((k) => caches.delete(k))
          );
        }
      } catch {
        // ignore cache prune errors
      }
      await swSelf.clients.claim();
    })()
  );
});

swSelf.addEventListener(
  "fetch",
  (event: { request: Request; respondWith: (p: Promise<Response>) => void }) => {
    const req = event.request;
    if (!shouldSwCacheStaticAsset(req.url, swSelf.location.origin, req.method)) {
      return;
    }
    event.respondWith(
      (async () => {
        try {
          const cache = await caches.open(CACHE_NAME);
          const cached = await cache.match(req);
          if (cached) {
            return cached;
          }
          const resp = await fetch(req);
          if (resp && resp.ok && resp.status === 200) {
            try {
              await cache.put(req, resp.clone());
            } catch {
              // ignore quota errors
            }
          }
          return resp;
        } catch {
          return fetch(req);
        }
      })()
    );
  }
);

swSelf.addEventListener(
  "push",
  (event: {
    data?: { text?: () => string; json?: () => unknown } | null;
    waitUntil: (p: Promise<unknown>) => void;
  }) => {
    let raw = "";
    if (event.data) {
      if (typeof event.data.text === "function") {
        try {
          raw = event.data.text();
        } catch {
          raw = "";
        }
      } else if (typeof event.data.json === "function") {
        try {
          raw = JSON.stringify(event.data.json());
        } catch {
          raw = "";
        }
      }
    }
    const payload = parseSwPushPayload(raw, swSelf.location.origin);
    event.waitUntil(
      swSelf.registration.showNotification(payload.title || "Coda Agent Alert", {
        body: payload.body,
        icon: "/apple-touch-icon.png",
        badge: "/logo.svg",
        tag: payload.tag || "coda-alert",
        data: { url: payload.url || "/console" },
      })
    );
  }
);

swSelf.addEventListener(
  "notificationclick",
  (event: {
    notification: { close: () => void; data?: { url?: unknown } };
    waitUntil: (p: Promise<unknown>) => void;
  }) => {
    event.notification.close();
    const targetUrl = normalizeSwNotificationTargetUrl(
      event.notification?.data?.url,
      swSelf.location.origin
    );
    event.waitUntil(
      (async () => {
        const winClients = await swSelf.clients.matchAll({
          type: "window",
          includeUncontrolled: true,
        });
        for (const client of winClients) {
          try {
            const cu = new URL(client.url);
            if (cu.origin === swSelf.location.origin && cu.pathname.startsWith("/console")) {
              if (typeof client.focus === "function") {
                await client.focus();
                return;
              }
            }
          } catch {
            // ignore malformed URL
          }
        }
        for (const client of winClients) {
          try {
            const cu = new URL(client.url);
            if (cu.origin === swSelf.location.origin) {
              if (typeof client.navigate === "function") {
                await client.navigate(targetUrl);
              }
              if (typeof client.focus === "function") {
                await client.focus();
                return;
              }
            }
          } catch {
            // ignore malformed URL
          }
        }
        if (typeof swSelf.clients.openWindow === "function") {
          await swSelf.clients.openWindow(targetUrl);
        }
      })()
    );
  }
);
