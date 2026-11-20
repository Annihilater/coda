export type IconName =
  | "terminal"
  | "laptop"
  | "server"
  | "smartphone"
  | "cloud"
  | "shield"
  | "zap"
  | "key"
  | "copy"
  | "check"
  | "sun"
  | "moon"
  | "monitor"
  | "search"
  | "settings"
  | "sidebar"
  | "paperclip"
  | "clipboard"
  | "mic"
  | "keyboard"
  | "plus"
  | "arrowRight"
  | "close"
  | "chevronLeft"
  | "chevronRight"
  | "more"
  | "apple"
  | "arrowLeft"
  | "mail"
  | "user"
  | "lock"
  | "eye"
  | "eyeOff"
  | "gitBranch"
  | "layers"
  | "cpu"
  | "globe"
  | "bell";

export function svgIcon(name: IconName, size = 14): string {
  const attrs = `width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" class="pm-icon" aria-hidden="true"`;
  switch (name) {
    case "terminal":
      return `<svg ${attrs}><polyline points="4 17 10 11 4 5"></polyline><line x1="12" y1="19" x2="20" y2="19"></line></svg>`;
    case "laptop":
      return `<svg ${attrs}><rect x="3" y="4" width="18" height="12" rx="2"></rect><line x1="2" y1="20" x2="22" y2="20"></line></svg>`;
    case "server":
      return `<svg ${attrs}><rect x="2" y="3" width="20" height="8" rx="2"></rect><rect x="2" y="13" width="20" height="8" rx="2"></rect><line x1="6" y1="7" x2="6.01" y2="7"></line><line x1="6" y1="17" x2="6.01" y2="17"></line></svg>`;
    case "smartphone":
      return `<svg ${attrs}><rect x="6" y="2" width="12" height="20" rx="2"></rect><line x1="12" y1="18" x2="12.01" y2="18"></line></svg>`;
    case "cloud":
      return `<svg ${attrs}><path d="M17.5 19H7a5 5 0 0 1-.5-9.97A6 6 0 0 1 18.2 10a4.5 4.5 0 0 1-.7 9Z"></path></svg>`;
    case "shield":
      return `<svg ${attrs}><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"></path></svg>`;
    case "zap":
      return `<svg ${attrs}><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon></svg>`;
    case "key":
      return `<svg ${attrs}><circle cx="7.5" cy="15.5" r="5.5"></circle><path d="m21 2-9.6 9.6"></path><path d="m15.5 7.5 3 3L22 7l-3-3"></path></svg>`;
    case "copy":
      return `<svg ${attrs}><rect x="9" y="9" width="13" height="13" rx="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>`;
    case "check":
      return `<svg ${attrs}><polyline points="20 6 9 17 4 12"></polyline></svg>`;
    case "sun":
      return `<svg ${attrs}><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"></path></svg>`;
    case "moon":
      return `<svg ${attrs}><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"></path></svg>`;
    case "monitor":
      return `<svg ${attrs}><rect x="2" y="3" width="20" height="14" rx="2"></rect><line x1="8" y1="21" x2="16" y2="21"></line><line x1="12" y1="17" x2="12" y2="21"></line></svg>`;
    case "search":
      return `<svg ${attrs}><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>`;
    case "settings":
      return `<svg ${attrs}><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>`;
    case "sidebar":
      return `<svg ${attrs}><rect x="3" y="3" width="18" height="18" rx="2"></rect><line x1="9" y1="3" x2="9" y2="21"></line></svg>`;
    case "paperclip":
      return `<svg ${attrs}><path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"></path></svg>`;
    case "clipboard":
      return `<svg ${attrs}><rect x="9" y="2" width="6" height="4" rx="1"></rect><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"></path></svg>`;
    case "mic":
      return `<svg ${attrs}><path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"></path><path d="M19 10v2a7 7 0 0 1-14 0v-2"></path><line x1="12" y1="19" x2="12" y2="22"></line></svg>`;
    case "keyboard":
      return `<svg ${attrs}><rect x="2" y="4" width="20" height="16" rx="2"></rect><path d="M6 8h.01M10 8h.01M14 8h.01M18 8h.01M8 12h.01M12 12h.01M16 12h.01M7 16h10"></path></svg>`;
    case "plus":
      return `<svg ${attrs}><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>`;
    case "arrowRight":
      return `<svg ${attrs}><line x1="5" y1="12" x2="19" y2="12"></line><polyline points="12 5 19 12 12 19"></polyline></svg>`;
    case "arrowLeft":
      return `<svg ${attrs}><line x1="19" y1="12" x2="5" y2="12"></line><polyline points="12 19 5 12 12 5"></polyline></svg>`;
    case "close":
      return `<svg ${attrs}><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>`;
    case "chevronLeft":
      return `<svg ${attrs}><polyline points="15 18 9 12 15 6"></polyline></svg>`;
    case "chevronRight":
      return `<svg ${attrs}><polyline points="9 18 15 12 9 6"></polyline></svg>`;
    case "more":
      return `<svg ${attrs}><circle cx="12" cy="12" r="1"></circle><circle cx="19" cy="12" r="1"></circle><circle cx="5" cy="12" r="1"></circle></svg>`;
    case "mail":
      return `<svg ${attrs}><rect x="2" y="4" width="20" height="16" rx="2"></rect><path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"></path></svg>`;
    case "user":
      return `<svg ${attrs}><path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"></path><circle cx="12" cy="7" r="4"></circle></svg>`;
    case "lock":
      return `<svg ${attrs}><rect x="3" y="11" width="18" height="11" rx="2"></rect><path d="M7 11V7a5 5 0 0 1 10 0v4"></path></svg>`;
    case "eye":
      return `<svg ${attrs}><path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z"></path><circle cx="12" cy="12" r="3"></circle></svg>`;
    case "eyeOff":
      return `<svg ${attrs}><path d="M9.88 9.88a3 3 0 1 0 4.24 4.24"></path><path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68"></path><path d="M6.61 6.61A13.526 13.526 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61"></path><line x1="2" y1="2" x2="22" y2="22"></line></svg>`;
    case "gitBranch":
      return `<svg ${attrs}><line x1="6" y1="3" x2="6" y2="15"></line><circle cx="18" cy="6" r="3"></circle><circle cx="6" cy="18" r="3"></circle><path d="M18 9a9 9 0 0 1-9 9"></path></svg>`;
    case "layers":
      return `<svg ${attrs}><polygon points="12 2 2 7 12 12 22 7 12 2"></polygon><polyline points="2 17 12 22 22 17"></polyline><polyline points="2 12 12 17 22 12"></polyline></svg>`;
    case "cpu":
      return `<svg ${attrs}><rect x="4" y="4" width="16" height="16" rx="2"></rect><rect x="9" y="9" width="6" height="6"></rect><path d="M15 2v2M15 20v2M2 15h2M2 9h2M20 15h2M20 9h2M9 2v2M9 20v2"></path></svg>`;
    case "globe":
      return `<svg ${attrs}><circle cx="12" cy="12" r="10"></circle><line x1="2" y1="12" x2="22" y2="12"></line><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"></path></svg>`;
    case "bell":
      return `<svg ${attrs}><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"></path><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"></path></svg>`;
    case "apple":
      return `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="currentColor" class="pm-icon" aria-hidden="true"><path d="M16.36 1.64a4.54 4.54 0 0 1-1.08 3.3 3.92 3.92 0 0 1-3.13 1.44 4.24 4.24 0 0 1 1.1-3.2A4.48 4.48 0 0 1 16.36 1.64ZM20.28 17.2a9.9 9.9 0 0 1-1.15 2.08c-.65.94-1.45 2.07-2.58 2.09-1.06.02-1.4-.63-2.61-.63-1.22 0-1.6.61-2.63.65-1.09.04-1.92-1.06-2.58-2-1.76-2.53-2.78-6.48-1.16-9.29a4.68 4.68 0 0 1 3.95-2.4c1.14-.02 2.21.77 2.9.77.69 0 1.98-.95 3.34-.81.57.02 2.16.23 3.18 1.72a4.38 4.38 0 0 0-2.09 3.69 4.28 4.28 0 0 0 2.61 3.91c-.08.28-.14.54-.23.82Z"/></svg>`;
  }
}

