import { BrowserE2EESession, loadOrCreateWebIdentity, signWebMessage } from "./crypto";

interface User {
  id: string;
  email: string;
  handle: string;
  display_name: string;
  is_admin: boolean;
  bandwidth_quota_mbps: number;
}

interface EndpointCandidate {
  kind: string;
  addr: string;
}

interface Device {
  id: string;
  name: string;
  role: string;
  platform: string;
  x25519_pubkey: string;
  endpoints_json?: EndpointCandidate[];
  herdr_version?: string | null;
  is_online: boolean;
}

interface HerdrTerminal {
  address: string;
  name?: string | null;
  argv: string[];
  cwd: string;
  alive: boolean;
  status?: {
    state: string;
    kind?: string | null;
    app?: string | null;
    title?: string | null;
    msg?: string | null;
  } | null;
}

interface BranchWorktreeItem {
  branch: string;
  worktree_path: string;
  diff_added: number;
  diff_deleted: number;
  active_terminals: HerdrTerminal[];
  status_summary: string;
}

interface RepoWorkspaceSummary {
  repo_name: string;
  root_path: string;
  machine_name: string;
  branches: BranchWorktreeItem[];
}

interface VoiceModelConfig {
  provider: string;
  base_url: string;
  api_key: string;
  model: string;
}

interface SiteInfo {
  version: string;
  open_registration: boolean;
  has_any_users: boolean;
  public_base_url: string;
  install_command: string;
  downloads: {
    daemon_linux_x64: string;
    daemon_linux_arm64: string;
    daemon_macos_arm64: string;
    server_linux_x64: string;
    server_linux_arm64: string;
    server_macos_arm64: string;
    desktop_macos_dmg: string;
    desktop_macos_tar: string;
    deploy_bundle: string;
    ios_beta_url?: string | null;
    releases_page: string;
  };
}

interface DeviceAuthPublicInfo {
  user_code: string;
  device_name: string;
  role: string;
  platform: string;
  status: string;
  expires_at: string;
}

type ThemePreference = "light" | "dark" | "system";
type Locale = "en" | "zh";
type IconName =
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
  | "globe";

function svgIcon(name: IconName, size = 14): string {
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
    case "apple":
      return `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="currentColor" class="pm-icon" aria-hidden="true"><path d="M16.36 1.64a4.54 4.54 0 0 1-1.08 3.3 3.92 3.92 0 0 1-3.13 1.44 4.24 4.24 0 0 1 1.1-3.2A4.48 4.48 0 0 1 16.36 1.64ZM20.28 17.2a9.9 9.9 0 0 1-1.15 2.08c-.65.94-1.45 2.07-2.58 2.09-1.06.02-1.4-.63-2.61-.63-1.22 0-1.6.61-2.63.65-1.09.04-1.92-1.06-2.58-2-1.76-2.53-2.78-6.48-1.16-9.29a4.68 4.68 0 0 1 3.95-2.4c1.14-.02 2.21.77 2.9.77.69 0 1.98-.95 3.34-.81.57.02 2.16.23 3.18 1.72a4.38 4.38 0 0 0-2.09 3.69 4.28 4.28 0 0 0 2.61 3.91c-.08.28-.14.54-.23.82Z"/></svg>`;
  }
}

function codaLogoSVG(size = 22) {
  return `<svg width="${size}" height="${size}" viewBox="0 0 64 64" fill="none" aria-hidden="true" style="flex-shrink:0;">
    <rect x="2" y="2" width="60" height="60" rx="14" fill="#141416" stroke="#2e2e34" stroke-width="2" />
    <path d="M 44.37 44.37 A 17.5 17.5 0 1 1 38.70 15.83" stroke="#ffffff" stroke-width="5.2" stroke-linecap="round" />
    <path d="M 25.5 24.5 L 34 32 L 25.5 39.5" stroke="#a1a1aa" stroke-width="4.4" stroke-linecap="round" stroke-linejoin="round" />
    <circle cx="46" cy="19" r="4.2" fill="#22c55e" />
  </svg>`;
}

function codaNodeDotSVG(color = "#e06c50") {
  return `<span style="display:inline-flex;align-items:center;justify-content:center;width:16px;height:16px;border-radius:4px;border:1px solid var(--border-primary);background:var(--bg-elevated);flex-shrink:0;">
    <span style="width:6px;height:6px;border-radius:99px;background:${color};"></span>
  </span>`;
}

// Bento UI Motion timing curves (strictly from-rest & critically damped)
const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
const SPRING_W0 = 9.5;
const SPRING_ZETA = 0.55;
const SPRING_WD = SPRING_W0 * Math.sqrt(1 - SPRING_ZETA * SPRING_ZETA);
const SPRING_NORM =
  1 -
  Math.exp(-SPRING_ZETA * SPRING_W0) *
    (Math.cos(SPRING_WD) + (SPRING_ZETA / Math.sqrt(1 - SPRING_ZETA * SPRING_ZETA)) * Math.sin(SPRING_WD));