export function codaLogoSVG(size = 22): string {
  return `<svg width="${size}" height="${size}" viewBox="0 0 64 64" fill="none" aria-hidden="true" style="flex-shrink:0;">
    <rect x="2" y="2" width="60" height="60" rx="14" fill="#141416" stroke="#2e2e34" stroke-width="2" />
    <path d="M 44.37 44.37 A 17.5 17.5 0 1 1 38.70 15.83" stroke="#ffffff" stroke-width="5.2" stroke-linecap="round" />
    <path d="M 25.5 24.5 L 34 32 L 25.5 39.5" stroke="#a1a1aa" stroke-width="4.4" stroke-linecap="round" stroke-linejoin="round" />
    <circle cx="46" cy="19" r="4.2" fill="#22c55e" />
  </svg>`;
}

export function codaNodeDotSVG(color = "#e06c50"): string {
  return `<span style="display:inline-flex;align-items:center;justify-content:center;width:16px;height:16px;border-radius:4px;border:1px solid var(--border-primary);background:var(--bg-elevated);flex-shrink:0;">
    <span style="width:6px;height:6px;border-radius:99px;background:${color};"></span>
  </span>`;
}

// Bento UI Motion timing curves (strictly from-rest & critically damped)
export const clamp01 = (v: number): number => Math.max(0, Math.min(1, v));
const SPRING_W0 = 9.5;
const SPRING_ZETA = 0.55;
const SPRING_WD = SPRING_W0 * Math.sqrt(1 - SPRING_ZETA * SPRING_ZETA);
const SPRING_NORM =
  1 -
  Math.exp(-SPRING_ZETA * SPRING_W0) *
    (Math.cos(SPRING_WD) + (SPRING_ZETA / Math.sqrt(1 - SPRING_ZETA * SPRING_ZETA)) * Math.sin(SPRING_WD));

export function springFromRest(p: number): number {
  const t = clamp01(p);
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  const raw =
    1 -
    Math.exp(-SPRING_ZETA * SPRING_W0 * t) *
      (Math.cos(SPRING_WD * t) +
        (SPRING_ZETA / Math.sqrt(1 - SPRING_ZETA * SPRING_ZETA)) * Math.sin(SPRING_WD * t));
  return raw / SPRING_NORM;
}

const SOFT_NORM = 1 - (1 + 6) * Math.exp(-6);
export function soft(p: number): number {
  const t = clamp01(p);
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  return (1 - (1 + 6 * t) * Math.exp(-6 * t)) / SOFT_NORM;
}