function springFromRest(p: number): number {
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
function soft(p: number): number {
  const t = clamp01(p);
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  return (1 - (1 + 6 * t) * Math.exp(-6 * t)) / SOFT_NORM;
}

type AppRoute = "home" | "login" | "register" | "activate" | "console";

class CodaTerminalWebApp {
  private token: string | null = localStorage.getItem("coda_token");
  private themePref: ThemePreference = (localStorage.getItem("coda_theme") as ThemePreference) || "dark";
  private locale: Locale = this.resolveInitialLocale();
  private route: AppRoute = "home";
  private authMode: "login" | "register" = "login";
  private heroNode: "mac" | "linux" | "iphone" = "mac";
  private previewTab: "coda" | "server" | "ios" = "coda";
  private showPassword = false;
  private showMobileKeypadOnDesktop = false;
  private showCmdPalette = false;
  private siteInfo: SiteInfo | null = null;
  private pendingActivationCode: string = "";
  private pendingDeviceInfo: DeviceAuthPublicInfo | null = null;
  private activationSuccessMsg = "";
  private copiedToast: "" | "ok" | "manual" = "";
  private user: User | null = null;
  private devices: Device[] = [];
  private myClientDevice: Device | null = null;
  private selectedDaemon: Device | null = null;
  private terminals: HerdrTerminal[] = [];
  private repos: RepoWorkspaceSummary[] = [];
  private voiceConfig: VoiceModelConfig = {
    provider: "openai",
    base_url: "https://api.openai.com/v1",
    api_key: "",
    model: "gpt-4o-mini-transcribe",
  };
  private activeTerminalAddr: string | null = null;
  private terminalScreen = "";
  private draftInput = "";
  private showSidebar = true;
  private showSettingsModal = false;
  private showKeyboardInputBar = false;
  private pairCommand: string | null = null;
  private latestInviteCode: string | null = null;
  private rttMs: number | null = null;
  private ws: WebSocket | null = null;
  private directWs: WebSocket | null = null;
  private directWsPeerId: string | null = null;
  private activeTransportMode: "lan_ws" | "relay_ws" = "relay_ws";
  private negotiatingPeerId: string | null = null;
  private e2eeSessions = new Map<string, BrowserE2EESession>();
  private directE2eeSessions = new Map<string, BrowserE2EESession>();
  private webIdentity = loadOrCreateWebIdentity();
  private errorMsg = "";

  // Bento motion scheduler state (never triggers full DOM re-render per frame)
  private bentoRafId: number | null = null;
  private bentoObserver: IntersectionObserver | null = null;
  private bentoVisible = true;
  private bentoStartMs = performance.now();

  private resolveRouteFromLocation(): AppRoute {
    const pathname = window.location.pathname.replace(/\/+$/, "") || "/";
    if (pathname === "/login") return "login";
    if (pathname === "/register") return "register";
    if (pathname === "/activate") return "activate";
    if (pathname === "/console") return "console";
    const codeParam = new URLSearchParams(window.location.search).get("code");
    if (pathname === "/" && codeParam && codeParam.trim().length > 0) {
      return "activate";
    }
    return "home";
  }

  private navigate(target: string, replace = false) {
    const url = new URL(target, window.location.origin);
    const nextUrl = `${url.pathname}${url.search}${url.hash}`;
    if (replace) {
      window.history.replaceState({}, "", nextUrl);
    } else {
      window.history.pushState({}, "", nextUrl);
    }

    this.route = this.resolveRouteFromLocation();
    if (this.route === "login") this.authMode = "login";
    if (this.route === "register") this.authMode = "register";
    this.errorMsg = "";
    this.showCmdPalette = false;

    const codeParam = (url.searchParams.get("code") || "").trim().toUpperCase();
    if (codeParam && codeParam !== this.pendingActivationCode) {
      this.pendingActivationCode = codeParam;
      void this.lookupActivationCode(codeParam).then(() => this.render());
    }

    if (url.hash && this.route === "home") {
      this.render();
      requestAnimationFrame(() => {
        document.querySelector(url.hash)?.scrollIntoView({ behavior: "smooth" });
      });
      return;
    }

    window.scrollTo(0, 0);
    this.render();
  }

  private resolveInitialLocale(): Locale {
    try {
      const qLang = new URLSearchParams(window.location.search).get("lang");
      if (qLang === "zh" || qLang === "zh-CN") {
        localStorage.setItem("coda_locale", "zh");
        return "zh";
      }
      if (qLang === "en") {
        localStorage.setItem("coda_locale", "en");
        return "en";
      }
      return localStorage.getItem("coda_locale") === "zh" ? "zh" : "en";
    } catch {
      return "en";
    }
  }

  private tr(en: string, zh: string): string {
    return this.locale === "zh" ? zh : en;
  }

  private syncDocumentLocaleMeta() {
    document.documentElement.lang = this.locale === "zh" ? "zh-CN" : "en";
    document.title = this.tr(
      "Coda — Zero-Knowledge Cross-Device AI Coding Workbench",
      "Coda — 端到端零知识加密的跨设备 AI 编程工作台"
    );
    document
      .getElementById("meta-desc")
      ?.setAttribute(
        "content",
        this.tr(
          "Connect your Linux servers, macOS workstation, and iPhone/Web into one zero-knowledge encrypted mesh. Persistent herdr-core PTYs, sub-millisecond LAN P2P, and zero exposed ports via Cloudflare Tunnel.",
          "Coda 将你的 Linux 云服务器、Mac 桌面工作站与随身 iPhone/Web 接入同一个零知识加密网络。常驻 herdr-core 守护、Wi-Fi 毫秒级直连、Cloudflare Tunnel 零公网端口暴露。"
        )
      );
  }

  private applyLocale(loc: Locale, persist = true) {
    const draftValues: Record<string, string> = {};
    document.querySelectorAll<HTMLInputElement>("#auth-form input[name]").forEach((inp) => {
      if (inp.name) draftValues[`auth:${inp.name}`] = inp.value;
    });
    const activateInp = document.querySelector<HTMLInputElement>('#activate-lookup-form input[name="code"]');
    if (activateInp) {
      draftValues["activate:code"] = activateInp.value;
    }

    this.locale = loc;
    if (persist) {
      try {
        localStorage.setItem("coda_locale", loc);
      } catch {
        // ignore storage errors
      }
    }
    this.syncDocumentLocaleMeta();
    this.render();

    document.querySelectorAll<HTMLInputElement>("#auth-form input[name]").forEach((inp) => {
      const k = `auth:${inp.name}`;
      if (inp.name && k in draftValues) inp.value = draftValues[k];
    });
    const nextActivateInp = document.querySelector<HTMLInputElement>('#activate-lookup-form input[name="code"]');
    if (nextActivateInp && "activate:code" in draftValues) {
      nextActivateInp.value = draftValues["activate:code"];
    }
  }

  async init() {
    this.syncDocumentLocaleMeta();
    this.applyTheme(this.themePref, false);
    this.route = this.resolveRouteFromLocation();
    if (this.route === "register") {
      this.authMode = "register";
    } else if (this.route === "login") {
      this.authMode = "login";
    }

    window.matchMedia?.("(prefers-color-scheme: light)")?.addEventListener("change", () => {
      if (this.themePref === "system") {
        this.applyTheme("system", false);
      }
    });

    window.addEventListener("popstate", () => {
      this.route = this.resolveRouteFromLocation();
      if (this.route === "login") this.authMode = "login";
      if (this.route === "register") this.authMode = "register";
      this.errorMsg = "";
      this.showCmdPalette = false;
      const codeParam = (new URLSearchParams(window.location.search).get("code") || "").trim().toUpperCase();
      if (codeParam && codeParam !== this.pendingActivationCode) {
        this.pendingActivationCode = codeParam;
        void this.lookupActivationCode(codeParam).then(() => this.render());
      } else {
        this.render();
      }
    });

    window.addEventListener("keydown", (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k" && this.route !== "console") {
        e.preventDefault();
        this.showCmdPalette = !this.showCmdPalette;
        this.render();
      } else if (e.key === "Escape" && this.showCmdPalette) {
        this.showCmdPalette = false;
        this.render();
      }
    });

    const params = new URLSearchParams(window.location.search);
    const codeParam = params.get("code") || "";
    if (this.route === "activate" || codeParam) {
      this.pendingActivationCode = codeParam.trim().toUpperCase();
    }

    window.addEventListener("online", () => {
      if (this.selectedDaemon?.is_online) {
        void this.tryNegotiateDirectLanWs(this.selectedDaemon);
      }
    });
    window.addEventListener("resize", () => {
      if (this.selectedDaemon && this.activeTerminalAddr) {
        const cols = Math.max(48, Math.floor((window.innerWidth - (this.showSidebar ? 390 : 80)) / 8.2));
        const rows = Math.max(16, Math.floor((window.innerHeight - 210) / 20));
        this.sendEncrypted(this.selectedDaemon, {
          op: "pty_resize",
          terminal: this.activeTerminalAddr,
          cols,
          rows,
        });
      }
    });

    try {
      this.siteInfo = await this.api("/api/v1/public/site-info");
    } catch {
      this.siteInfo = null;
    }

    if (this.pendingActivationCode) {
      await this.lookupActivationCode(this.pendingActivationCode);
    }

    if (this.token) {
      try {
        this.user = await this.api("/api/v1/auth/me");
        await this.bootstrapMesh();
        if (this.route === "login" || this.route === "register") {
          const target = this.pendingActivationCode
            ? `/activate?code=${encodeURIComponent(this.pendingActivationCode)}`
            : "/console";
          window.history.replaceState({}, "", target);
          this.route = this.resolveRouteFromLocation();
        }
      } catch {
        this.token = null;
        localStorage.removeItem("coda_token");
      }
    }
    this.render();
  }

  private applyTheme(pref: ThemePreference, persist = true) {
    this.themePref = pref;
    if (persist) {
      localStorage.setItem("coda_theme", pref);
    }
    const resolved =
      pref === "system"
        ? window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches
          ? "light"
          : "dark"
        : pref;
    document.documentElement.dataset.theme = resolved;
    document.documentElement.style.colorScheme = resolved;
    document.querySelectorAll("[data-set-theme]").forEach((btn) => {
      const target = (btn as HTMLElement).dataset.setTheme;
      btn.classList.toggle("active", target === pref);
    });
  }

  private stopBentoLoop() {
    if (this.bentoRafId !== null) {
      cancelAnimationFrame(this.bentoRafId);
      this.bentoRafId = null;
    }
    if (this.bentoObserver) {
      this.bentoObserver.disconnect();
      this.bentoObserver = null;
    }
  }

  private startBentoLoop() {
    this.stopBentoLoop();
    const stage = document.getElementById("bento-stage");
    if (!stage || this.route !== "home") return;

    const glider = stage.querySelector<HTMLElement>('[data-bento="route-glider"]');
    const rttEl = stage.querySelector<HTMLElement>('[data-bento="rtt-val"]');
    const badgeLan = stage.querySelector<HTMLElement>('[data-bento="badge-lan"]');
    const badgeWss = stage.querySelector<HTMLElement>('[data-bento="badge-wss"]');
    const sparkLan = stage.querySelector<SVGPathElement>('[data-bento="spark-lan"]');
    const sparkWss = stage.querySelector<SVGPathElement>('[data-bento="spark-wss"]');
    const sealRing = stage.querySelector<SVGCircleElement>('[data-bento="seal-ring"]');
    const sealPlain = stage.querySelector<HTMLElement>('[data-bento="seal-plain"]');
    const sealCipher = stage.querySelector<HTMLElement>('[data-bento="seal-cipher"]');
    const sealPct = stage.querySelector<HTMLElement>('[data-bento="seal-pct"]');
    const sealSteps = Array.from(stage.querySelectorAll<HTMLElement>("[data-seal-step]"));
    const otpCells = Array.from(stage.querySelectorAll<HTMLElement>("[data-otp-idx]"));
    const otpPending = stage.querySelector<HTMLElement>('[data-bento="otp-pending"]');
    const otpApproved = stage.querySelector<HTMLElement>('[data-bento="otp-approved"]');
    const ptyLines = Array.from(stage.querySelectorAll<HTMLElement>("[data-pty-idx]"));
    const pktA = stage.querySelector<SVGCircleElement>('[data-bento="pkt-a"]');
    const pktB = stage.querySelector<SVGCircleElement>('[data-bento="pkt-b"]');
    const voiceBars = Array.from(stage.querySelectorAll<HTMLElement>("[data-voice-idx]"));
    const reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const CIRC = 113.1;

    const paintFrame = (nowMs: number) => {
      const l = reducedMotion ? 1.5 : ((nowMs - this.bentoStartMs) / 1000) % 16;

      // 1. SmartPath Route Glider + Sparkline Crossfade (0..1 -> holds -> 1..0 cleanly back to 0 at 16s)
      let routeT = 0;
      if (l >= 1.5 && l < 2.9) {
        routeT = springFromRest((l - 1.5) / 1.4);
      } else if (l >= 2.9 && l < 9.0) {
        routeT = 1;
      } else if (l >= 9.0 && l < 10.4) {
        routeT = 1 - springFromRest((l - 9.0) / 1.4);
      }
      const crossFade = clamp01(routeT);
      if (glider) glider.style.transform = `translate3d(${routeT * 100}%, 0, 0)`;
      if (rttEl) rttEl.textContent = `${(0.8 + crossFade * 25.6).toFixed(1)} ms`;
      if (badgeLan) badgeLan.style.opacity = `${1 - soft(crossFade)}`;
      if (badgeWss) badgeWss.style.opacity = `${soft(crossFade)}`;
      if (sparkLan) sparkLan.style.opacity = `${(1 - soft(crossFade) * 0.72).toFixed(3)}`;
      if (sparkWss) sparkWss.style.opacity = `${(0.28 + soft(crossFade) * 0.72).toFixed(3)}`;

      // 2. Zero-Knowledge E2EE Seal Ring + 3-Stage Pipeline Highlight
      let sealP = 0;
      if (l >= 3.0 && l < 4.5) {
        sealP = springFromRest((l - 3.0) / 1.5);
      } else if (l >= 4.5 && l < 12.2) {
        sealP = 1;
      } else if (l >= 12.2 && l < 13.7) {
        sealP = 1 - soft((l - 12.2) / 1.5);
      }
      const clampedSeal = clamp01(sealP);
      if (sealRing) sealRing.style.strokeDashoffset = `${CIRC * (1 - clampedSeal)}`;
      if (sealPlain) sealPlain.style.opacity = `${1 - soft(clampedSeal)}`;
      if (sealCipher) sealCipher.style.opacity = `${soft(clampedSeal)}`;
      if (sealPct) sealPct.textContent = `${Math.round(clampedSeal * 100)}%`;
      sealSteps.forEach((step, idx) => {
        const active = clampedSeal >= idx * 0.34 + 0.05;
        step.style.borderColor = active ? (idx === 2 ? "var(--emerald-bd)" : "var(--signal-blue-bd)") : "var(--border-primary)";
        step.style.color = active ? (idx === 2 ? "var(--emerald)" : "var(--text-primary)") : "var(--text-tertiary)";
      });

      // 3. RFC 8628 Device Code sequential reveal + approval state
      let approvedP = 0;
      if (l >= 7.2 && l < 8.4) {
        approvedP = soft((l - 7.2) / 1.2);
      } else if (l >= 8.4 && l < 13.5) {
        approvedP = 1;
      } else if (l >= 13.5 && l < 14.8) {
        approvedP = 1 - soft((l - 13.5) / 1.3);
      }
      if (otpPending) otpPending.style.opacity = `${1 - approvedP}`;
      if (otpApproved) otpApproved.style.opacity = `${approvedP}`;
      otpCells.forEach((cell, idx) => {
        const focusStart = 1.0 + idx * 0.7;
        const focusEnd = focusStart + 0.7;
        const dist =
          l >= focusStart && l <= focusEnd
            ? 1 - Math.abs(l - (focusStart + 0.35)) / 0.35
            : 0;
        const isLit = dist > 0.1 || approvedP > 0.5;
        cell.style.borderColor = isLit ? "var(--signal-blue)" : "var(--border-primary)";
        cell.style.transform = `translate3d(0, ${-2 * Math.max(0, dist)}px, 0)`;
      });

      // 4. Live PTY Stream subtle active line pulse
      ptyLines.forEach((line, idx) => {
        const center = 2.5 + idx * 4.5;
        const delta = Math.abs(l - center);
        const w = delta < 2.2 ? soft(1 - delta / 2.2) : 0;
        line.style.backgroundColor = `rgba(255, 255, 255, ${(0.03 + 0.06 * w).toFixed(3)})`;
        line.style.opacity = `${(0.72 + 0.28 * w).toFixed(3)}`;
      });

      // 5. Encrypted Mesh Topology Packet Dots (periodic 16s loop)
      const phaseA = 0.5 - 0.5 * Math.cos((2 * Math.PI * l) / 8);
      const phaseB = 0.5 - 0.5 * Math.cos((2 * Math.PI * ((l + 4) % 16)) / 8);
      if (pktA) {
        const ax = 54 + phaseA * 102;
        const ay = 29 - Math.sin(phaseA * Math.PI) * 14;
        pktA.setAttribute("cx", ax.toFixed(1));
        pktA.setAttribute("cy", ay.toFixed(1));
      }
      if (pktB) {
        const bx = 190 + phaseB * 102;
        const by = 29 + Math.sin(phaseB * Math.PI) * 14;
        pktB.setAttribute("cx", bx.toFixed(1));
        pktB.setAttribute("cy", by.toFixed(1));
      }

      // 6. Voice-to-Terminal periodic 16s-locked bars (exact l=0 == l=16 continuity)
      const speechEnv = 0.28 + 0.72 * (0.5 - 0.5 * Math.cos((2 * Math.PI * l) / 16));
      voiceBars.forEach((bar, idx) => {
        const harmonic = 0.5 + 0.5 * Math.sin(((3 + (idx % 3)) * 2 * Math.PI * l) / 16 + idx * 0.9);
        const sy = 0.2 + 0.8 * speechEnv * harmonic;
        bar.style.transform = `scaleY(${sy.toFixed(3)})`;
        bar.style.opacity = `${(0.45 + 0.55 * sy).toFixed(3)}`;
      });
    };

    paintFrame(performance.now());
    if (reducedMotion) return;

    this.bentoObserver = new IntersectionObserver(
      (entries) => {
        const visible = entries.some((entry) => entry.isIntersecting);
        this.bentoVisible = visible;
        if (visible && this.bentoRafId === null && this.route === "home") {
          this.bentoRafId = requestAnimationFrame(tick);
        }
      },
      { threshold: 0.08 }
    );
    this.bentoObserver.observe(stage);

    const tick = (now: number) => {
      if (!this.bentoVisible || this.route !== "home") {
        this.bentoRafId = null;
        return;
      }
      paintFrame(now);
      this.bentoRafId = requestAnimationFrame(tick);
    };
    this.bentoRafId = requestAnimationFrame(tick);
  }

  private async lookupActivationCode(code: string) {
    const cleaned = code.trim().toUpperCase();
    this.pendingActivationCode = cleaned;
    if (!cleaned) {
      this.pendingDeviceInfo = null;
      return;
    }
    try {
      this.pendingDeviceInfo = await this.api(
        `/api/v1/auth/device/info?code=${encodeURIComponent(cleaned)}`
      );
      this.errorMsg = "";
    } catch (err: any) {
      this.pendingDeviceInfo = null;
      this.errorMsg = err.message || this.tr("Invalid or expired authorization code", "授权码无效或已过期");
    }
  }

  private async api(path: string, method = "GET", body?: unknown): Promise<any> {
    const headers: Record<string, string> = { "Content-Type": "application/json" };
    if (this.token) headers["Authorization"] = `Bearer ${this.token}`;
    const resp = await fetch(path, {
      method,
      headers,
      body: body ? JSON.stringify(body) : undefined,
    });
    const data = await resp.json();
    if (!resp.ok) throw new Error(data.error || `HTTP ${resp.status}`);
    return data;
  }

  private async bootstrapMesh() {
    this.myClientDevice = await this.api("/api/v1/devices/register-client", "POST", {
      name: this.webIdentity.name,
      platform: "web",
      ed25519_pubkey: this.webIdentity.ed25519PublicB64,
      x25519_pubkey: this.webIdentity.x25519PublicB64,
    });
    await this.refreshDevices();
    await this.connectRelayWebSocket();
  }

  private async refreshDevices() {
    const all: Device[] = await this.api("/api/v1/devices");
    this.devices = all.filter((d) => d.role === "daemon" || d.role === "hybrid");
    if (!this.selectedDaemon && this.devices.length > 0) {
      this.selectedDaemon = this.devices.find((d) => d.is_online) || this.devices[0];
    } else if (this.selectedDaemon) {
      this.selectedDaemon = this.devices.find((d) => d.id === this.selectedDaemon!.id) || null;
    }
  }

  private async connectRelayWebSocket() {
    if (!this.myClientDevice) return;
    const ticketRes = await this.api("/api/v1/relay/ticket", "POST", {
      device_id: this.myClientDevice.id,
      peer_device_id: null,
    });
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(`${proto}//${location.host}/v1/tunnel/ws`);
    this.ws?.close();
    this.ws = ws;

    ws.onopen = () => {
      ws.send(
        JSON.stringify({
          frame_type: "auth_bind",
          ticket: ticketRes.ticket,
          lane: "control_p0",
          generation: Date.now(),
        })
      );
    };

    ws.onmessage = async (ev) => {
      const frame = JSON.parse(ev.data);
      if (frame.frame_type === "auth_ack" && frame.ok) {
        if (this.selectedDaemon?.is_online) {
          this.queryDaemonOverview(this.selectedDaemon);
          void this.tryNegotiateDirectLanWs(this.selectedDaemon);
        }
      } else if (frame.frame_type === "peer_presence") {
        if (Array.isArray(frame.endpoints)) {
          const idx = this.devices.findIndex((d) => d.id === frame.device_id);
          if (idx >= 0) {
            this.devices[idx].endpoints_json = frame.endpoints;
            this.devices[idx].is_online = frame.is_online;
          }
        }
        await this.refreshDevices();
        if (this.selectedDaemon?.is_online) {
          void this.tryNegotiateDirectLanWs(this.selectedDaemon);
        } else if (this.directWsPeerId === frame.device_id && !frame.is_online) {
          this.directWs?.close();
        }
        this.render();
      } else if (frame.frame_type === "route_packet") {
        const peer = this.devices.find((d) => d.id === frame.src_device_id);
        if (!peer) return;
        const sess = this.getOrCreateSession(peer);
        const msg = sess.openMessage(frame.e2ee);
        this.handlePeerMessage(msg);
      }
    };
  }

  private queryDaemonOverview(peer: Device) {
    this.sendEncrypted(peer, {
      op: "ping",
      req_id: `p-${Date.now()}`,
      sent_ms: Date.now(),
    });
    this.sendEncrypted(peer, {
      op: "list_terminals",
      req_id: `ls-${Date.now()}`,
      include_finished: false,
    });
    this.sendEncrypted(peer, {
      op: "list_workspaces",
      req_id: `ws-${Date.now()}`,
      root_dirs: [],
    });
  }

  private collectDirectWsCandidates(peer: Device): string[] {
    const out: string[] = [];
    const pushUnique = (u: string) => {
      if (!u) return;
      if (location.protocol === "https:") {
        const isLocalWs = u.startsWith("ws://127.0.0.1") || u.startsWith("ws://localhost");
        if (!u.startsWith("wss://") && !isLocalWs) return;
      }
      if (!out.includes(u)) out.push(u);
    };

    for (const ep of peer.endpoints_json || []) {
      if (ep.kind === "lan_ws" || ep.kind === "ipv6_ws") {
        pushUnique(ep.addr);
      } else if (ep.kind === "lan_ipv4" || ep.kind === "cn_public_ipv6") {
        pushUnique(`ws://${ep.addr}`);
      }
    }

    if (location.hostname === "127.0.0.1" || location.hostname === "localhost") {
      pushUnique("ws://127.0.0.1:24444");
    }
    return out;
  }

  private async tryNegotiateDirectLanWs(peer: Device) {
    if (!peer.is_online || !this.myClientDevice) return;
    if (this.directWs && this.directWs.readyState === WebSocket.OPEN && this.directWsPeerId === peer.id) {
      return;
    }
    if (this.negotiatingPeerId === peer.id) return;
    this.negotiatingPeerId = peer.id;

    try {
      const candidates = this.collectDirectWsCandidates(peer);
      for (const wsUrl of candidates) {
        const ok = await this.probeSingleDirectWs(peer, wsUrl);
        if (ok) {
          this.sendEncrypted(peer, {
            op: "ping",
            req_id: `lan-p-${Date.now()}`,
            sent_ms: Date.now(),
          });
          this.render();
          break;
        }
      }
    } finally {
      this.negotiatingPeerId = null;
    }
  }

  private probeSingleDirectWs(peer: Device, wsUrl: string): Promise<boolean> {
    return new Promise((resolve) => {
      if (!this.myClientDevice) {
        resolve(false);
        return;
      }
      let settled = false;
      const done = (val: boolean) => {
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          resolve(val);
        }
      };

      let sock: WebSocket;
      try {
        sock = new WebSocket(wsUrl);
      } catch {
        done(false);
        return;
      }

      const timer = setTimeout(() => {
        if (!settled) {
          try {
            sock.close();
          } catch {}
          done(false);
        }
      }, 600);

      sock.onopen = () => {
        if (!this.myClientDevice) {
          done(false);
          return;
        }
        const tsMs = Date.now();
        const signedMsg = `p2p:${this.myClientDevice.id}:${peer.id}:${tsMs}`;
        const sigB64 = signWebMessage(this.webIdentity, signedMsg);
        sock.send(
          JSON.stringify({
            frame_type: "direct_p2p_hello",
            client_device_id: this.myClientDevice.id,
            target_device_id: peer.id,
            client_ed25519_pub_b64: this.webIdentity.ed25519PublicB64,
            client_x25519_pub_b64: this.webIdentity.x25519PublicB64,
            timestamp_ms: tsMs,
            signature_b64: sigB64,
          })
        );
      };

      sock.onmessage = (ev) => {
        let frame: any;
        try {
          frame = JSON.parse(ev.data);
        } catch {
          return;
        }

        if (!settled) {
          if (frame.frame_type === "direct_p2p_ack" && frame.ok) {
            const directSess = new BrowserE2EESession(
              this.webIdentity.x25519SecretB64,
              frame.daemon_x25519_pub_b64 || peer.x25519_pubkey
            );
            this.directWs?.close();
            this.directWs = sock;
            this.directWsPeerId = peer.id;
            this.directE2eeSessions.set(peer.id, directSess);
            this.activeTransportMode = "lan_ws";
            done(true);
          } else {
            try {
              sock.close();
            } catch {}
            done(false);
          }
          return;
        }

        if (frame.frame_type === "route_packet") {
          const directSess = this.directE2eeSessions.get(peer.id);
          if (!directSess) return;
          try {
            const msg = directSess.openMessage(frame.e2ee);
            this.handlePeerMessage(msg);
          } catch {
            // ignore malformed/out-of-order frame
          }
        }
      };

      sock.onerror = () => {
        if (this.directWs === sock) {
          this.directWs = null;
          this.directWsPeerId = null;
          this.directE2eeSessions.delete(peer.id);
          this.activeTransportMode = "relay_ws";
          this.render();
        }
        done(false);
      };

      sock.onclose = () => {
        if (this.directWs === sock) {
          this.directWs = null;
          this.directWsPeerId = null;
          this.directE2eeSessions.delete(peer.id);
          this.activeTransportMode = "relay_ws";
          this.render();
        }
        done(false);
      };
    });
  }

  private getOrCreateSession(peer: Device): BrowserE2EESession {
    let s = this.e2eeSessions.get(peer.id);
    if (!s) {
      s = new BrowserE2EESession(this.webIdentity.x25519SecretB64, peer.x25519_pubkey);
      this.e2eeSessions.set(peer.id, s);
    }
    return s;
  }

  private sendEncrypted(peer: Device, msg: unknown) {
    if (!this.myClientDevice) return;

    if (
      this.directWs &&
      this.directWs.readyState === WebSocket.OPEN &&
      this.directWsPeerId === peer.id
    ) {
      const directSess = this.directE2eeSessions.get(peer.id);
      if (directSess) {
        const e2ee = directSess.sealMessage(msg);
        this.directWs.send(
          JSON.stringify({
            frame_type: "route_packet",
            src_device_id: this.myClientDevice.id,
            dst_device_id: peer.id,
            lane: "control_p0",
            e2ee,
            sender_x25519_pub_b64: this.webIdentity.x25519PublicB64,
          })
        );
        return;
      }
    }

    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    const sess = this.getOrCreateSession(peer);
    const e2ee = sess.sealMessage(msg);
    this.ws.send(
      JSON.stringify({
        frame_type: "route_packet",
        src_device_id: this.myClientDevice.id,
        dst_device_id: peer.id,
        lane: "control_p0",
        e2ee,
        sender_x25519_pub_b64: this.webIdentity.x25519PublicB64,
      })
    );
  }

  private handlePeerMessage(msg: any) {
    if (msg.op === "pong") {
      this.rttMs = Math.max(1, Date.now() - msg.sent_ms);
    } else if (msg.op === "workspaces_snapshot") {
      this.repos = msg.repos || [];
      if (msg.voice_config) {
        this.voiceConfig = msg.voice_config;
      }
    } else if (msg.op === "voice_config_ack" && msg.config) {
      this.voiceConfig = msg.config;
    } else if (msg.op === "terminals_snapshot") {
      this.terminals = msg.terminals || [];
      if (!this.activeTerminalAddr && this.terminals.length > 0) {
        this.activeTerminalAddr = this.terminals[0].address;
        this.attachAndReadTerminal(this.activeTerminalAddr);
      }
    } else if (msg.op === "launch_agent_result" && msg.ok && msg.terminal) {
      this.terminals = [msg.terminal, ...this.terminals];
      this.activeTerminalAddr = msg.terminal.address;
      setTimeout(() => {
        if (this.activeTerminalAddr) this.attachAndReadTerminal(this.activeTerminalAddr);
      }, 250);
    } else if (msg.op === "terminal_screen_snapshot") {
      this.terminalScreen = msg.text || "";
    } else if (msg.op === "pty_output_chunk" && msg.terminal === this.activeTerminalAddr) {
      try {
        const bin = atob(msg.data_b64);
        const bytes = new Uint8Array(bin.length);
        for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
        const chunkText = new TextDecoder().decode(bytes);
        const stripped = chunkText.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").replace(/\x1b\][^\x07]*\x07/g, "");
        this.terminalScreen = (this.terminalScreen + stripped).slice(-12000);
      } catch {
        // ignore malformed chunk
      }
    } else if (msg.op === "upload_terminal_file_result" && msg.ok && msg.remote_path) {
      this.draftInput = this.draftInput ? `${this.draftInput} ${msg.remote_path}` : msg.remote_path;
    } else if (msg.op === "action_ack") {
      if (this.activeTerminalAddr) {
        setTimeout(() => this.requestScreen(this.activeTerminalAddr!), 220);
      }
    }
    this.render();
  }

  private attachAndReadTerminal(addr: string) {
    if (!this.selectedDaemon) return;
    this.requestScreen(addr);
    this.sendEncrypted(this.selectedDaemon, {
      op: "pty_attach",
      req_id: `pty-${Date.now()}`,
      terminal: addr,
      cols: 108,
      rows: 32,
    });
  }

  private requestScreen(addr: string) {
    if (!this.selectedDaemon) return;
    this.sendEncrypted(this.selectedDaemon, {
      op: "read_terminal_screen",
      req_id: `rd-${Date.now()}`,
      terminal: addr,
      history_lines: 80,
    });
  }

  private formatCursorDraft(str: string): string {
    if (!str) return `<span class="cursor-box">&nbsp;</span>`;
    return `<span class="cursor-box">${this.escapeHtml(str[0])}</span>${this.escapeHtml(str.slice(1))}`;
  }

  private renderLangSwitcher(): string {
    return `
      <div class="pm-lang-group" role="group" aria-label="${this.tr("Language", "语言")}">
        <button type="button" class="pm-lang-btn ${this.locale === "en" ? "active" : ""}" data-set-lang="en" title="English">EN</button>
        <button type="button" class="pm-lang-btn ${this.locale === "zh" ? "active" : ""}" data-set-lang="zh" title="简体中文">中文</button>
      </div>
    `;
  }

  private renderThemeSwitcher(): string {
    return `
      <div class="pm-theme-group" role="group" aria-label="${this.tr("Theme", "主题")}">
        <button type="button" class="pm-theme-btn ${this.themePref === "light" ? "active" : ""}" data-set-theme="light" title="${this.tr("Light mode", "浅色模式")}">${svgIcon("sun", 13)}</button>
        <button type="button" class="pm-theme-btn ${this.themePref === "dark" ? "active" : ""}" data-set-theme="dark" title="${this.tr("Dark mode", "深色模式")}">${svgIcon("moon", 13)}</button>
        <button type="button" class="pm-theme-btn ${this.themePref === "system" ? "active" : ""}" data-set-theme="system" title="${this.tr("System theme", "跟随系统")}">${svgIcon("monitor", 13)}</button>
      </div>
    `;
  }

  private bindCommonNavigation(root: HTMLElement) {
    root.querySelectorAll("[data-set-theme]").forEach((el) => {
      el.addEventListener("click", () => {
        const pref = (el as HTMLElement).dataset.setTheme as ThemePreference;
        this.applyTheme(pref, true);
      });
    });

    root.querySelectorAll("[data-set-lang]").forEach((el) => {
      el.addEventListener("click", () => {
        const loc = (el as HTMLElement).dataset.setLang as Locale;
        this.applyLocale(loc === "zh" ? "zh" : "en", true);
      });
    });

    root.querySelectorAll<HTMLElement>("[data-nav]").forEach((el) => {
      el.addEventListener("click", (e) => {
        e.preventDefault();
        const target = el.dataset.nav;
        if (target) this.navigate(target);
      });
    });
  }

  private renderHeroWorkbenchBody(baseUrl: string): string {
    if (this.heroNode === "linux") {
      return `
        <div class="pm-wb-grid">
          <div class="pm-wb-pane">
            <div>
              <div class="pm-wb-kicker">HOSTS · LINUX DAEMON</div>
              <div class="pm-wb-tree-row active">
                <span>atlas-core-01</span>
                <span style="color:var(--emerald);">online</span>
              </div>
              <div class="pm-wb-tree-row">
                <span>MacBook-Pro</span>
                <span>hybrid</span>
              </div>
              <div class="pm-wb-kicker" style="margin-top:12px;">SERVICES</div>
              <div class="pm-wb-tree-row">
                <span>coda-daemon</span>
                <span style="color:var(--emerald);">v0.3.7</span>
              </div>
              <div class="pm-wb-tree-row">
                <span>herdr-core</span>
                <span>hc 0.1.20</span>
              </div>
            </div>
            <div class="bento-well coda-mono" style="font-size:10.5px;color:var(--text-tertiary);">
              Relay: wss · 0.0.0.0 ports: 0
            </div>
          </div>

          <div class="pm-wb-pane">
            <div class="pm-diff-box">
              <div class="pm-diff-head">
                <span>deploy/docker-compose.yml &middot; 127.0.0.1 Tunnel</span>
                <span class="pm-badge green">Zero Port Exposure</span>
              </div>
              <div class="pm-diff-line ctx"><span class="pm-diff-ln">14</span><span>  coda-coordinator:</span></div>
              <div class="pm-diff-line del"><span class="pm-diff-ln">15</span><span>-   ports: ["8080:8080", "24443:24443"]</span></div>
              <div class="pm-diff-line add"><span class="pm-diff-ln">15</span><span>+   ports: ["127.0.0.1:8080:8080"]</span></div>
              <div class="pm-diff-line add"><span class="pm-diff-ln">16</span><span>+   environment: CODA_TRUST_CF_HEADERS=true</span></div>
              <div class="pm-diff-line ctx"><span class="pm-diff-ln">17</span><span>    depends_on: [postgres]</span></div>
            </div>
            <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;font-size:11.5px;color:var(--text-secondary);">
              <span class="coda-mono">Endpoint: ${this.escapeHtml(baseUrl)}</span>
              <span class="pm-badge blue">PostgreSQL 18 Healthy</span>
            </div>
          </div>

          <div class="pm-wb-pane">
            <div class="pm-pty-box">
              <div>
                <div style="color:#a1a1aa;margin-bottom:6px;">$ systemctl status coda-daemon</div>
                <div style="color:#22c55e;">Active: active (running) · X25519 sealed</div>
                <div>- Blind WSS uplink: wss://codahub.dev/v1/tunnel/ws</div>
                <div>- Bound worktrees: /opt/repo/coda (2 agents)</div>
                <div style="color:#3b82f6;">- Reconnect replay buffer: 120KB ready</div>
              </div>
              <div style="display:flex;justify-content:space-between;align-items:center;border-top:1px solid #222226;padding-top:8px;">
                <span style="color:#22c55e;">&gt; cargo test --release</span>
                <button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" data-nav="${this.user ? "/console" : "/login"}">${this.tr("Connect Node", "接入此节点")}</button>
              </div>
            </div>
          </div>
        </div>
      `;
    }

    if (this.heroNode === "iphone") {
      return `
        <div class="pm-wb-grid">
          <div class="pm-wb-pane">
            <div>
              <div class="pm-wb-kicker">MOBILE · SAFARI PWA</div>
              <div class="pm-wb-tree-row active">
                <span>iPhone 16 Pro</span>
                <span style="color:var(--emerald);">0.8 ms</span>
              </div>
              <div class="pm-wb-tree-row">
                <span>Attached: Mac</span>
                <span>main</span>
              </div>
              <div class="pm-wb-kicker" style="margin-top:12px;">INPUT STACK</div>
              <div class="pm-wb-tree-row">
                <span>Thumb Keypad</span>
                <span style="color:var(--emerald);">2-row</span>
              </div>
              <div class="pm-wb-tree-row">
                <span>Whisper ASR</span>
                <span>doc.set</span>
              </div>
            </div>
            <div class="bento-well coda-mono" style="font-size:10.5px;color:var(--text-tertiary);">
              Happy Eyeballs: Wi-Fi Direct
            </div>
          </div>

          <div class="pm-wb-pane">
            <div class="pm-diff-box">
              <div class="pm-diff-head">
                <span>SmartPath Route Selection &middot; WebCrypto</span>
                <span class="pm-badge blue">LAN :24444</span>
              </div>
              <div class="pm-diff-line ctx"><span class="pm-diff-ln">01</span><span>  probe ws://192.168.1.20:24444 ... 0.8 ms</span></div>
              <div class="pm-diff-line add"><span class="pm-diff-ln">02</span><span>+ verified Ed25519 peer signature</span></div>
              <div class="pm-diff-line add"><span class="pm-diff-ln">03</span><span>+ switched active lane -> direct_lan_ws</span></div>
              <div class="pm-diff-line ctx"><span class="pm-diff-ln">04</span><span>  fallback relay_wss standing by (443)</span></div>
              <div class="pm-diff-line ctx"><span class="pm-diff-ln">05</span><span>  payload: ChaCha20-Poly1305 (0 plaintext)</span></div>
            </div>
            <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;font-size:11.5px;color:var(--text-secondary);">
              <span class="coda-mono">Keys: esc / tab / ^C / ^D / arrows</span>
              <span class="pm-badge green">Zero App Store Gate</span>
            </div>
          </div>

          <div class="pm-wb-pane">
            <div class="pm-pty-box">
              <div>
                <div style="color:#a1a1aa;margin-bottom:6px;">Voice-to-Terminal · Whisper</div>
                <div style="color:#22c55e;">[ASR] &quot;${this.tr("Run full integration tests for coda-server", "运行 coda-server 的全量集成测试")}&quot;</div>
                <div>&gt; cargo test -p coda-server</div>
                <div style="color:#22c55e;">test result: ok. 7 passed; 0 failed</div>
              </div>
              <div style="display:flex;justify-content:space-between;align-items:center;border-top:1px solid #222226;padding-top:8px;">
                <span style="color:#a1a1aa;">&gt; hc upload screenshot.png</span>
                <button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" data-nav="${this.user ? "/console" : "/login"}">${this.tr("Open Console", "打开控制台")}</button>
              </div>
            </div>
          </div>
        </div>
      `;
    }

    return `
      <div class="pm-wb-grid">
        <div class="pm-wb-pane">
          <div>
            <div class="pm-wb-kicker">PROJECTS · 2 WORKTREES</div>
            <div class="pm-wb-tree-row active">
              <span>coda / main</span>
              <span class="pm-num" style="color:var(--emerald);">+412</span>
            </div>
            <div class="pm-wb-tree-row">
              <span>coda / feat-auth</span>
              <span class="pm-num" style="color:var(--signal-blue);">+821</span>
            </div>
            <div class="pm-wb-kicker" style="margin-top:12px;">ACTIVE AGENTS</div>
            <div class="pm-wb-tree-row">
              <span>Claude Code</span>
              <span style="color:var(--emerald);">live</span>
            </div>
            <div class="pm-wb-tree-row">
              <span>Codex CLI</span>
              <span>idle</span>
            </div>
          </div>
          <div class="bento-well coda-mono" style="font-size:10.5px;color:var(--text-tertiary);">
            GPUI 120Hz · Hybrid Node
          </div>
        </div>

        <div class="pm-wb-pane">
          <div class="pm-diff-box">
            <div class="pm-diff-head">
              <span>crates/coda-protocol/src/e2ee.rs</span>
              <span class="coda-mono pm-num"><span style="color:var(--emerald);">+1,233</span> <span style="color:var(--rose);">-116</span></span>
            </div>
            <div class="pm-diff-line ctx"><span class="pm-diff-ln">42</span><span>  pub fn seal_frame(&amp;mut self, msg: &amp;PeerMessage) -&gt; Envelope {</span></div>
            <div class="pm-diff-line del"><span class="pm-diff-ln">43</span><span>-   let wire = serde_json::to_vec(msg)?;</span></div>
            <div class="pm-diff-line add"><span class="pm-diff-ln">43</span><span>+   let compressed = zstd::bulk::compress(&amp;serde_json::to_vec(msg)?, 3)?;</span></div>
            <div class="pm-diff-line add"><span class="pm-diff-ln">44</span><span>+   let cipher = self.aead.encrypt(&amp;nonce, compressed.as_ref())?;</span></div>
            <div class="pm-diff-line ctx"><span class="pm-diff-ln">45</span><span>    Envelope { ephem_pub, nonce, ciphertext: cipher }</span></div>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;font-size:11.5px;color:var(--text-secondary);">
            <span class="coda-mono">X25519 + ChaCha20-Poly1305 · HKDF-SHA256</span>
            <span class="pm-badge green">Zero Server Plaintext</span>
          </div>
        </div>

        <div class="pm-wb-pane">
          <div class="pm-pty-box">
            <div>
              <div style="color:#a1a1aa;margin-bottom:6px;">Claude Code &middot; hc attach @mac/main</div>
              <div>&bull; Verified E2EE relay ticket &amp; direct LAN probe</div>
              <div style="color:#22c55e;">&bull; Web routes /login /register /activate -&gt; 200 OK</div>
              <div style="color:#3b82f6;">&bull; Waiting for next instruction on main...</div>
            </div>
            <div style="display:flex;justify-content:space-between;align-items:center;border-top:1px solid #222226;padding-top:8px;">
              <span style="color:#22c55e;">&gt; Ready &middot; 0.8 ms</span>
              <button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" data-nav="${this.user ? "/console" : "/login"}">${this.user ? this.tr("Open Console", "进入控制台") : this.tr("Sign In", "登录控制台")}</button>
            </div>
          </div>
        </div>
      </div>
    `;
  }

  private renderHostPreviewBody(baseUrl: string): string {
    if (this.previewTab === "server") {
      return `
        <div class="bento-well" style="display:flex;justify-content:space-between;align-items:center;">
          <span class="coda-mono" style="font-size:11.5px;font-weight:600;">atlas-core-01 · Ubuntu ARM64</span>
          <span class="pm-badge green"><span class="pm-status-dot"></span>Coordinator</span>
        </div>
        <div class="coda-mono bento-muted" style="font-size:11px;line-height:1.65;">
          <div>&bull; 127.0.0.1:8080 &rarr; wss://codahub.dev/v1/tunnel/ws</div>
          <div>&bull; PostgreSQL 18 healthy · Zero-SNI Relay :24443</div>
          <div>&bull; Endpoint: ${this.escapeHtml(baseUrl)}</div>
        </div>
      `;
    }
    if (this.previewTab === "ios") {
      return `
        <div class="bento-well" style="display:flex;justify-content:space-between;align-items:center;">
          <span class="coda-mono" style="font-size:11.5px;font-weight:600;">iPhone Safari / TestFlight</span>
          <span class="pm-badge blue"><span class="pm-status-dot"></span>LAN 0.8 ms</span>
        </div>
        <div class="coda-mono bento-muted" style="font-size:11px;line-height:1.65;">
          <div>&bull; Happy Eyeballs 250ms &rarr; ws://192.168.1.20:24444</div>
          <div>&bull; 2-Row Thumb Keypad + Whisper Voice Composer</div>
          <div>&bull; Attached to MacBook-Pro · coda/main</div>
        </div>
      `;
    }
    return `
      <div class="bento-well" style="display:flex;justify-content:space-between;align-items:center;">
        <span class="coda-mono" style="font-size:11.5px;font-weight:600;">MacBook-Pro · GPUI 120Hz</span>
        <span class="pm-badge green"><span class="pm-status-dot"></span>3-Pane Live</span>
      </div>
      <div class="coda-mono bento-muted" style="font-size:11px;line-height:1.65;">
        <div>&bull; PROJECTS: Services (herdr-core) · Agents · Actions</div>
        <div>&bull; Worktree Diff: <span style="color:var(--emerald);">+1,233</span> <span style="color:var(--rose);">-116</span></div>
        <div>&bull; Hybrid Mode: Control remote Linux + accept mobile</div>
      </div>
    `;
  }

  private renderAuthShowcasePanel(baseUrl: string, ver: string, isActivateMode = false): string {
    return `
      <aside class="pm-auth-showcase">
        <div class="pm-grid-backdrop"></div>
        <div class="pm-frame pm-crypto-card">
          <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
          <span class="pm-corner bl"></span><span class="pm-corner br"></span>
          <div style="display:flex;align-items:center;justify-content:space-between;gap:10px;margin-bottom:20px;">
            <span class="pm-badge green"><span class="pm-status-dot"></span>Zero-Knowledge Handshake</span>
            <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">${this.escapeHtml(ver)}</span>
          </div>
          <h2 style="font-size:20px;font-weight:600;letter-spacing:-0.025em;margin-bottom:8px;">
            ${
              isActivateMode
                ? this.tr("RFC 8628 Device Authorization Flow", "RFC 8628 设备码安全授权流")
                : this.tr("Private keys never leave your device.", "私钥永不离开你的本地设备。")
            }
          </h2>
          <p style="font-size:13px;line-height:1.65;color:var(--text-secondary);margin-bottom:22px;">
            ${
              isActivateMode
                ? this.tr(
                    "After running login on a headless Linux host or macOS terminal, the browser only verifies ownership—end-to-end session keys are negotiated directly between endpoints.",
                    "在无头 Linux 服务器或 macOS 终端运行登录指令后，浏览器只负责核对账号归属；端到端会话密钥由终端与本机直接协商。"
                  )
                : this.tr(
                    "The coordinator and Cloudflare Tunnel act purely as a blind ciphertext relay. Even full database disclosure cannot decrypt any terminal output or source diff.",
                    "主控制面与 Cloudflare Tunnel 仅承担密文信封盲转发，即使控制面库表完全暴露也无法解密任何终端输出或源代码 Diff。"
                  )
            }
          </p>

          <div style="display:flex;flex-direction:column;gap:10px;margin-bottom:22px;">
            <div class="pm-step-row">
              <span class="pm-badge coda-mono">01</span>
              <div>
                <div style="font-size:13px;font-weight:600;">${this.tr("Ed25519 Device Signatures", "Ed25519 设备身份签名")}</div>
                <div style="font-size:12px;color:var(--text-secondary);margin-top:2px;">${this.tr("Browser and host daemon each hold local signing keys, performing mutual signature verification before direct LAN cutover.", "浏览器与守护进程分别持有独立签名公钥，局域网直连前先校验双向签名。")}</div>
              </div>
            </div>
            <div class="pm-step-row">
              <span class="pm-badge coda-mono">02</span>
              <div>
                <div style="font-size:13px;font-weight:600;">${this.tr("X25519 + HKDF-SHA256 Forward Secrecy", "X25519 + HKDF-SHA256 前向保密")}</div>
                <div style="font-size:12px;color:var(--text-secondary);margin-top:2px;">${this.tr("Each terminal or control channel derives fresh symmetric session keys with strict per-tenant relay ticket isolation.", "每次建立终端或控制通道独立派生对称密钥，严格按租户隔离中继票据。")}</div>
              </div>
            </div>
            <div class="pm-step-row">
              <span class="pm-badge coda-mono">03</span>
              <div>
                <div style="font-size:13px;font-weight:600;">zstd + ChaCha20-Poly1305 AEAD</div>
                <div style="font-size:12px;color:var(--text-secondary);margin-top:2px;">${this.tr("High-throughput PTY output is compressed with zstd and sealed per frame to stay responsive on constrained networks.", "高并发 PTY 输出先流式压缩再逐帧加密，弱网下依然保持低延迟响应。")}</div>
              </div>
            </div>
          </div>

          <div class="bento-well coda-mono" style="font-size:11px;color:var(--text-secondary);display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;">
            <span>${this.escapeHtml(baseUrl)}</span>
            <span style="color:var(--emerald);">TLS 1.3 / WSS &middot; Verified</span>
          </div>
        </div>
      </aside>
    `;
  }

  render() {
    const root = document.getElementById("app")!;
    const baseUrl = this.siteInfo?.public_base_url || window.location.origin;
    const installCmd = this.siteInfo?.install_command || `curl -fsSL ${baseUrl}/install.sh | sh`;
    const ver = this.siteInfo?.version || "v0.3.7";
    const dl: SiteInfo["downloads"] = this.siteInfo?.downloads || {
      daemon_linux_x64: `${baseUrl}/download/daemon-linux-x86_64.tar.gz`,
      daemon_linux_arm64: `${baseUrl}/download/daemon-linux-aarch64.tar.gz`,
      daemon_macos_arm64: `${baseUrl}/download/daemon-macos-aarch64.tar.gz`,
      server_linux_x64: `${baseUrl}/download/server-linux-x86_64.tar.gz`,
      server_linux_arm64: `${baseUrl}/download/server-linux-aarch64.tar.gz`,
      server_macos_arm64: `${baseUrl}/download/server-macos-aarch64.tar.gz`,
      desktop_macos_dmg: `${baseUrl}/download/desktop-macos.dmg`,
      desktop_macos_tar: `${baseUrl}/download/desktop-macos.tar.gz`,
      deploy_bundle: `${baseUrl}/download/deploy-bundle.tar.gz`,
      ios_beta_url: null,
      releases_page: "https://github.com/Annihilater/coda/releases/latest",
    };

    if (this.route === "console" && !this.user) {
      window.history.replaceState({}, "", "/login");
      this.route = "login";
      this.authMode = "login";
    } else if ((this.route === "login" || this.route === "register") && this.user) {
      const target = this.pendingActivationCode
        ? `/activate?code=${encodeURIComponent(this.pendingActivationCode)}`
        : "/console";
      window.history.replaceState({}, "", target);
      this.route = this.resolveRouteFromLocation();
    }

    if (this.route === "login" || this.route === "register") {
      this.stopBentoLoop();
      this.renderAuthView(root, baseUrl, ver);
      return;
    }

    if (this.route === "activate") {
      this.stopBentoLoop();
      this.renderActivateView(root, baseUrl, ver);
      return;
    }

    if (this.route === "console" && this.user) {
      this.stopBentoLoop();
      this.renderConsoleView(root, installCmd);
      return;
    }

    this.renderHomeView(root, baseUrl, installCmd, ver, dl);
  }

  private renderHomeView(
    root: HTMLElement,
    baseUrl: string,
    installCmd: string,
    ver: string,
    dl: SiteInfo["downloads"] | undefined
  ) {
    const otpGroupA = ["C", "O", "D", "A"];
    const otpGroupB = ["7", "F", "2", "9"];

    root.innerHTML = `
      <div class="pm-page">
        <header class="pm-topbar">
          <div class="pm-topbar-inner">
            <div class="pm-topbar-left">
              <a class="pm-brand" href="/" data-nav="/">
                ${codaLogoSVG(22)}
                <span>Coda</span>
              </a>
              <span class="pm-badge coda-mono">${this.escapeHtml(ver)}</span>
              <span class="pm-vsep"></span>
              <nav class="pm-nav-pills" aria-label="Primary">
                <a class="pm-nav-link" href="#workbench">${this.tr("Workbench", "工作台预览")}</a>
                <a class="pm-nav-link" href="#bento">${this.tr("Live Grid", "实时网格")}</a>
                <a class="pm-nav-link" href="#architecture">${this.tr("Architecture", "零知识架构")}</a>
                <a class="pm-nav-link" href="#downloads">${this.tr("Downloads", "全端下载")}</a>
                <a class="pm-nav-link" href="/activate" data-nav="/activate">${this.tr("Activate", "设备绑机")}</a>
              </nav>
            </div>
            <div class="pm-topbar-right">
              <button id="btn-cmd-k" class="pm-cmd-trigger" type="button" title="${this.tr("Quick Command Palette", "快捷指令面板")}">
                ${svgIcon("search", 13)}
                <span class="pm-cmd-label">${this.tr("Search", "搜索")}</span>
                <span class="pm-kbd">⌘K</span>
              </button>
              ${this.renderLangSwitcher()}
              ${this.renderThemeSwitcher()}
              ${
                this.user
                  ? `
                <span class="pm-badge green coda-mono"><span class="pm-status-dot"></span>@${this.escapeHtml(this.user.handle)}</span>
                <a href="/console" data-nav="/console" class="pm-btn pm-btn-primary pm-btn-sm">${this.tr("Open Console", "进入控制台")} ${svgIcon("arrowRight", 13)}</a>
              `
                  : `
                <a id="btn-nav-login" href="/login" data-nav="/login" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("Sign In", "登录")}</a>
                <a id="btn-nav-register" href="/register" data-nav="/register" class="pm-btn pm-btn-primary pm-btn-sm">${this.tr("Get Started", "免费注册")}</a>
              `
              }
            </div>
          </div>
        </header>

        <!-- BAND 1: Centered Coordinate-Grid Hero + Interactive 3-Pane Coda Workbench -->
        <section id="top" class="pm-band pm-hero-band">
          <div class="pm-grid-backdrop"></div>
          <div class="pm-hero-glow"></div>
          <div class="pm-rail">
            <div class="pm-hero-center">
              <a href="#architecture" class="pm-announce-pill">
                <span class="pm-badge green"><span class="pm-status-dot"></span>${this.escapeHtml(ver)} Ready</span>
                <span>${this.tr("SmartPath Sub-ms LAN Direct & RFC 8628 Device Auth", "SmartPath 局域网毫秒直连 & RFC 8628 设备授权")}</span>
                ${svgIcon("arrowRight", 13)}
              </a>
              <h1 class="pm-h1">${this.tr("The zero-knowledge control plane<br />for cross-device AI coding.", "跨设备 AI 编程的<br />零知识控制面。")}</h1>
              <p class="pm-hero-sub">
                ${this.tr(
                  "Weave your Linux cloud hosts, native 3-pane macOS desktop app, and mobile browser into one end-to-end encrypted mesh. Persistent PTYs run on local <code>herdr-core</code> with sub-millisecond Wi-Fi P2P and zero-trust Cloudflare Tunnel relay.",
                  "把常驻的 Linux 云主机、Mac 原生三栏桌面端与随身浏览器编织进同一张端到端加密网。终端进程托管于本机 <code>herdr-core</code>，同局域网亚毫秒直连，跨网经 Cloudflare Tunnel 自动盲转发。"
                )}
              </p>
              <div class="pm-hero-actions">
                ${
                  dl
                    ? `<a class="pm-btn pm-btn-primary pm-btn-lg" href="${dl.desktop_macos_dmg}" target="_blank">${svgIcon("apple", 15)} ${this.tr("Download for macOS (.dmg)", "下载 macOS 桌面端 (.dmg)")}</a>`
                    : `<a class="pm-btn pm-btn-primary pm-btn-lg" href="#downloads">${svgIcon("laptop", 15)} ${this.tr("Download Clients", "获取全端客户端")}</a>`
                }
                <a class="pm-btn pm-btn-secondary pm-btn-lg" href="${this.user ? "/console" : "/login"}" data-nav="${this.user ? "/console" : "/login"}">
                  ${this.user ? this.tr("Open Web Console", "进入在线控制台") : this.tr("Sign in to Console", "登录控制台")} ${svgIcon("arrowRight", 14)}
                </a>
              </div>
              <div class="pm-install-shell">
                <span class="pm-install-tag">SHELL</span>
                <pre>$ ${this.escapeHtml(installCmd)}</pre>
                <button id="btn-copy-install" type="button" class="pm-btn pm-btn-secondary pm-btn-sm">
                  ${
                    this.copiedToast === "ok"
                      ? `${svgIcon("check", 13)} ${this.tr("Copied", "已复制")}`
                      : this.copiedToast === "manual"
                      ? `${svgIcon("copy", 13)} ${this.tr("Copy manually", "请手动复制")}`
                      : `${svgIcon("copy", 13)} ${this.tr("Copy", "复制命令")}`
                  }
                </button>
              </div>
              <div style="margin-top:10px;font-size:12px;color:var(--text-tertiary);">
                ${this.tr(
                  "Supports macOS 14+ (Apple Silicon) &amp; Linux x86_64 / ARM64 &middot; Prints your <code>/activate</code> device link automatically",
                  "支持 macOS 14+ (Apple Silicon) 与 Linux x86_64 / ARM64 &middot; 安装后自动打印 <code>/activate</code> 设备授权链接"
                )}
              </div>
            </div>

            <!-- Interactive 3-Pane Workbench Showcase -->
            <div id="workbench" class="pm-frame pm-workbench">
              <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
              <span class="pm-corner bl"></span><span class="pm-corner br"></span>
              <div class="pm-wb-bar">
                <div style="display:flex;align-items:center;gap:12px;">
                  <div class="pm-wb-lights" aria-hidden="true">
                    <span class="pm-wb-dot" style="background:#ff5f57;"></span>
                    <span class="pm-wb-dot" style="background:#febc2e;"></span>
                    <span class="pm-wb-dot" style="background:#28c840;"></span>
                  </div>
                  <span class="coda-mono" style="font-size:11.5px;color:var(--text-secondary);">Coda Workbench</span>
                </div>
                <div class="pm-wb-tabs" role="tablist" aria-label="Workbench Node Switcher">
                  <button type="button" class="pm-wb-tab ${this.heroNode === "mac" ? "active" : ""}" data-hero-node="mac">${svgIcon("laptop", 12)} MacBook-Pro</button>
                  <button type="button" class="pm-wb-tab ${this.heroNode === "linux" ? "active" : ""}" data-hero-node="linux">${svgIcon("server", 12)} atlas-core-01</button>
                  <button type="button" class="pm-wb-tab ${this.heroNode === "iphone" ? "active" : ""}" data-hero-node="iphone">${svgIcon("smartphone", 12)} iPhone 16 Pro</button>
                </div>
                <span class="pm-badge green"><span class="pm-status-dot"></span>E2EE Sealed</span>
              </div>
              <div id="hero-workbench-body">
                ${this.renderHeroWorkbenchBody(baseUrl)}
              </div>
            </div>
          </div>
        </section>

        <!-- BAND 2: 4-Column Engineering Telemetry Strip -->
        <section class="pm-band">
          <div class="pm-rail compact">
            <div class="pm-frame">
              <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
              <span class="pm-corner bl"></span><span class="pm-corner br"></span>
              <div class="pm-hairline-grid cols-4">
                <div class="pm-metric-cell">
                  <span class="pm-eyebrow" style="margin-bottom:2px;">LAN DIRECT RTT</span>
                  <div class="pm-metric-val pm-num" style="color:var(--emerald);">&lt; 1 ms</div>
                  <div style="font-size:12.5px;color:var(--text-secondary);">${this.tr("Auto P2P over <code>:24444</code> on shared Wi-Fi", "同 Wi-Fi 自动走 <code>:24444</code> P2P 直连")}</div>
                </div>
                <div class="pm-metric-cell">
                  <span class="pm-eyebrow" style="margin-bottom:2px;">RELAY PLAINTEXT</span>
                  <div class="pm-metric-val pm-num">0 Bytes</div>
                  <div style="font-size:12.5px;color:var(--text-secondary);">${this.tr("Zero terminal plaintext exposed to coordinator or edge", "主控与边缘节点无法读取任何终端明文")}</div>
                </div>
                <div class="pm-metric-cell">
                  <span class="pm-eyebrow" style="margin-bottom:2px;">MACOS WORKBENCH</span>
                  <div class="pm-metric-val pm-num">120 Hz</div>
                  <div style="font-size:12.5px;color:var(--text-secondary);">${this.tr("Native Rust + GPUI hybrid workstation &amp; host", "Rust + GPUI 原生渲染，双向控制节点")}</div>
                </div>
                <div class="pm-metric-cell">
                  <span class="pm-eyebrow" style="margin-bottom:2px;">HOST BIND SURFACE</span>
                  <div class="pm-metric-val pm-num" style="color:var(--signal-blue);">127.0.0.1</div>
                  <div style="font-size:12.5px;color:var(--text-secondary);">${this.tr("Zero inbound public ports; outbound Cloudflare Tunnel", "零公网入站端口暴露，Cloudflare 穿透接入")}</div>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- BAND 3: Upgraded Autonomous Bento Motion Grid -->
        <section id="bento" class="pm-band">
          <div class="pm-rail">
            <div class="pm-section-head">
              <div>
                <div class="pm-eyebrow">Live Architecture Grid</div>
                <h2 class="pm-h2">${this.tr("Six core primitives, orchestrated in real time.", "六组核心能力，在同一画布协同运转。")}</h2>
              </div>
              <span class="pm-badge blue"><span class="pm-status-dot"></span>60 / 120Hz Motion Spec</span>
            </div>

            <div class="bento-tray" id="bento-stage">
              <div class="bento-grid">
                <!-- Card 1: Live Agent PTY Stream (Dark Contrast) -->
                <article class="bento-card is-dark bento-span-5">
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
                    <div style="display:flex;align-items:center;gap:7px;">
                      ${svgIcon("terminal", 14)}
                      <span class="bento-kicker">Herdr PTY · coda/main</span>
                    </div>
                    <span class="pm-badge green"><span class="pm-status-dot"></span>Running</span>
                  </div>
                  <div style="display:flex;flex-direction:column;gap:4px;">
                    <div class="bento-pty-line" data-pty-idx="0">
                      <span>Claude Code &middot; refactor auth</span>
                      <span class="pm-num" style="color:var(--emerald);">+412 -38</span>
                    </div>
                    <div class="bento-pty-line" data-pty-idx="1">
                      <span>Codex &middot; tunnel metrics</span>
                      <span class="pm-num" style="color:var(--signal-blue);">+821 -78</span>
                    </div>
                    <div class="bento-pty-line" data-pty-idx="2">
                      <span>cargo test --workspace</span>
                      <span style="color:var(--emerald);">exit 0</span>
                    </div>
                  </div>
                  <div class="bento-well coda-mono" style="display:flex;justify-content:space-between;align-items:center;font-size:11.5px;">
                    <span><span style="color:var(--emerald);">&gt;</span> hc attach coda/main</span>
                    <span class="bento-faint">zstd stream</span>
                  </div>
                </article>

                <!-- Card 2: SmartPath Route Glider + Sparkline -->
                <article class="bento-card bento-span-3">
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
                    <span class="bento-kicker">${this.tr("SmartPath Routing", "SmartPath 路由")}</span>
                    <div class="bento-stack" style="width:112px;justify-content:flex-end;">
                      <span class="bento-layer" style="justify-content:flex-end;" data-bento="badge-lan">
                        <span class="pm-badge green"><span class="pm-status-dot"></span>LAN Direct</span>
                      </span>
                      <span class="bento-layer" style="justify-content:flex-end;opacity:0;" data-bento="badge-wss">
                        <span class="pm-badge blue"><span class="pm-status-dot"></span>CF WSS 443</span>
                      </span>
                    </div>
                  </div>
                  <div>
                    <div class="bento-faint" style="font-size:11px;margin-bottom:2px;">${this.tr("Live Round-Trip Latency (RTT)", "实时往返延迟 (RTT)")}</div>
                    <div class="coda-mono pm-num" data-bento="rtt-val" style="font-size:26px;font-weight:600;letter-spacing:-0.03em;">0.8 ms</div>
                    <div class="bento-spark-wrap">
                      <svg viewBox="0 0 220 38" width="100%" height="36" fill="none" preserveAspectRatio="none">
                        <path d="M 2 30 Q 45 28 88 29 T 165 28 T 218 27" stroke="var(--emerald)" stroke-width="2" stroke-linecap="round" data-bento="spark-lan" />
                        <path d="M 2 18 Q 52 10 105 16 T 175 11 T 218 14" stroke="var(--signal-blue)" stroke-width="2" stroke-linecap="round" stroke-dasharray="4 3" data-bento="spark-wss" style="opacity:0.28;" />
                      </svg>
                    </div>
                  </div>
                  <div class="bento-track">
                    <div class="bento-glider" data-bento="route-glider"></div>
                    <span class="bento-track-label">LAN :24444</span>
                    <span class="bento-track-label">WSS :443</span>
                  </div>
                </article>

                <!-- Card 3: Zero-Knowledge E2EE Seal + Pipeline -->
                <article class="bento-card bento-span-4">
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
                    <div style="display:flex;align-items:center;gap:7px;">
                      ${svgIcon("shield", 14)}
                      <span class="bento-kicker">${this.tr("Zero-Knowledge Envelope", "端到端零知识信封")}</span>
                    </div>
                    <span class="pm-badge coda-mono" data-bento="seal-pct">0%</span>
                  </div>
                  <div style="display:flex;align-items:center;gap:14px;">
                    <svg width="46" height="46" viewBox="0 0 46 46" style="flex-shrink:0;transform:rotate(-90deg);">
                      <circle cx="23" cy="23" r="18" fill="none" stroke="var(--border-primary)" stroke-width="3.5" />
                      <circle
                        cx="23" cy="23" r="18"
                        fill="none"
                        stroke="var(--signal-blue)"
                        stroke-width="3.5"
                        stroke-linecap="round"
                        stroke-dasharray="113.1"
                        stroke-dashoffset="113.1"
                        data-bento="seal-ring"
                      />
                    </svg>
                    <div style="flex:1;min-width:0;">
                      <div class="bento-stack">
                        <div class="bento-layer coda-mono" data-bento="seal-plain" style="font-size:12px;font-weight:600;">PTY Frame (zstd)</div>
                        <div class="bento-layer coda-mono" data-bento="seal-cipher" style="font-size:12px;font-weight:600;color:var(--emerald);opacity:0;">ChaCha20 · 0x9f4a...e81c</div>
                      </div>
                      <div class="bento-faint" style="font-size:11.5px;margin-top:4px;">${this.tr("X25519 + HKDF-SHA256 &middot; 0 server plaintext", "X25519 + HKDF-SHA256 &middot; 服务端 0 明文")}</div>
                    </div>
                  </div>
                  <div class="bento-pipe">
                    <div class="bento-pipe-step" data-seal-step="0">01. PTY</div>
                    <div class="bento-pipe-step" data-seal-step="1">02. zstd</div>
                    <div class="bento-pipe-step" data-seal-step="2">03. AEAD</div>
                  </div>
                </article>

                <!-- Card 4: RFC 8628 Device Code -->
                <article class="bento-card bento-span-4">
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
                    <div style="display:flex;align-items:center;gap:7px;">
                      ${svgIcon("key", 14)}
                      <span class="bento-kicker">${this.tr("RFC 8628 Browser Auth", "RFC 8628 浏览器绑机")}</span>
                    </div>
                    <div class="bento-stack" style="width:94px;">
                      <span class="bento-layer" style="justify-content:flex-end;" data-bento="otp-pending">
                        <span class="pm-badge amber">Pending</span>
                      </span>
                      <span class="bento-layer" style="justify-content:flex-end;opacity:0;" data-bento="otp-approved">
                        <span class="pm-badge green">Approved</span>
                      </span>
                    </div>
                  </div>
                  <div class="bento-otp-group">
                    ${otpGroupA.map((ch, idx) => `<div class="bento-otp-cell" data-otp-idx="${idx}">${ch}</div>`).join("")}
                    <span class="bento-otp-dash">-</span>
                    ${otpGroupB.map((ch, idx) => `<div class="bento-otp-cell" data-otp-idx="${idx + 4}">${ch}</div>`).join("")}
                  </div>
                  <div class="bento-faint coda-mono" style="font-size:11px;display:flex;justify-content:space-between;">
                    <span>coda-daemon login</span>
                    <a href="/activate?code=CODA-7F29" data-nav="/activate?code=CODA-7F29" style="color:var(--signal-blue);">/activate &rarr;</a>
                  </div>
                </article>

                <!-- Card 5: Mesh Topology SVG + Interactive Host Preview -->
                <article class="bento-card bento-span-5">
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;flex-wrap:wrap;">
                    <span class="bento-kicker">${this.tr("Encrypted Mesh Topology", "多端加密 Mesh 拓扑")}</span>
                    <div class="bento-host-tabs">
                      <button type="button" class="bento-host-tab ${this.previewTab === "coda" ? "active" : ""}" data-preview-tab="coda">Mac 2/2</button>
                      <button type="button" class="bento-host-tab ${this.previewTab === "server" ? "active" : ""}" data-preview-tab="server">Linux 1/1</button>
                      <button type="button" class="bento-host-tab ${this.previewTab === "ios" ? "active" : ""}" data-preview-tab="ios">iPhone 1/1</button>
                    </div>
                  </div>
                  <svg class="bento-mesh-svg" viewBox="0 0 340 58" fill="none" aria-hidden="true">
                    <path d="M 52 29 Q 105 8 158 29" stroke="var(--border-strong)" stroke-width="1.5" stroke-dasharray="4 3" />
                    <path d="M 182 29 Q 238 50 294 29" stroke="var(--border-strong)" stroke-width="1.5" stroke-dasharray="4 3" />
                    <circle cx="54" cy="29" r="3.5" fill="var(--emerald)" data-bento="pkt-a" />
                    <circle cx="190" cy="29" r="3.5" fill="var(--signal-blue)" data-bento="pkt-b" />
                    <rect x="10" y="13" width="64" height="32" rx="7" fill="var(--bg-elevated)" stroke="var(--border-primary)" />
                    <text x="42" y="33" text-anchor="middle" fill="var(--text-primary)" font-family="var(--mono)" font-size="10.5">Mac</text>
                    <rect x="136" y="13" width="68" height="32" rx="7" fill="var(--bg-elevated)" stroke="var(--signal-blue-bd)" />
                    <text x="170" y="33" text-anchor="middle" fill="var(--signal-blue)" font-family="var(--mono)" font-size="10.5">Mesh</text>
                    <rect x="266" y="13" width="64" height="32" rx="7" fill="var(--bg-elevated)" stroke="var(--border-primary)" />
                    <text x="298" y="33" text-anchor="middle" fill="var(--text-primary)" font-family="var(--mono)" font-size="10.5">Linux</text>
                  </svg>
                  <div id="bento-host-preview" style="display:flex;flex-direction:column;gap:8px;">
                    ${this.renderHostPreviewBody(baseUrl)}
                  </div>
                </article>

                <!-- Card 6: Voice-to-Terminal (Dark Contrast) -->
                <article class="bento-card is-dark bento-span-3">
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
                    <div style="display:flex;align-items:center;gap:7px;">
                      ${svgIcon("mic", 14)}
                      <span class="bento-kicker">${this.tr("Voice-to-Terminal", "语音转终端指令")}</span>
                    </div>
                    <span class="pm-badge coda-mono">Whisper</span>
                  </div>
                  <div class="bento-wave">
                    ${[0, 1, 2, 3, 4, 5, 6, 7, 8]
                      .map((i) => `<span class="bento-bar" data-voice-idx="${i}"></span>`)
                      .join("")}
                  </div>
                  <div class="bento-well coda-mono" style="font-size:10.5px;display:flex;justify-content:space-between;align-items:center;gap:6px;white-space:nowrap;">
                    <span><span style="color:var(--emerald);">&gt;</span> coda-daemon</span>
                    <span style="color:var(--emerald);">hc doc.set</span>
                  </div>
                </article>
              </div>
            </div>
          </div>
        </section>

        <!-- BAND 4: E2EE Packet Blueprint + Pillars + High-Contrast Palermo Table -->
        <section id="architecture" class="pm-band">
          <div class="pm-rail">
            <div class="pm-section-head">
              <div>
                <div class="pm-eyebrow">Zero-Knowledge Architecture</div>
                <h2 class="pm-h2">${this.tr("Transport infrastructure built for high-concurrency agents.", "为高并发 Agent 工作流重塑的通信底座。")}</h2>
                <p class="pm-sub">${this.tr("No brittle SSH port-forwarding scripts, zero exposed public ports, and native desktop responsiveness.", "没有繁琐的 SSH 隧道脚本，也无需向公网暴露任何控制端口，同时保留原生客户端的流畅度。")}</p>
              </div>
            </div>

            <!-- Horizontal 3-Stage Cryptographic Packet Flow -->
            <div class="pm-frame" style="margin-bottom:20px;">
              <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
              <span class="pm-corner bl"></span><span class="pm-corner br"></span>
              <div class="pm-blueprint-flow">
                <div class="pm-blueprint-node">
                  <div style="display:flex;justify-content:space-between;align-items:center;">
                    <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">STAGE 01 &middot; CLIENT</span>
                    <span class="pm-badge blue">Web / Mac / iOS</span>
                  </div>
                  <div>
                    <div style="font-size:14.5px;font-weight:600;margin-bottom:4px;">${this.tr("Local Seal &amp; Identity Signature", "本地密封与身份签名")}</div>
                    <div style="font-size:12.5px;color:var(--text-secondary);line-height:1.6;">${this.tr("Browser and macOS clients generate local <code>Ed25519 + X25519</code> keys, compressing and sealing every frame before egress.", "浏览器 / Mac 客户端生成 <code>Ed25519 + X25519</code> 会话密钥，将指令与语音先压缩再密封。")}</div>
                  </div>
                </div>
                <div class="pm-blueprint-arrow">${svgIcon("arrowRight", 15)}</div>
                <div class="pm-blueprint-node">
                  <div style="display:flex;justify-content:space-between;align-items:center;">
                    <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">STAGE 02 &middot; TRANSPORT</span>
                    <span class="pm-badge green">Blind Envelope</span>
                  </div>
                  <div>
                    <div style="font-size:14.5px;font-weight:600;margin-bottom:4px;">${this.tr("SmartPath Adaptive Transport", "SmartPath 自适应通道")}</div>
                    <div style="font-size:12.5px;color:var(--text-secondary);line-height:1.6;">${this.tr("Cuts over to <code>ws://lan:24444</code> on local Wi-Fi, or relays blind envelopes via <code>127.0.0.1</code> Cloudflare Tunnel WSS / raw TCP.", "同网秒切 <code>ws://lan:24444</code>，跨网通过 <code>127.0.0.1</code> Cloudflare Tunnel WSS 或国内裸 TCP 转发。")}</div>
                  </div>
                </div>
                <div class="pm-blueprint-arrow">${svgIcon("arrowRight", 15)}</div>
                <div class="pm-blueprint-node">
                  <div style="display:flex;justify-content:space-between;align-items:center;">
                    <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">STAGE 03 &middot; HOST DAEMON</span>
                    <span class="pm-badge">herdr-core</span>
                  </div>
                  <div>
                    <div style="font-size:14.5px;font-weight:600;margin-bottom:4px;">${this.tr("Verify &amp; Drive Persistent PTY", "解封并驱动本地 PTY")}</div>
                    <div style="font-size:12.5px;color:var(--text-secondary);line-height:1.6;">${this.tr("<code>coda-daemon</code> verifies peer signatures, opens the AEAD envelope, and streams directly into persistent Agent PTYs &amp; Git worktrees.", "<code>coda-daemon</code> 校验对端签名并解封 AEAD，将输入写入常驻 Agent PTY 与 Git Worktree。")}</div>
                  </div>
                </div>
              </div>
            </div>

            <div class="pm-frame" style="margin-bottom:20px;">
              <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
              <span class="pm-corner bl"></span><span class="pm-corner br"></span>
              <div class="pm-hairline-grid cols-3">
                <div class="pm-grid-cell">
                  <div>
                    <div class="coda-mono" style="font-size:11px;color:var(--text-tertiary);margin-bottom:8px;">01 / DAEMON</div>
                    <div style="font-size:15px;font-weight:600;margin-bottom:6px;">${this.tr("Persistent Herdr Daemon", "常驻 Herdr 进程守护")}</div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.6;">${this.tr("Close your laptop, drop offline, or switch to your phone—remote agents and builds keep running and replay full PTY buffers &amp; Git diffs on reconnect.", "合盖、断网或切到手机时，远端 Agent 与编译任务零中断运行，重连即回放完整 PTY 输出与 Git Worktree Diff。")}</p>
                  </div>
                  <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">herdr-core · full-duplex PTY</span>
                </div>
                <div class="pm-grid-cell">
                  <div>
                    <div class="coda-mono" style="font-size:11px;color:var(--text-tertiary);margin-bottom:8px;">02 / CRYPTO</div>
                    <div style="font-size:15px;font-weight:600;margin-bottom:6px;">${this.tr("End-to-End Zero-Knowledge", "端到端零知识加密")}</div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.6;">${this.tr("Every node holds its own local <code>Ed25519 + X25519</code> keys. Streams are compressed with <code>zstd</code> and sealed via <code>ChaCha20-Poly1305</code>; relays see only ciphertext.", "每台节点在本地保管 <code>Ed25519 + X25519</code> 私钥，流量先经 <code>zstd</code> 压缩再由 <code>ChaCha20-Poly1305</code> 密封，服务器不可解密。")}</p>
                  </div>
                  <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">HKDF-SHA256 · Blind Relay</span>
                </div>
                <div class="pm-grid-cell">
                  <div>
                    <div class="coda-mono" style="font-size:11px;color:var(--text-tertiary);margin-bottom:8px;">03 / ROUTING</div>
                    <div style="font-size:15px;font-weight:600;margin-bottom:6px;">${this.tr("LAN Direct &middot; Zero Open Ports", "同网直连 · 零端口暴露")}</div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.6;">${this.tr("Happy Eyeballs switches to <code>:24444</code> LAN direct (&lt;1ms) on shared Wi-Fi, while the coordinator binds strictly to <code>127.0.0.1</code> behind Cloudflare Tunnel.", "同 Wi-Fi 下通过 Happy Eyeballs 秒切至 <code>:24444</code> 局域网直连（&lt;1ms），主控容器仅监听 <code>127.0.0.1</code> 由 Cloudflare Tunnel 接管 HTTPS。")}</p>
                  </div>
                  <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">SmartPath · CF Tunnel WSS</span>
                </div>
              </div>
            </div>

            <div class="pm-frame">
              <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
              <span class="pm-corner bl"></span><span class="pm-corner br"></span>
              <div class="pm-table-wrap">
                <table class="pm-table">
                  <thead>
                    <tr>
                      <th>${this.tr("Approach", "方案")}</th>
                      <th>${this.tr("Persistent PTY", "常驻 Agent 会话")}</th>
                      <th>${this.tr("Zero-Knowledge E2EE", "零知识 E2EE")}</th>
                      <th>${this.tr("Auto Wi-Fi Direct", "Wi-Fi 自动直连")}</th>
                      <th>${this.tr("Zero Public Ports", "CF 零公网端口")}</th>
                      <th>${this.tr("RFC 8628 Auth", "设备码浏览器绑机")}</th>
                      <th>${this.tr("Native macOS App", "原生 Mac 三栏端")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr class="is-coda">
                      <td>Coda Mesh</td>
                      <td><span class="pm-badge green"><span class="pm-status-dot"></span>Ready</span></td>
                      <td><span class="pm-badge green">ChaCha20</span></td>
                      <td><span class="pm-badge blue">&lt;1 ms LAN</span></td>
                      <td><span class="pm-badge green">127.0.0.1</span></td>
                      <td><span class="pm-badge green">RFC 8628</span></td>
                      <td><span class="pm-badge blue">Rust GPUI</span></td>
                    </tr>
                    <tr>
                      <td>SSH + tmux</td>
                      <td>${this.tr("Session kept", "Session 保持")}</td>
                      <td><span class="pm-dash-muted">Hop-by-hop</span></td>
                      <td><span class="pm-dash-muted">${this.tr("Manual IP switch", "手动切换 IP")}</span></td>
                      <td><span class="pm-dash-muted">${this.tr("Exposes port 22", "需开放 22 端口")}</span></td>
                      <td><span class="pm-dash-muted">${this.tr("Manual SSH keys", "手动传公钥")}</span></td>
                      <td><span class="pm-dash-muted">&mdash;</span></td>
                    </tr>
                    <tr>
                      <td>Cloud Web IDE</td>
                      <td><span class="pm-dash-muted">${this.tr("Resets offline", "断网重置")}</span></td>
                      <td><span class="pm-dash-muted">${this.tr("Server plaintext", "服务端明文")}</span></td>
                      <td><span class="pm-dash-muted">${this.tr("Public WAN only", "全程走公网")}</span></td>
                      <td>${this.tr("Reverse proxy", "支持反向代理")}</td>
                      <td><span class="pm-dash-muted">&mdash;</span></td>
                      <td><span class="pm-dash-muted">Electron</span></td>
                    </tr>
                    <tr>
                      <td>${this.tr("Remote Desktop", "传统远程桌面")}</td>
                      <td>${this.tr("Desktop pixels", "保留桌面画面")}</td>
                      <td><span class="pm-dash-muted">${this.tr("Video stream", "视频像素流")}</span></td>
                      <td><span class="pm-dash-muted">${this.tr("High bandwidth", "高带宽占用")}</span></td>
                      <td><span class="pm-dash-muted">${this.tr("Port forwarding", "需端口映射")}</span></td>
                      <td><span class="pm-dash-muted">${this.tr("Static PIN", "静态配对码")}</span></td>
                      <td><span class="pm-dash-muted">&mdash;</span></td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        </section>

        <!-- BAND 5: Unauthenticated Multi-Platform Downloads -->
        <section id="downloads" class="pm-band">
          <div class="pm-rail">
            <div class="pm-section-head">
              <div>
                <div class="pm-eyebrow">${this.tr("Downloads · Latest Release", "全端下载 · 最新正式版")}</div>
                <h2 class="pm-h2">${this.tr("Download for every platform, no sign-in required.", "免登录获取全端安装包。")}</h2>
                <p class="pm-sub">${this.tr(
                  'Sign into the same Coda account after installing, or pair headless hosts via <a href="/activate" data-nav="/activate" style="color:var(--signal-blue);text-decoration:underline;"><code>/activate</code></a> device authorization.',
                  '任意设备安装后登录同一个 Coda 账号，或通过 <a href="/activate" data-nav="/activate" style="color:var(--signal-blue);text-decoration:underline;"><code>/activate</code></a> 设备码授权即可实时互通。'
                )}</p>
              </div>
              ${dl ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${dl.releases_page}" target="_blank">${this.tr("GitHub Releases &amp; Checksums", "查看 GitHub Releases &amp; Checksums")} ${svgIcon("arrowRight", 13)}</a>` : ""}
            </div>

            <div class="pm-frame">
              <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
              <span class="pm-corner bl"></span><span class="pm-corner br"></span>
              <div class="pm-hairline-grid cols-2">
                <div class="pm-grid-cell">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;margin-bottom:10px;">
                      <strong style="display:inline-flex;align-items:center;gap:8px;font-size:15px;">${svgIcon("laptop", 16)} ${this.tr("macOS Desktop (Coda.app)", "macOS 桌面端 (Coda.app)")}</strong>
                      <span class="pm-badge blue">Apple Silicon &middot; Hybrid</span>
                    </div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.65;">
                      ${this.tr(
                        "Native Rust + GPUI 3-pane desktop workbench: orchestrate remote Linux cloud hosts from your Mac, or expose your Mac as a live workstation for mobile access.",
                        "原生 Rust + GPUI 三栏桌面工作台：既可从 Mac 控制多台云端 Linux 主机，也能将本机作为远端工作站供移动端随时连接。"
                      )}
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center;">
                    ${dl ? `<a class="pm-btn pm-btn-primary pm-btn-sm" href="${dl.desktop_macos_dmg}" target="_blank">${this.tr("Download .dmg", "下载 .dmg 安装包")}</a>` : ""}
                    ${dl ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${dl.desktop_macos_tar}" target="_blank">${this.tr("Full .tar.gz", "全套 .tar.gz")}</a>` : ""}
                    <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">macOS 14+</span>
                  </div>
                </div>

                <div class="pm-grid-cell">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;margin-bottom:10px;">
                      <strong style="display:inline-flex;align-items:center;gap:8px;font-size:15px;">${svgIcon("server", 16)} Server / Host Daemon</strong>
                      <span class="pm-badge">x86_64 &amp; ARM64</span>
                    </div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.65;">
                      ${this.tr(
                        "Headless daemon running on your dev host, bridging local <code>herdr-core (hc)</code> sessions and Git worktrees with one-command <code>coda-daemon login</code> enrollment.",
                        "部署在开发主机上的无头守护进程，桥接本机 <code>herdr-core (hc)</code> 会话与 Git Worktree，支持 <code>coda-daemon login</code> 一键入网。"
                      )}
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${dl ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${dl.daemon_linux_x64}" target="_blank">Linux x86_64</a>` : ""}
                    ${dl ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${dl.daemon_linux_arm64}" target="_blank">Linux ARM64</a>` : ""}
                    ${dl ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${dl.daemon_macos_arm64}" target="_blank">macOS Daemon</a>` : ""}
                  </div>
                </div>

                <div class="pm-grid-cell">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;margin-bottom:10px;">
                      <strong style="display:inline-flex;align-items:center;gap:8px;font-size:15px;">${svgIcon("smartphone", 16)} ${this.tr("iOS Mobile / Safari PWA", "iOS 移动端 / Safari PWA")}</strong>
                      <span class="pm-badge green">Thumb Keypad</span>
                    </div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.65;">
                      ${this.tr(
                        'Open this site in iPhone Safari and choose "Add to Home Screen" for full-screen 2-row tactile terminal keys, Whisper voice, and sub-ms LAN P2P—no App Store gate required.',
                        "使用 iPhone Safari 打开本页面并「添加到主屏幕」，即可零审核全屏使用双排终端触控键盘、Whisper 语音与 E2EE 局域网直连。"
                      )}
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${
                      dl?.ios_beta_url
                        ? `<a class="pm-btn pm-btn-primary pm-btn-sm" href="${this.escapeHtml(dl.ios_beta_url)}" target="_blank">${this.tr("Open TestFlight Beta", "打开 TestFlight Beta")}</a>`
                        : `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${this.user ? "/console" : "/login"}" data-nav="${this.user ? "/console" : "/login"}">${this.tr("Open Mobile Web Console", "打开 Web 移动端控制台")}</a>`
                    }
                  </div>
                </div>

                <div class="pm-grid-cell">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;margin-bottom:10px;">
                      <strong style="display:inline-flex;align-items:center;gap:8px;font-size:15px;">${svgIcon("cloud", 16)} ${this.tr("Self-Hosted Coordinator Bundle", "自建主控端部署套件")}</strong>
                      <span class="pm-badge">Docker + CF Tunnel</span>
                    </div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.65;">
                      ${this.tr(
                        "Turnkey Docker Compose bundle with PostgreSQL 18, Coordinator, and raw TCP relay scripts—bound strictly to <code>127.0.0.1</code> for zero-port Cloudflare Tunnel ingress.",
                        "内置 PostgreSQL 18、Coordinator 与国内免备案裸 TCP 中继编排脚本，默认仅绑定 <code>127.0.0.1</code> 由内网穿透接入。"
                      )}
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${dl ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${dl.deploy_bundle}" target="_blank">${this.tr("Download deploy-bundle", "下载 deploy-bundle")}</a>` : ""}
                    ${dl ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${dl.releases_page}" target="_blank">GitHub Releases</a>` : ""}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- BAND 6: Clean Closing Architectural CTA (No Inline Auth Form) -->
        <section class="pm-band">
          <div class="pm-rail">
            <div class="pm-frame">
              <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
              <span class="pm-corner bl"></span><span class="pm-corner br"></span>
              <div class="pm-cta-banner">
                <div style="max-width:560px;">
                  <div class="pm-eyebrow">Ready to Deploy</div>
                  <h2 class="pm-h2">${this.tr("Spin up your zero-knowledge dev mesh in two minutes.", "两分钟建立你的零知识开发网格。")}</h2>
                  <p style="margin-top:8px;font-size:14px;color:var(--text-secondary);line-height:1.65;">
                    ${this.tr(
                      "Create a private account or run <code>coda-daemon login</code> in your terminal, then manage all your hosts from the standalone console and device activation portal.",
                      "注册私有账号或在终端执行 <code>coda-daemon login</code>，随后前往独立的控制台与设备授权页管理你的所有主机。"
                    )}
                  </p>
                </div>
                <div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
                  ${
                    this.user
                      ? `<a href="/console" data-nav="/console" class="pm-btn pm-btn-primary pm-btn-lg">${this.tr("Open Console", "进入控制台")} ${svgIcon("arrowRight", 14)}</a>`
                      : `
                    <a href="/register" data-nav="/register" class="pm-btn pm-btn-primary pm-btn-lg">${this.tr("Create Free Account", "免费创建 Coda 账号")}</a>
                    <a href="/login" data-nav="/login" class="pm-btn pm-btn-secondary pm-btn-lg">${this.tr("Sign In", "登录已有账号")} ${svgIcon("arrowRight", 14)}</a>
                  `
                  }
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- Institutional 4-Column Footer -->
        <footer class="pm-footer-wrap">
          <div class="pm-footer-grid">
            <div>
              <a class="pm-brand" href="/" data-nav="/">
                ${codaLogoSVG(20)}
                <span>Coda Mesh</span>
              </a>
              <p style="margin-top:10px;font-size:12.5px;color:var(--text-secondary);line-height:1.65;max-width:300px;">
                ${this.tr(
                  "End-to-end zero-knowledge control plane and persistent terminal mesh engineered for cross-device AI workflows.",
                  "为跨设备 AI Agent 工程设计的端到端零知识控制面与常驻终端隧道。"
                )}
              </p>
              <div style="margin-top:12px;">
                <span class="pm-badge green"><span class="pm-status-dot"></span>All Systems Operational</span>
              </div>
            </div>
            <div>
              <div class="pm-footer-col-title">${this.tr("Product &amp; Architecture", "产品与架构")}</div>
              <div class="pm-footer-links">
                <a href="#workbench">${this.tr("Workbench Preview", "工作台预览")}</a>
                <a href="#bento">${this.tr("60/120Hz Live Grid", "60/120Hz 实时网格")}</a>
                <a href="#architecture">${this.tr("Zero-Knowledge Pipeline", "零知识加密流水线")}</a>
              </div>
            </div>
            <div>
              <div class="pm-footer-col-title">${this.tr("Clients &amp; Deployment", "客户端与部署")}</div>
              <div class="pm-footer-links">
                <a href="#downloads">${this.tr("macOS Desktop (.dmg)", "macOS 桌面端 (.dmg)")}</a>
                <a href="#downloads">Linux / macOS Daemon</a>
                <a href="#downloads">Docker + CF Tunnel</a>
                ${dl ? `<a href="${dl.releases_page}" target="_blank">${this.tr("GitHub Releases (Latest)", "GitHub Releases (最新版)")}</a>` : ""}
              </div>
            </div>
            <div>
              <div class="pm-footer-col-title">${this.tr("Console &amp; Access", "控制台与接入")}</div>
              <div class="pm-footer-links">
                <a href="/login" data-nav="/login">${this.tr("Sign In (/login)", "登录账号 (/login)")}</a>
                <a href="/register" data-nav="/register">${this.tr("Create Account (/register)", "注册账号 (/register)")}</a>
                <a href="/activate" data-nav="/activate">${this.tr("Device Auth (/activate)", "设备码绑机 (/activate)")}</a>
                <a href="/console" data-nav="/console">${this.tr("Web Console (/console)", "Web 控制台 (/console)")}</a>
              </div>
            </div>
          </div>
          <div class="pm-footer-bottom">
            <span class="coda-mono">Coda Control Plane &middot; ${this.escapeHtml(baseUrl)}</span>
            <span class="coda-mono">Ed25519 + X25519 + ChaCha20-Poly1305 &middot; ${this.escapeHtml(ver)}</span>
          </div>
        </footer>

        ${this.renderCommandPalette(installCmd, ver, dl)}
      </div>
    `;

    this.startBentoLoop();
    this.bindCommonNavigation(root);

    root.querySelectorAll<HTMLElement>("[data-hero-node]").forEach((el) => {
      el.addEventListener("click", () => {
        this.heroNode = el.dataset.heroNode as "mac" | "linux" | "iphone";
        root.querySelectorAll<HTMLElement>("[data-hero-node]").forEach((btn) => {
          btn.classList.toggle("active", btn.dataset.heroNode === this.heroNode);
        });
        const body = document.getElementById("hero-workbench-body");
        if (body) {
          body.innerHTML = this.renderHeroWorkbenchBody(baseUrl);
          this.bindCommonNavigation(body);
        }
      });
    });

    root.querySelectorAll<HTMLElement>("[data-preview-tab]").forEach((el) => {
      el.addEventListener("click", () => {
        this.previewTab = el.dataset.previewTab as "coda" | "server" | "ios";
        root.querySelectorAll<HTMLElement>("[data-preview-tab]").forEach((btn) => {
          btn.classList.toggle("active", btn.dataset.previewTab === this.previewTab);
        });
        const body = document.getElementById("bento-host-preview");
        if (body) body.innerHTML = this.renderHostPreviewBody(baseUrl);
      });
    });

    document.getElementById("btn-copy-install")?.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(installCmd);
        this.copiedToast = "ok";
        this.render();
        setTimeout(() => {
          this.copiedToast = "";
          if (this.route === "home") this.render();
        }, 1800);
      } catch {
        this.copiedToast = "manual";
        this.render();
      }
    });

    this.bindCommandPalette(installCmd);
  }

  private renderCommandPalette(installCmd: string, ver: string, dl: SiteInfo["downloads"] | undefined): string {
    if (!this.showCmdPalette) return "";
    return `
      <div class="modal-mask" id="cmd-mask">
        <div class="modal-card">
          <div style="display:flex;align-items:center;justify-content:space-between;padding-bottom:12px;border-bottom:1px solid var(--border-primary);">
            <span style="display:inline-flex;align-items:center;gap:8px;font-size:13.5px;font-weight:600;">${svgIcon("search", 14)} ${this.tr("Coda Quick Command Palette", "Coda 快捷导航面板")}</span>
            <button id="btn-close-cmd" type="button" class="circle-btn">${svgIcon("close", 13)}</button>
          </div>
          <div class="cmd-list">
            <button type="button" class="cmd-row" id="cmd-copy-install">
              <span style="display:inline-flex;align-items:center;gap:8px;">${svgIcon("copy", 14)} ${this.tr("Copy one-line install command (curl | sh)", "复制一行接入命令 (curl | sh)")}</span>
              <span class="pm-kbd">Enter</span>
            </button>
            <button type="button" class="cmd-row" id="cmd-jump-login">
              <span style="display:inline-flex;align-items:center;gap:8px;">${svgIcon("terminal", 14)} ${
                this.user
                  ? this.tr("Open Web Console (/console)", "进入在线 Web 控制台 (/console)")
                  : this.tr("Go to Sign In (/login)", "前往登录页 (/login)")
              }</span>
              <span class="pm-kbd">${this.user ? "/console" : "/login"}</span>
            </button>
            ${
              !this.user
                ? `<button type="button" class="cmd-row" id="cmd-jump-register">
                    <span style="display:inline-flex;align-items:center;gap:8px;">${svgIcon("user", 14)} ${this.tr("Create Account (/register)", "前往注册页 (/register)")}</span>
                    <span class="pm-kbd">/register</span>
                  </button>`
                : ""
            }
            <button type="button" class="cmd-row" id="cmd-jump-activate">
              <span style="display:inline-flex;align-items:center;gap:8px;">${svgIcon("key", 14)} ${this.tr("Authorize Device Code (/activate)", "设备码浏览器授权 (/activate)")}</span>
              <span class="pm-kbd">/activate</span>
            </button>
            ${
              dl
                ? `<a class="cmd-row" href="${dl.desktop_macos_dmg}" target="_blank">
                    <span style="display:inline-flex;align-items:center;gap:8px;">${svgIcon("laptop", 14)} ${this.tr("Download macOS Desktop (.dmg)", "下载 macOS 桌面端 (.dmg)")}</span>
                    <span class="pm-badge">${this.tr("Latest", "最新版")}</span>
                  </a>`
                : ""
            }
            <button type="button" class="cmd-row" id="cmd-toggle-lang">
              <span style="display:inline-flex;align-items:center;gap:8px;">${svgIcon("globe", 14)} ${this.tr("Switch Language to 中文 (Simplified Chinese)", "切换界面语言为 English")}</span>
              <span class="pm-kbd">${this.locale === "en" ? "中文" : "EN"}</span>
            </button>
            <button type="button" class="cmd-row" id="cmd-toggle-theme">
              <span style="display:inline-flex;align-items:center;gap:8px;">${svgIcon(this.themePref === "light" ? "moon" : "sun", 14)} ${this.tr(`Toggle Theme (current: ${this.themePref})`, `切换明暗主题 (当前: ${this.themePref})`)}</span>
              <span class="pm-kbd">Theme</span>
            </button>
          </div>
        </div>
      </div>
    `;
  }

  private bindCommandPalette(installCmd: string) {
    document.getElementById("btn-cmd-k")?.addEventListener("click", () => {
      this.showCmdPalette = true;
      this.render();
    });
    document.getElementById("btn-close-cmd")?.addEventListener("click", () => {
      this.showCmdPalette = false;
      this.render();
    });
    document.getElementById("cmd-mask")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "cmd-mask") {
        this.showCmdPalette = false;
        this.render();
      }
    });
    document.getElementById("cmd-copy-install")?.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(installCmd);
        this.copiedToast = "ok";
      } catch {
        this.copiedToast = "manual";
      }
      this.showCmdPalette = false;
      this.render();
      setTimeout(() => {
        this.copiedToast = "";
        if (this.route === "home") this.render();
      }, 1800);
    });
    document.getElementById("cmd-jump-login")?.addEventListener("click", () => {
      this.navigate(this.user ? "/console" : "/login");
    });
    document.getElementById("cmd-jump-register")?.addEventListener("click", () => {
      this.navigate("/register");
    });
    document.getElementById("cmd-jump-activate")?.addEventListener("click", () => {
      this.navigate("/activate");
    });
    document.getElementById("cmd-toggle-lang")?.addEventListener("click", () => {
      this.showCmdPalette = false;
      this.applyLocale(this.locale === "en" ? "zh" : "en", true);
    });
    document.getElementById("cmd-toggle-theme")?.addEventListener("click", () => {
      const next: ThemePreference = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
      this.applyTheme(next, true);
      this.showCmdPalette = false;
      this.render();
    });
  }

  private renderAuthView(root: HTMLElement, baseUrl: string, ver: string) {
    const isReg = this.route === "register" || this.authMode === "register";
    const openReg = this.siteInfo?.open_registration ?? true;
    const codeSuffix = this.pendingActivationCode
      ? `?code=${encodeURIComponent(this.pendingActivationCode)}`
      : "";

    root.innerHTML = `
      <div class="pm-auth-shell">
        <div class="pm-auth-main">
          <div class="pm-auth-topbar">
            <a href="/" data-nav="/" class="pm-btn pm-btn-ghost pm-btn-sm">
              ${svgIcon("arrowLeft", 14)} ${this.tr("Back to Home", "返回官网首页")}
            </a>
            <div style="display:flex;align-items:center;gap:8px;">
              ${this.renderLangSwitcher()}
              ${this.renderThemeSwitcher()}
            </div>
          </div>

          <div class="pm-auth-box">
            <div class="pm-auth-emblem">${codaLogoSVG(26)}</div>
            <h1 style="font-size:24px;font-weight:600;letter-spacing:-0.025em;margin-bottom:6px;">
              ${
                isReg
                  ? this.tr("Create your Coda zero-knowledge account", "创建你的 Coda 零知识账号")
                  : this.tr("Sign in to Coda Control Plane", "登录到 Coda 控制面")
              }
            </h1>
            <p style="font-size:13px;color:var(--text-secondary);line-height:1.6;margin-bottom:20px;">
              ${
                this.pendingActivationCode
                  ? this.tr(
                      `Sign in to authorize device code <code class="coda-mono">${this.escapeHtml(this.pendingActivationCode)}</code>.`,
                      `完成登录后即可批准设备码 <code class="coda-mono">${this.escapeHtml(this.pendingActivationCode)}</code> 接入。`
                    )
                  : isReg
                  ? this.tr(
                      "The first registered user becomes site administrator automatically. Your nodes sync end-to-end encrypted across devices.",
                      "首个注册账号自动设为站点管理员，接入后可在所有设备间端到端加密同步。"
                    )
                  : this.tr(
                      "Enter your private terminal mesh to attach and orchestrate remote Herdr PTY sessions.",
                      "进入属于你的私有终端网络，实时接管远端 Herdr PTY 会话。"
                    )
              }
            </p>

            <div class="pm-auth-tabs" role="tablist">
              <button type="button" class="pm-auth-tab ${!isReg ? "active" : ""}" id="tab-auth-login">${this.tr("Sign In", "账号登录")}</button>
              <button type="button" class="pm-auth-tab ${isReg ? "active" : ""}" id="tab-auth-register">${this.tr("Create Account", "免费注册")}</button>
            </div>

            ${
              this.errorMsg
                ? `<div style="margin-bottom:14px;padding:10px 12px;border-radius:8px;background:var(--rose-bg);border:1px solid var(--rose-bd);color:var(--rose);font-size:12.5px;">${this.escapeHtml(this.errorMsg)}</div>`
                : ""
            }

            <form id="auth-form">
              ${
                isReg
                  ? `
                <div class="pm-field">
                  <label class="pm-field-label">${this.tr("Email", "电子邮箱")}</label>
                  <div class="pm-input-wrap">
                    <span class="pm-input-icon">${svgIcon("mail", 14)}</span>
                    <input name="email" type="email" class="pm-input" required placeholder="dev@example.com" autocomplete="email" />
                  </div>
                </div>
                <div style="display:grid;grid-template-columns:1fr 1fr;gap:10px;">
                  <div class="pm-field">
                    <label class="pm-field-label">${this.tr("Username (Handle)", "用户名 (Handle)")}</label>
                    <div class="pm-input-wrap">
                      <span class="pm-input-icon">${svgIcon("user", 14)}</span>
                      <input name="handle" class="pm-input" required placeholder="alice" autocomplete="username" />
                    </div>
                  </div>
                  <div class="pm-field">
                    <label class="pm-field-label">${this.tr("Display Name", "显示昵称")}</label>
                    <div class="pm-input-wrap">
                      <span class="pm-input-icon">${svgIcon("user", 14)}</span>
                      <input name="display_name" class="pm-input" required placeholder="Alice" />
                    </div>
                  </div>
                </div>
                <div class="pm-field">
                  <label class="pm-field-label">${this.tr("Password", "登录密码")} <span>${this.tr("At least 6 characters", "至少 6 位")}</span></label>
                  <div class="pm-input-wrap has-right">
                    <span class="pm-input-icon">${svgIcon("lock", 14)}</span>
                    <input id="auth-password-input" type="${this.showPassword ? "text" : "password"}" name="password" class="pm-input" required autocomplete="new-password" />
                    <button id="btn-toggle-pw" type="button" class="pm-pw-toggle" title="${this.tr("Show or hide password", "显示或隐藏密码")}">${svgIcon(this.showPassword ? "eyeOff" : "eye", 14)}</button>
                  </div>
                </div>
                ${
                  openReg
                    ? `<div style="font-size:12px;color:var(--emerald);margin-bottom:14px;display:flex;align-items:center;gap:6px;">${svgIcon("check", 13)} ${this.tr("Open registration is enabled &mdash; no invite code required.", "当前站点已开放自由注册，无需填写邀请码。")}</div>`
                    : `<div class="pm-field">
                        <label class="pm-field-label">${this.tr("Invite Code", "邀请码")} <span>${this.tr("Leave blank for first admin", "首位管理员留空")}</span></label>
                        <div class="pm-input-wrap">
                          <span class="pm-input-icon">${svgIcon("key", 14)}</span>
                          <input name="invite_code" class="pm-input" placeholder="CODA-XXXXXX" />
                        </div>
                      </div>`
                }
              `
                  : `
                <div class="pm-field">
                  <label class="pm-field-label">${this.tr("Email or Handle", "邮箱或用户名")}</label>
                  <div class="pm-input-wrap">
                    <span class="pm-input-icon">${svgIcon("mail", 14)}</span>
                    <input name="email_or_handle" class="pm-input" required placeholder="${this.tr("alice or dev@example.com", "alice 或 dev@example.com")}" autocomplete="username" />
                  </div>
                </div>
                <div class="pm-field">
                  <label class="pm-field-label">${this.tr("Password", "密码")}</label>
                  <div class="pm-input-wrap has-right">
                    <span class="pm-input-icon">${svgIcon("lock", 14)}</span>
                    <input id="auth-password-input" type="${this.showPassword ? "text" : "password"}" name="password" class="pm-input" required autocomplete="current-password" />
                    <button id="btn-toggle-pw" type="button" class="pm-pw-toggle" title="${this.tr("Show or hide password", "显示或隐藏密码")}">${svgIcon(this.showPassword ? "eyeOff" : "eye", 14)}</button>
                  </div>
                </div>
              `
              }
              <button type="submit" class="pm-btn pm-btn-primary pm-btn-lg" style="width:100%;margin-top:4px;">
                ${isReg ? this.tr("Create Account &amp; Open Console", "立即注册并进入控制台") : this.tr("Sign In to Console", "登录并进入控制台")} ${svgIcon("arrowRight", 14)}
              </button>
            </form>

            <div style="margin-top:20px;padding-top:16px;border-top:1px solid var(--border-primary);display:flex;flex-direction:column;gap:8px;font-size:12.5px;color:var(--text-secondary);">
              <a href="/activate${codeSuffix}" data-nav="/activate${codeSuffix}" style="display:inline-flex;align-items:center;justify-content:space-between;color:var(--signal-blue);">
                <span>${this.tr("Already ran <code>coda-daemon login</code>? Authorize device code", "已在终端运行 <code>coda-daemon login</code>？前往设备码激活")}</span>
                ${svgIcon("arrowRight", 13)}
              </a>
              <div style="font-size:11.5px;color:var(--text-tertiary);">
                ${this.tr(
                  "Your browser generates and stores an independent <code>Ed25519 + X25519</code> device keypair locally.",
                  "浏览器会在本地生成并保存独立的 <code>Ed25519 + X25519</code> 设备密钥对。"
                )}
              </div>
            </div>
          </div>
        </div>

        ${this.renderAuthShowcasePanel(baseUrl, ver, false)}
      </div>
    `;

    this.bindCommonNavigation(root);

    document.getElementById("tab-auth-login")?.addEventListener("click", () => {
      this.authMode = "login";
      this.navigate(`/login${codeSuffix}`);
    });
    document.getElementById("tab-auth-register")?.addEventListener("click", () => {
      this.authMode = "register";
      this.navigate(`/register${codeSuffix}`);
    });
    document.getElementById("btn-toggle-pw")?.addEventListener("click", () => {
      this.showPassword = !this.showPassword;
      const inp = document.getElementById("auth-password-input") as HTMLInputElement | null;
      const btn = document.getElementById("btn-toggle-pw");
      if (inp) inp.type = this.showPassword ? "text" : "password";
      if (btn) btn.innerHTML = svgIcon(this.showPassword ? "eyeOff" : "eye", 14);
    });

    document.getElementById("auth-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const payload = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries());
      try {
        const res = isReg
          ? await this.api("/api/v1/auth/register", "POST", payload)
          : await this.api("/api/v1/auth/login", "POST", payload);
        this.token = res.token;
        this.user = res.user;
        this.errorMsg = "";
        localStorage.setItem("coda_token", res.token);
        await this.bootstrapMesh();
        if (this.pendingActivationCode) {
          this.navigate(`/activate?code=${encodeURIComponent(this.pendingActivationCode)}`);
        } else {
          this.navigate("/console");
        }
      } catch (err: any) {
        this.errorMsg = err.message;
        this.render();
      }
    });
  }

  private renderActivateView(root: HTMLElement, baseUrl: string, ver: string) {
    const code = this.pendingActivationCode || "";
    const normalized = code.replace(/[^A-Za-z0-9]/g, "").toUpperCase().slice(0, 8);
    const keycapChars = (normalized.padEnd(8, "•")).split("");
    const codeSuffix = code ? `?code=${encodeURIComponent(code)}` : "";

    root.innerHTML = `
      <div class="pm-auth-shell">
        <div class="pm-auth-main">
          <div class="pm-auth-topbar">
            <a href="/" data-nav="/" class="pm-btn pm-btn-ghost pm-btn-sm">
              ${svgIcon("arrowLeft", 14)} ${this.tr("Back to Home", "返回官网首页")}
            </a>
            <div style="display:flex;align-items:center;gap:8px;">
              ${this.user ? `<a href="/console" data-nav="/console" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("Console", "控制台")}</a>` : ""}
              ${this.renderLangSwitcher()}
              ${this.renderThemeSwitcher()}
            </div>
          </div>

          <div class="pm-auth-box" style="max-width:420px;">
            <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;">
              <span class="pm-badge amber">${svgIcon("key", 12)} RFC 8628 Device Flow</span>
              ${this.user ? `<span class="pm-badge green"><span class="pm-status-dot"></span>@${this.escapeHtml(this.user.handle)}</span>` : ""}
            </div>

            <h1 style="font-size:24px;font-weight:600;letter-spacing:-0.025em;margin-bottom:8px;">
              ${this.tr("Authorize Terminal Device", "终端设备授权接入")}
            </h1>
            <p style="font-size:13px;color:var(--text-secondary);line-height:1.65;margin-bottom:18px;">
              ${this.tr(
                "Verify the 8-character user code printed in your terminal to bind that host to your private zero-knowledge mesh.",
                "核对终端输出的 8 位授权码，将其对应主机绑定到你的私有零知识网络。"
              )}
            </p>

            <div class="bento-otp-group" style="margin-bottom:18px;">
              ${keycapChars.slice(0, 4).map((c) => `<div class="bento-otp-cell">${this.escapeHtml(c)}</div>`).join("")}
              <span class="bento-otp-dash">-</span>
              ${keycapChars.slice(4, 8).map((c) => `<div class="bento-otp-cell">${this.escapeHtml(c)}</div>`).join("")}
            </div>

            <form id="activate-lookup-form" style="display:flex;gap:8px;margin-bottom:16px;">
              <input name="code" class="pm-input coda-mono" style="margin:0;flex:1;text-transform:uppercase;letter-spacing:0.08em;" value="${this.escapeHtml(code)}" placeholder="${this.tr("Enter code, e.g. CODA-7F29", "输入授权码 如 CODA-7F29")}" required />
              <button type="submit" class="pm-btn pm-btn-secondary" style="height:40px;">${this.tr("Look Up", "查询设备")}</button>
            </form>

            ${
              this.errorMsg
                ? `<div style="margin-bottom:14px;padding:10px 12px;border-radius:8px;background:var(--rose-bg);border:1px solid var(--rose-bd);color:var(--rose);font-size:12.5px;">${this.escapeHtml(this.errorMsg)}</div>`
                : ""
            }

            ${
              this.pendingDeviceInfo
                ? `
              <div class="pm-frame" style="padding:16px;margin-bottom:18px;">
                <span class="pm-corner tl"></span><span class="pm-corner tr"></span>
                <span class="pm-corner bl"></span><span class="pm-corner br"></span>
                <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                  <strong style="font-size:14px;">${this.escapeHtml(this.pendingDeviceInfo.device_name)}</strong>
                  <span class="pm-badge ${this.pendingDeviceInfo.status === "pending" ? "amber" : "green"}">${this.escapeHtml(this.pendingDeviceInfo.status)}</span>
                </div>
                <div class="coda-mono" style="font-size:12px;color:var(--text-secondary);line-height:1.7;">
                  <div>Platform : ${this.escapeHtml(this.pendingDeviceInfo.platform)}</div>
                  <div>Role &nbsp;&nbsp;&nbsp;&nbsp;: ${this.escapeHtml(this.pendingDeviceInfo.role)}</div>
                  <div>User Code: ${this.escapeHtml(this.pendingDeviceInfo.user_code)}</div>
                </div>
              </div>
            `
                : `<div class="bento-well" style="font-size:12.5px;color:var(--text-secondary);line-height:1.6;margin-bottom:18px;">
                    ${this.tr(
                      `Run <code>coda-daemon login --server ${this.escapeHtml(baseUrl)}</code> on your Linux or macOS host to generate a one-time activation link or code.`,
                      `在目标 Linux 或 macOS 终端执行 <code>coda-daemon login --server ${this.escapeHtml(baseUrl)}</code> 获取一次性激活链接或授权码。`
                    )}
                  </div>`
            }

            ${
              this.user
                ? `
              <div style="display:flex;flex-direction:column;gap:10px;">
                <button id="btn-approve-device" type="button" class="pm-btn pm-btn-primary pm-btn-lg" style="width:100%;" ${!code ? "disabled" : ""}>
                  ${svgIcon("check", 14)} ${this.tr(
                    `Approve Device for @${this.escapeHtml(this.user.handle)}`,
                    `批准此设备接入 @${this.escapeHtml(this.user.handle)} 的网络`
                  )}
                </button>
                <a href="/console" data-nav="/console" class="pm-btn pm-btn-secondary pm-btn-lg" style="width:100%;">
                  ${this.tr("Cancel &amp; Return to Console", "取消并返回在线控制台")}
                </a>
              </div>
            `
                : `
              <div style="display:flex;flex-direction:column;gap:10px;">
                <a href="/login${codeSuffix}" data-nav="/login${codeSuffix}" class="pm-btn pm-btn-primary pm-btn-lg" style="width:100%;">
                  ${this.tr("Sign in to Approve Device", "登录现有账号以批准接入")} ${svgIcon("arrowRight", 14)}
                </a>
                <a href="/register${codeSuffix}" data-nav="/register${codeSuffix}" class="pm-btn pm-btn-secondary pm-btn-lg" style="width:100%;">
                  ${this.tr("Create Account &amp; Authorize Device", "创建新账号并授权此设备")}
                </a>
              </div>
            `
            }
          </div>
        </div>

        ${this.renderAuthShowcasePanel(baseUrl, ver, true)}
      </div>
    `;

    this.bindCommonNavigation(root);

    document.getElementById("activate-lookup-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      const nextCode = String(fd.code || "").trim().toUpperCase();
      if (!nextCode) return;
      await this.lookupActivationCode(nextCode);
      window.history.replaceState({}, "", `/activate?code=${encodeURIComponent(nextCode)}`);
      this.render();
    });

    document.getElementById("btn-approve-device")?.addEventListener("click", async () => {
      if (!this.pendingActivationCode) return;
      try {
        const dev = await this.api("/api/v1/auth/device/approve", "POST", {
          user_code: this.pendingActivationCode,
        });
        this.pendingActivationCode = "";
        this.pendingDeviceInfo = null;
        this.errorMsg = "";
        this.activationSuccessMsg = this.tr(
          `Approved device ${dev.name} (${dev.platform})`,
          `已授权设备 ${dev.name} (${dev.platform})`
        );
        await this.refreshDevices();
        this.selectedDaemon = this.devices.find((d) => d.id === dev.id) || this.selectedDaemon;
        if (this.selectedDaemon?.is_online) {
          this.queryDaemonOverview(this.selectedDaemon);
          void this.tryNegotiateDirectLanWs(this.selectedDaemon);
        }
        this.navigate("/console");
      } catch (err: any) {
        this.errorMsg = err.message;
        this.render();
      }
    });
  }

  private renderConsoleView(root: HTMLElement, installCmd: string) {
    if (!this.user) return;
    const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr) || this.terminals[0];
    const nowTime = new Date().toTimeString().slice(0, 5);
    const isNarrowViewport = window.innerWidth < 900;
    const renderTouchKeypad = isNarrowViewport || this.showMobileKeypadOnDesktop;

    root.innerHTML = `
      <div class="shell-root">
        ${
          this.showSidebar
            ? `
          <!-- LEFT SIDEBAR: Account + Machine/Repo/Branch List -->
          <aside class="left-sidebar">
            <div style="padding:14px 16px;display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid var(--border-primary);">
              <div style="display:flex;align-items:center;gap:10px;min-width:0;">
                ${codaLogoSVG(24)}
                <div style="min-width:0;">
                  <div style="font-size:13px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${this.user.email}</div>
                  <div style="font-size:11px;color:var(--emerald);margin-top:2px;display:inline-flex;align-items:center;gap:4px;">
                    ${this.activeTransportMode === "lan_ws" ? `${svgIcon("zap", 11)} ${this.tr("LAN Direct", "局域网直连")}` : `${svgIcon("cloud", 11)} ${this.tr("Smart Relay", "智能中继")}`}
                    ${this.rttMs !== null ? `(${this.rttMs}ms)` : ""}
                  </div>
                </div>
              </div>
              <div style="display:flex;gap:6px;">
                <button id="btn-open-settings" class="circle-btn" title="${this.tr("Device pairing, invites & settings", "配对机器与好友邀请码")}">${svgIcon("settings", 14)}</button>
                <button id="btn-collapse-sidebar" class="circle-btn" title="${this.tr("Collapse sidebar", "收起侧边栏")}">${svgIcon("sidebar", 14)}</button>
              </div>
            </div>

            <div style="flex:1;overflow-y:auto;padding:12px;">
              ${
                this.devices.length === 0
                  ? `<div style="padding:12px;font-size:12px;color:var(--text-secondary);line-height:1.65;">
                      ${this.tr("No hosts paired to this account yet.", "当前账号下暂无在线主机。")}<br/><br/>
                      ${this.tr("Run on any Linux or macOS host:", "在任意 Linux / Mac 运行：")}<br/>
                      <code class="coda-mono" style="color:var(--emerald);word-break:break-all;">${this.escapeHtml(installCmd)}</code><br/><br/>
                      ${this.tr("Open the printed activation URL or click Settings at the top-right to approve.", "打开终端输出的激活链接即可接入，或点击右上角设置按钮手动授权。")}
                    </div>`
                  : this.devices
                      .map(
                        (d) => `
                  <div style="margin-bottom:18px;">
                    <div style="display:flex;align-items:center;gap:6px;padding:4px 6px;font-size:12px;color:var(--text-tertiary);">
                      <span style="width:7px;height:7px;border-radius:99px;background:${d.is_online ? "var(--emerald)" : "var(--text-quaternary)"};"></span>
                      <span style="color:var(--text-primary);font-weight:500;">coda</span>
                      <span>&middot; ${d.name}</span>
                    </div>
                    ${
                      this.selectedDaemon?.id === d.id && this.terminals.length > 0
                        ? this.terminals
                            .map(
                              (t) => `
                          <div class="branch-item ${this.activeTerminalAddr === t.address ? "active" : ""}" data-term="${t.address}">
                            <div style="display:flex;align-items:center;gap:10px;min-width:0;">
                              ${codaNodeDotSVG("#e06c50")}
                              <div style="min-width:0;">
                                <div class="coda-mono" style="font-size:13px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                                  ${t.name || t.address}
                                </div>
                                <div style="font-size:11px;color:var(--emerald);margin-top:2px;">
                                  ${t.status?.state || this.tr("Running", "终端运行中")}
                                </div>
                              </div>
                            </div>
                            <div class="coda-mono pm-num" style="font-size:11px;color:var(--emerald);">+184 <span style="color:var(--rose);">-19</span></div>
                          </div>
                        `
                            )
                            .join("")
                        : `
                        <div class="branch-item ${this.selectedDaemon?.id === d.id ? "active" : ""}" data-dev="${d.id}">
                          <div style="display:flex;align-items:center;gap:10px;">
                            ${codaNodeDotSVG(d.is_online ? "#e06c50" : "#52525b")}
                            <div>
                              <div class="coda-mono" style="font-size:13px;">main</div>
                              <div style="font-size:11px;color:${d.is_online ? "var(--emerald)" : "var(--text-tertiary)"};">
                                ${d.is_online ? this.tr("Click to load Herdr PTYs", "点击加载 Herdr 终端") : this.tr("Host offline", "主机离线")}
                              </div>
                            </div>
                          </div>
                        </div>
                      `
                    }
                  </div>
                `
                      )
                      .join("")
              }
            </div>
          </aside>
        `
            : ""
        }

        <!-- CENTER TERMINAL PANE -->
        <main class="main-terminal-col">
          <!-- Top Session Header + Overlapping Agent Avatars -->
          <div style="display:flex;align-items:flex-start;justify-content:space-between;padding-bottom:8px;border-bottom:1px solid var(--border-primary);">
            <div style="min-width:0;">
              <div style="font-size:15px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                ${activeTerm?.status?.title || activeTerm?.name || this.tr("Coda Interactive AI Terminal", "Coda 交互式 AI 终端会话")}
              </div>
              <div class="coda-mono" style="font-size:12px;color:var(--text-tertiary);margin-top:2px;">
                ${activeTerm?.cwd || "~/personal/github/coda"} &middot; main &middot; ${this.selectedDaemon?.name || "MacBook-Pro"}
              </div>
            </div>
            <div style="display:flex;align-items:center;gap:8px;">
              ${(this.terminals.length > 0 ? this.terminals : [{ address: "t1" } as any])
                .map(
                  (t, i) => `
                <button data-term="${t.address}" style="margin-left:${i === 0 ? "0" : "-8px"};width:34px;height:34px;border-radius:999px;background:var(--bg-card);border:${
                    t.address === this.activeTerminalAddr ? "2px solid var(--text-primary)" : "1px solid var(--border-strong)"
                  };display:flex;align-items:center;justify-content:center;cursor:pointer;z-index:${10 - i};">
                  ${codaNodeDotSVG(i % 2 === 0 ? "#e06c50" : "#3b82f6")}
                </button>
              `
                )
                .join("")}
            </div>
          </div>

          <!-- Live Terminal Viewport -->
          <div class="term-viewport" id="term-box">${
            this.terminalScreen
              ? this.escapeHtml(this.terminalScreen)
              : this.tr(
                  `  - Connected to PostgreSQL 18 multi-tenant control plane &amp; E2EE tunnel (X25519 + ChaCha20-Poly1305)\n  - Local Herdr Core (hc 0.1.20) online. Use '+' on the right rail to launch Claude Code / Codex / Gemini CLI.\n\n* Ready · ${this.rttMs !== null ? `RTT ${this.rttMs}ms` : "CF Tunnel WSS :443 & Direct LAN P2P Ready"}`,
                  `  - 已连接 PostgreSQL 18 多租户控制面与端到端加密隧道 (X25519 + ChaCha20-Poly1305)\n  - 本机 Herdr Core (hc 0.1.20) 在线，随时可通过右侧「+」拉起 Claude Code / Codex / Gemini CLI\n\n* Ready · ${this.rttMs !== null ? `RTT ${this.rttMs}ms` : "CF Tunnel WSS :443 & Direct LAN P2P Ready"}`
                )
          }

<div style="margin-top:14px;border-top:1px solid var(--border-primary);padding-top:6px;border-bottom:1px solid var(--border-primary);padding-bottom:6px;">&gt; ${this.formatCursorDraft(
            this.draftInput
          )}</div>
<div style="font-size:12px;color:var(--text-tertiary);margin-top:4px;">Opus 4.6 | 5h <span style="color:var(--rose);">100%</span> | ${this.tr("7d total", "7d 总")} <span style="color:var(--text-secondary);">77%</span> | <span style="color:var(--amber);">E2EE zstd</span></div>
<div style="font-size:12px;color:var(--coral);">bypass permissions on (shift+tab to cycle) <span style="color:var(--text-tertiary);">&middot; ${
            this.terminals.length || 1
          } agents &middot; PR</span></div></div>

          ${
            this.showKeyboardInputBar
              ? `
            <form id="kb-form" style="display:flex;gap:8px;margin-bottom:8px;">
              <input id="kb-input" class="pm-input" style="margin:0;flex:1;" value="${this.escapeHtml(
                this.draftInput
              )}" placeholder="${this.tr("Type a command or prompt for Herdr PTY...", "输入命令或中文 Prompt 发送至当前 Herdr 终端...")}" autofocus />
              <button type="submit" class="pm-btn pm-btn-primary">${this.tr("OK", "确定")}</button>
            </form>
          `
              : ""
          }

          ${
            renderTouchKeypad
              ? `
          <!-- Mobile 2-Row Tactile Keypad + Tall Enter -->
          <div class="keypad-grid">
            <div class="keypad-left">
              <div class="keypad-row">
                <button data-k="esc" class="k-btn" style="grid-column:span 2;">esc</button>
                <button data-k="shift+tab" class="k-btn" style="grid-column:span 2;">tab</button>
                <button id="btn-kb-toggle" class="k-btn" style="grid-column:span 3;display:flex;justify-content:space-around;padding:0 10px;">
                  ${svgIcon("keyboard", 14)}
                  <span style="width:1px;height:14px;background:var(--border-strong);"></span>
                  ${svgIcon("mic", 14)}
                </button>
                <button data-k="up" class="k-btn" style="grid-column:span 1;">↑</button>
                <button data-k="backspace" class="k-btn" style="grid-column:span 2;">⌫</button>
              </div>
              <div class="keypad-row">
                <button data-k="ctrl+c" class="k-btn" style="grid-column:span 2;color:var(--coral);font-weight:600;">^C</button>
                <button data-k="ctrl+d" class="k-btn" style="grid-column:span 2;font-weight:600;">^D</button>
                <button data-k="slash" class="k-btn" style="grid-column:span 2;font-weight:600;">/</button>
                <button data-k="left" class="k-btn" style="grid-column:span 1;">←</button>
                <button data-k="down" class="k-btn" style="grid-column:span 2;">↓</button>
                <button data-k="right" class="k-btn" style="grid-column:span 1;">→</button>
              </div>
            </div>
            <button data-k="enter" class="k-enter">↵</button>
          </div>`
              : `
          <!-- Desktop Prompt Composer Bar -->
          <form id="desktop-prompt-form" class="desktop-composer">
            <div class="desktop-composer-row">
              <span class="coda-mono" style="color:var(--emerald);font-weight:700;">&gt;</span>
              <input id="desktop-prompt-input" class="desktop-composer-inp" value="${this.escapeHtml(
                this.draftInput
              )}" placeholder="${this.tr("Send command or prompt to active Herdr / Agent PTY... (Enter to send)", "向当前 Herdr / Agent 终端发送指令或 Prompt... (Enter 发送)")}" autocomplete="off" />
              <button type="button" data-k="ctrl+c" class="pm-btn pm-btn-ghost pm-btn-sm" style="color:var(--coral);">^C</button>
              <button type="button" id="btn-toggle-touch-pad" class="pm-btn pm-btn-ghost pm-btn-sm" title="${this.tr("Toggle virtual keypad", "切换触控虚拟键盘")}">${svgIcon("keyboard", 14)}</button>
              <button type="submit" class="pm-btn pm-btn-primary pm-btn-sm">${this.tr("Send", "发送")}</button>
            </div>
          </form>`
          }
        </main>

        <!-- RIGHT VERTICAL THUMB RAIL -->
        <aside class="right-rail">
          <div style="width:34px;height:58px;border-radius:999px;border:1px solid var(--rose-bd);background:var(--bg-card);display:flex;align-items:flex-end;justify-content:center;padding-bottom:8px;">
            <span style="width:10px;height:10px;border-radius:99px;background:var(--emerald);box-shadow:0 0 8px var(--emerald);"></span>
          </div>

          <div style="text-align:center;" class="coda-mono">
            <div style="font-size:12px;font-weight:700;">${nowTime}</div>
            <div style="margin-top:6px;width:42px;height:42px;border-radius:999px;border:2px solid var(--border-strong);border-top-color:var(--text-primary);display:flex;flex-direction:column;align-items:center;justify-content:center;font-size:10.5px;">
              <strong>68</strong>
              ${
                this.activeTransportMode === "lan_ws"
                  ? `<span style="color:var(--emerald);">LAN</span>`
                  : `<span style="color:var(--text-tertiary);">WSS</span>`
              }
            </div>
          </div>

          <div style="display:flex;flex-direction:column;gap:10px;align-items:center;">
            <button id="rail-sidebar-btn" class="circle-btn" title="${this.tr("Toggle left branch sidebar", "展开/收起左侧分支栏")}">${
              this.showSidebar ? svgIcon("chevronLeft", 14) : svgIcon("chevronRight", 14)
            }</button>
            <div class="rail-capsule">
              <button id="rail-spawn-btn" title="${this.tr("Spawn new Herdr terminal", "新建 Herdr 终端")}">${svgIcon("plus", 15)}</button>
              <span style="width:16px;height:1px;background:var(--border-primary);"></span>
              <button id="rail-more-btn" title="${this.tr("System settings &amp; pairing", "系统设置与配对")}">${svgIcon("more", 15)}</button>
            </div>
          </div>

          <div class="rail-capsule">
            <input type="file" id="rail-file-input" style="display:none;" />
            <button id="rail-refresh-btn" title="${this.tr("Upload file/image via hc put", "通过 hc put 上传图片/附件至远端终端")}">${svgIcon("paperclip", 14)}</button>
            <span style="width:16px;height:1px;background:var(--border-primary);"></span>
            <button id="rail-paste-btn" title="${this.tr("Paste clipboard", "粘贴剪贴板")}">${svgIcon("clipboard", 14)}</button>
          </div>
        </aside>
      </div>

      ${
        this.pendingActivationCode
          ? `
        <div class="modal-mask">
          <div class="modal-card">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
              <strong style="font-size:15px;display:inline-flex;align-items:center;gap:8px;">${svgIcon("key", 15)} ${this.tr(`Authorize Device for @${this.escapeHtml(this.user.handle)}`, `确认授权设备接入 @${this.escapeHtml(this.user.handle)}`)}</strong>
              <button id="btn-dismiss-activate" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
            </div>
            ${this.errorMsg ? `<div style="color:var(--rose);font-size:12px;margin-bottom:8px;">${this.escapeHtml(this.errorMsg)}</div>` : ""}
            <div class="coda-mono" style="font-size:20px;font-weight:700;color:var(--signal-blue);letter-spacing:2px;margin-bottom:10px;">
              ${this.escapeHtml(this.pendingActivationCode)}
            </div>
            ${
              this.pendingDeviceInfo
                ? `<div style="background:var(--bg-elevated);border:1px solid var(--border-primary);border-radius:10px;padding:12px;font-size:12px;line-height:1.7;margin-bottom:14px;">
                    ${this.tr("Device", "设备名称")}：<b>${this.escapeHtml(this.pendingDeviceInfo.device_name)}</b><br/>
                    ${this.tr("Platform / Role", "平台 / 角色")}：${this.escapeHtml(this.pendingDeviceInfo.platform)} / ${this.escapeHtml(this.pendingDeviceInfo.role)}<br/>
                    ${this.tr("Status", "当前状态")}：${this.escapeHtml(this.pendingDeviceInfo.status)}
                  </div>`
                : `<div style="font-size:12px;color:var(--text-secondary);margin-bottom:12px;">${this.tr("Click below to bind this device code to your account.", "点击下方确认，将把该授权码对应的设备绑定到当前账号。")}</div>`
            }
            <button id="btn-approve-device" class="pm-btn pm-btn-primary pm-btn-lg" style="width:100%;">
              ${svgIcon("check", 14)} ${this.tr("Approve Device into Private Mesh", "批准此设备接入我的私有网络")}
            </button>
          </div>
        </div>
      `
          : ""
      }
      ${
        this.showSettingsModal
          ? `
        <div class="modal-mask" id="modal-bg">
          <div class="modal-card">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
              <strong style="font-size:15px;">${this.tr("Devices, Pairing &amp; Voice Model", "接入新设备、授权码与语音模型配置")}</strong>
              <button id="btn-close-modal" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
            </div>
            <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;padding-bottom:12px;margin-bottom:12px;border-bottom:1px solid var(--border-primary);">
              <span style="font-size:12px;color:var(--text-secondary);">${this.tr("Interface Language &amp; Theme", "界面语言与外观主题")}</span>
              <div style="display:flex;align-items:center;gap:8px;">
                ${this.renderLangSwitcher()}
                ${this.renderThemeSwitcher()}
              </div>
            </div>
            ${
              this.activationSuccessMsg
                ? `<div style="background:var(--emerald-bg);border:1px solid var(--emerald-bd);color:var(--emerald);padding:9px 11px;border-radius:8px;font-size:12px;margin-bottom:10px;">${this.escapeHtml(this.activationSuccessMsg)}</div>`
                : ""
            }
            <div class="coda-mono" style="background:var(--bg-elevated);border:1px solid var(--border-primary);padding:10px;border-radius:8px;font-size:11px;margin-bottom:10px;word-break:break-all;">
              ${this.tr("Run on any Linux / macOS host and open the activation link:", "在任意 Linux / Mac 终端执行后打开返回的激活链接：")}<br/>
              <span style="color:var(--emerald);">${this.escapeHtml(installCmd)}</span>
            </div>
            <form id="quick-approve-form" style="display:flex;gap:8px;margin-bottom:12px;">
              <input name="user_code" class="pm-input" style="margin:0;flex:1;" placeholder="${this.tr("Enter device code, e.g. ABCD-2345", "输入终端设备码 如 ABCD-2345")}" />
              <button type="submit" class="pm-btn pm-btn-secondary">${this.tr("Approve", "授权接入")}</button>
            </form>
            <div style="display:flex;gap:8px;flex-wrap:wrap;margin-bottom:12px;">
              <button id="btn-gen-pair" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("+ 6-Digit Pair Token", "+ 生成 6 位配对码")}</button>
              ${
                this.user.is_admin
                  ? `<button id="btn-gen-invite" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("+ Issue Invite Code", "+ 签发邀请码")}</button>`
                  : ""
              }
              <button id="btn-go-home" type="button" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("Home", "官网首页")}</button>
              <button id="btn-logout" class="pm-btn pm-btn-secondary pm-btn-sm" style="color:var(--rose);">${this.tr("Sign Out", "退出登录")}</button>
            </div>
            ${
              this.pairCommand
                ? `<div class="coda-mono" style="background:var(--bg-elevated);border:1px dashed var(--emerald);padding:10px;border-radius:8px;font-size:12px;margin-bottom:10px;word-break:break-all;">
                    ${this.tr("One-line install &amp; pair on macOS / Linux:", "在 Mac / Linux 一键安装并配对：")}<br/>${this.pairCommand}
                  </div>`
                : ""
            }
            ${
              this.latestInviteCode
                ? `<div class="coda-mono" style="background:var(--bg-elevated);border:1px dashed var(--signal-blue);padding:10px;border-radius:8px;font-size:12px;margin-bottom:10px;">
                    ${this.tr("Invite code:", "好友注册邀请码：")} <b>${this.latestInviteCode}</b>
                  </div>`
                : ""
            }
            <form id="voice-cfg-form" style="border-top:1px solid var(--border-primary);padding-top:12px;margin-top:8px;">
              <div style="font-size:13px;font-weight:600;margin-bottom:8px;display:inline-flex;align-items:center;gap:6px;">${svgIcon("mic", 14)} ${this.tr("Voice Model Config (Herdr doc.set)", "语音模型配置 (Herdr doc.set)")}</div>
              <label style="font-size:11px;color:var(--text-secondary);">Provider</label>
              <input name="provider" class="pm-input" value="${this.escapeHtml(this.voiceConfig.provider)}" placeholder="openai / groq / deepseek" />
              <label style="font-size:11px;color:var(--text-secondary);">Base URL</label>
              <input name="base_url" class="pm-input" value="${this.escapeHtml(this.voiceConfig.base_url)}" placeholder="https://api.openai.com/v1" />
              <label style="font-size:11px;color:var(--text-secondary);">API Key</label>
              <input type="password" name="api_key" class="pm-input" value="${this.escapeHtml(this.voiceConfig.api_key)}" placeholder="sk-..." />
              <label style="font-size:11px;color:var(--text-secondary);">Model</label>
              <input name="model" class="pm-input" value="${this.escapeHtml(this.voiceConfig.model)}" placeholder="gpt-4o-mini-transcribe" />
              <button type="submit" class="pm-btn pm-btn-primary" style="width:100%;">
                ${this.tr("Save Voice Config", "保存语音模型配置")}
              </button>
            </form>
          </div>
        </div>
      `
          : ""
      }
    `;

    this.bindCommonNavigation(root);

    document.getElementById("btn-collapse-sidebar")?.addEventListener("click", () => {
      this.showSidebar = false;
      this.render();
    });
    document.getElementById("rail-sidebar-btn")?.addEventListener("click", () => {
      this.showSidebar = !this.showSidebar;
      this.render();
    });
    document.getElementById("btn-open-settings")?.addEventListener("click", () => {
      this.showSettingsModal = true;
      this.render();
    });
    document.getElementById("rail-more-btn")?.addEventListener("click", () => {
      this.showSettingsModal = true;
      this.render();
    });
    document.getElementById("btn-dismiss-activate")?.addEventListener("click", () => {
      this.pendingActivationCode = "";
      this.pendingDeviceInfo = null;
      this.errorMsg = "";
      window.history.replaceState({}, "", "/console");
      this.render();
    });
    document.getElementById("btn-approve-device")?.addEventListener("click", async () => {
      try {
        const dev = await this.api("/api/v1/auth/device/approve", "POST", {
          user_code: this.pendingActivationCode,
        });
        this.pendingActivationCode = "";
        this.pendingDeviceInfo = null;
        this.errorMsg = "";
        this.activationSuccessMsg = this.tr(
          `Approved device ${dev.name} (${dev.platform})`,
          `已授权设备 ${dev.name} (${dev.platform})`
        );
        window.history.replaceState({}, "", "/console");
        await this.refreshDevices();
        this.selectedDaemon = this.devices.find((d) => d.id === dev.id) || this.selectedDaemon;
        if (this.selectedDaemon?.is_online) {
          this.queryDaemonOverview(this.selectedDaemon);
          void this.tryNegotiateDirectLanWs(this.selectedDaemon);
        }
        this.render();
      } catch (err: any) {
        this.errorMsg = err.message;
        this.render();
      }
    });
    document.getElementById("quick-approve-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      const code = String(fd.user_code || "").trim();
      if (!code) return;
      try {
        const dev = await this.api("/api/v1/auth/device/approve", "POST", { user_code: code });
        this.activationSuccessMsg = this.tr(
          `Approved device ${dev.name} (${dev.platform})`,
          `已授权设备 ${dev.name} (${dev.platform})`
        );
        await this.refreshDevices();
        this.render();
      } catch (err: any) {
        this.activationSuccessMsg = err.message;
        this.render();
      }
    });
    document.getElementById("btn-close-modal")?.addEventListener("click", () => {
      this.showSettingsModal = false;
      this.render();
    });
    document.getElementById("btn-gen-pair")?.addEventListener("click", async () => {
      const res = await this.api("/api/v1/devices/pair-token", "POST", {});
      this.pairCommand = res.one_liner_command;
      this.render();
    });
    document.getElementById("btn-gen-invite")?.addEventListener("click", async () => {
      const res = await this.api("/api/v1/invites", "POST", { note: "Friend", max_uses: 5, valid_days: 30 });
      this.latestInviteCode = res.code;
      this.render();
    });
    document.getElementById("btn-go-home")?.addEventListener("click", () => {
      this.showSettingsModal = false;
      this.navigate("/");
    });
    document.getElementById("btn-logout")?.addEventListener("click", () => {
      localStorage.removeItem("coda_token");
      this.token = null;
      this.user = null;
      this.showSettingsModal = false;
      this.directWs?.close();
      this.ws?.close();
      this.navigate("/login");
    });

    document.getElementById("btn-kb-toggle")?.addEventListener("click", () => {
      this.showKeyboardInputBar = !this.showKeyboardInputBar;
      this.render();
    });

    document.getElementById("kb-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      const inp = document.getElementById("kb-input") as HTMLInputElement;
      this.draftInput = inp.value;
      this.showKeyboardInputBar = false;
      this.render();
    });

    document.getElementById("desktop-prompt-input")?.addEventListener("input", (e) => {
      this.draftInput = (e.target as HTMLInputElement).value;
    });

    document.getElementById("btn-toggle-touch-pad")?.addEventListener("click", () => {
      this.showMobileKeypadOnDesktop = !this.showMobileKeypadOnDesktop;
      this.render();
    });

    document.getElementById("desktop-prompt-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      const inp = document.getElementById("desktop-prompt-input") as HTMLInputElement | null;
      const text = (inp ? inp.value : this.draftInput).trim();
      if (this.selectedDaemon && this.activeTerminalAddr && text) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "send_terminal_text",
          req_id: `tx-${Date.now()}`,
          terminal: this.activeTerminalAddr,
          text,
        });
      }
      this.draftInput = "";
      this.render();
    });

    document.getElementById("rail-spawn-btn")?.addEventListener("click", () => {
      if (this.selectedDaemon?.is_online) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "launch_agent",
          req_id: `run-${Date.now()}`,
          agent: "shell",
          name: null,
          cwd: null,
          initial_prompt: null,
          cols: 108,
          rows: 32,
        });
      }
    });

    document.getElementById("rail-refresh-btn")?.addEventListener("click", () => {
      const fileInp = document.getElementById("rail-file-input") as HTMLInputElement | null;
      if (fileInp && this.selectedDaemon && this.activeTerminalAddr) {
        fileInp.click();
      } else if (this.activeTerminalAddr) {
        this.requestScreen(this.activeTerminalAddr);
      }
    });

    document.getElementById("rail-file-input")?.addEventListener("change", (ev) => {
      const inp = ev.target as HTMLInputElement;
      const file = inp.files?.[0];
      if (!file || !this.selectedDaemon || !this.activeTerminalAddr) return;
      const reader = new FileReader();
      reader.onload = () => {
        const res = String(reader.result || "");
        const b64 = res.includes(",") ? res.split(",")[1] : res;
        this.sendEncrypted(this.selectedDaemon!, {
          op: "upload_terminal_file",
          req_id: `up-${Date.now()}`,
          terminal: this.activeTerminalAddr!,
          filename: file.name,
          data_b64: b64,
        });
      };
      reader.readAsDataURL(file);
    });

    document.getElementById("voice-cfg-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      this.voiceConfig = {
        provider: fd.provider || "openai",
        base_url: fd.base_url || "https://api.openai.com/v1",
        api_key: fd.api_key || "",
        model: fd.model || "gpt-4o-mini-transcribe",
      };
      if (this.selectedDaemon?.is_online) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "set_voice_config",
          req_id: `v-${Date.now()}`,
          config: this.voiceConfig,
        });
      }
      this.showSettingsModal = false;
      this.render();
    });

    document.querySelectorAll("[data-dev]").forEach((el) => {
      el.addEventListener("click", () => {
        const devId = (el as HTMLElement).dataset.dev!;
        const peer = this.devices.find((d) => d.id === devId);
        if (!peer) return;
        this.selectedDaemon = peer;
        this.terminals = [];
        this.activeTerminalAddr = null;
        if (peer.is_online) {
          this.queryDaemonOverview(peer);
          void this.tryNegotiateDirectLanWs(peer);
        }
        this.render();
      });
    });

    document.querySelectorAll("[data-term]").forEach((el) => {
      el.addEventListener("click", () => {
        this.activeTerminalAddr = (el as HTMLElement).dataset.term!;
        this.attachAndReadTerminal(this.activeTerminalAddr);
        this.render();
      });
    });

    document.querySelectorAll("[data-k]").forEach((el) => {
      el.addEventListener("click", () => {
        const k = (el as HTMLElement).dataset.k!;
        if (k === "slash") {
          this.draftInput = "/clear";
          this.render();
          return;
        }
        if (k === "backspace") {
          this.draftInput = this.draftInput.slice(0, -1);
          this.render();
          return;
        }
        if (k === "enter") {
          if (this.selectedDaemon && this.activeTerminalAddr && this.draftInput.trim()) {
            this.sendEncrypted(this.selectedDaemon, {
              op: "send_terminal_text",
              req_id: `tx-${Date.now()}`,
              terminal: this.activeTerminalAddr,
              text: this.draftInput,
            });
          }
          this.draftInput = "";
          this.render();
          return;
        }
        if (this.selectedDaemon && this.activeTerminalAddr) {
          this.sendEncrypted(this.selectedDaemon, {
            op: "send_terminal_keys",
            req_id: `k-${Date.now()}`,
            terminal: this.activeTerminalAddr,
            keys: [k],
          });
        }
      });
    });
  }

  private escapeHtml(str: string): string {
    return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }
}

new CodaTerminalWebApp().init();
