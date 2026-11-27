import {
  BrowserE2EESession,
  RelayNonceReplayCache,
  RelaySessionAuth,
  createRelaySessionAuth,
  fromB64,
  generateEphemeralX25519,
  isTimestampFresh,
  loadOrCreateWebIdentity,
  p2pAckV2SigningText,
  p2pHelloV2SigningTextWithZstd,
  signWebMessage,
  toB64,
  verifyEd25519Signature,
  verifyRelaySessionAuth,
} from "./crypto";
import { WebVtTerminal, escapeHtml, safeHref } from "./vt";
import {
  type AdminUserSummary,
  type AppRoute,
  type AuthSessionInfo,
  type BranchWorktreeItem,
  type Device,
  type DeviceAuthPublicInfo,
  type HerdrTerminal,
  type InviteCodeInfo,
  type Locale,
  type PromptSnippet,
  type RelayNodeInfo,
  type RepoWorkspaceSummary,
  type SiteInfo,
  type ThemePreference,
  type TransportMode,
  type User,
  type VapidPublicKeyInfo,
  type VoiceModelConfig,
  clearSessionToken,
  readLocalPref,
  readSessionOrLegacyToken,
  saveSessionToken,
  writeLocalPref,
} from "./types";
import {
  type IconName,
  codaLogoSVG,
  codaNodeDotSVG,
  svgIcon,
} from "./icons";
import { BentoMotionController } from "./bento_motion";
import {
  encodeTerminalKeyEvent,
  isJumpNeedsYouShortcut,
  isToggleSplitViewShortcut,
  wrapBracketedPaste,
} from "./terminal_keys";
import {
  TerminalNotificationTracker,
  dispatchBrowserNotification,
  getBrowserNotificationPermission,
  isNeedsYouState,
  requestBrowserNotificationPermission,
} from "./notify";
import { SmartPathBackoffPolicy, collectDirectWsCandidates } from "./smart_path";
import {
  type HostHealthSnapshot,
  MOBILE_QUICK_CHIPS,
  buildWebSessionMarkdown,
  findWebTerminalMatches,
  firstSafeTerminalUrlInLine,
  formatWebHostHealth,
  triggerMarkdownDownload,
} from "./terminal_extras";
import {
  type RemoteFileBrowserState,
  type RemoteFileEntry,
  createInitialFileBrowserState,
  joinRootAndRel,
  parentSubPath,
  renderRemoteFileBrowserModal,
} from "./file_browser";
import {
  type PortPreviewState,
  createInitialPortPreviewState,
  decodePortPreviewPayload,
  parseWebPreviewPortAndPath,
  renderPortPreviewModal,
} from "./port_preview";
import {
  type ApprovalCapsuleAction,
  type GitDiffModalState,
  type NotificationWebhookSettings,
  buildClipboardImageFilename,
  canStepUpSpawnDirectory,
  collectNeedsAttentionItems,
  collectPrunableVtKeys,
  computeBootstrapAutoFolder,
  createDefaultNotificationSettings,
  createInitialGitDiffState,
  encodeApprovalActionBytes,
  encodeComposerSubmit,
  extractPureClipboardImages,
  formatUploadedPathForPty,
  isKnownTuiAgentTerminal,
  parentAbsolutePath,
  parseGitStatusShort,
  pickNextNeedsAttentionItem,
  rebuildSpeechRecognitionTranscript,
  renderGitDiffModal,
  renderNeedsAttentionSidebarSection,
  renderNeedsYouCapsuleBar,
  renderPromptComposerPanel,
  resolveEffectiveBracketedPaste,
  resolveGitDiffFileRoot,
  resolveSpawnPickerInitialRoot,
  resolveVoiceCaptureStrategy,
  stepPaletteSelectionIndex,
  validateGitCloneUrlClient,
  validateNotificationWebhookSettings,
  validateSingleFolderName,
} from "./git_diff_composer";
import {
  DEFAULT_PROMPT_SNIPPETS,
  MAX_PROMPT_SNIPPETS,
  SNIPPETS_CACHE_STORAGE_KEY,
  buildInviteRegistrationUrl,
  clearAccountScopedSignOutState,
  deriveConsoleConnectionVisualState,
  deriveSnippetTitleFromPrompt,
  detectBrowserIosNonStandalone,
  extractImagesFromClipboardItems,
  extractPushSubscriptionKeys,
  formatConsoleHeaderSubtitle,
  insertSnippetIntoDraft,
  isPushSubscriptionVapidKeyMismatch,
  normalizePromptSnippetsList,
  onboardingDismissedStorageKey,
  parseLocationRouteAndParams,
  renderAdminFriendsDirectorySection,
  renderConsoleEmptyStateHtml,
  renderFriendOnboardingCard,
  renderWebPushSettingsCard,
  teardownWebPushOnSignOut,
  validateNewPasswordLength,
  validatePromptSnippet,
  validateResetPasswordInputs,
  vapidBase64UrlToUint8Array,
} from "./friend_ux";

if (typeof window !== "undefined" && typeof document !== "undefined") {
  try {
    const pref = localStorage.getItem("coda_theme") || "dark";
    const resolved =
      pref === "system"
        ? window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches
          ? "light"
          : "dark"
        : pref === "light"
          ? "light"
          : "dark";
    document.documentElement.dataset.theme = resolved;
    document.documentElement.style.colorScheme = resolved;
    const qLang = new URLSearchParams(window.location.search).get("lang");
    const loc =
      qLang === "zh" || qLang === "zh-CN"
        ? "zh"
        : qLang === "en"
          ? "en"
          : localStorage.getItem("coda_locale") === "zh"
            ? "zh"
            : "en";
    document.documentElement.lang = loc === "zh" ? "zh-CN" : "en";
  } catch {
    document.documentElement.dataset.theme = "dark";
    document.documentElement.lang = "en";
  }
}

// V3-21: Browser sends `compressed: false` so raw bytes expand by (4/3)^2 (~1.78x) via
// upload base64 + E2EE ciphertext base64; 4 MiB keeps wire frames safely under MAX_WIRE_FRAME_BYTES (8 MiB).
const MAX_UPLOAD_BYTES = 4 * 1024 * 1024;

export class CodaTerminalWebApp {
  private token: string | null = readSessionOrLegacyToken();
  private themePref: ThemePreference = readLocalPref("coda_theme", "dark") as ThemePreference;
  private locale: Locale = this.resolveInitialLocale();
  private route: AppRoute = "home";
  private authMode: "login" | "register" = "login";
  private heroNode: "mac" | "linux" | "iphone" = "mac";
  private previewTab: "coda" | "server" | "ios" = "coda";
  private showPassword = false;
  private showMobileKeypadOnDesktop = false;
  private showCmdPalette = false;
  private cmdPaletteQuery = "";
  private cmdPaletteSelectedIdx = 0;
  private siteInfo: SiteInfo | null = null;
  private pendingActivationCode: string = "";
  private pendingDeviceInfo: DeviceAuthPublicInfo | null = null;
  private activationSuccessMsg = "";
  private settingsFeedback: { kind: "ok" | "err"; text: string } | null = null;
  private consoleNotice: { kind: "ok" | "err"; text: string } | null = null;
  private copiedToast: "" | "ok" | "manual" = "";
  private user: User | null = null;
  private allDevices: Device[] = [];
  private devices: Device[] = [];
  private authSessions: AuthSessionInfo[] = [];
  private invitesList: InviteCodeInfo[] = [];
  private myClientDevice: Device | null = null;
  private selectedDaemon: Device | null = null;
  private terminals: HerdrTerminal[] = [];
  private terminalsByPeer = new Map<string, HerdrTerminal[]>();
  private repos: RepoWorkspaceSummary[] = [];
  private voiceConfig: VoiceModelConfig = {
    provider: "openai",
    base_url: "https://api.openai.com/v1",
    api_key: "",
    model: "gpt-4o-mini-transcribe",
  };
  private voiceApiKeyLocal = "";
  private activeTerminalAddr: string | null = null;
  private splitMode = false;
  private secondaryTerminalAddr: string | null = null;
  private focusedPane: "primary" | "secondary" = "primary";
  private showTermSearch = false;
  private termSearchQuery = "";
  private termSearchIdx = 0;
  private hostHealthByPeer = new Map<string, HostHealthSnapshot>();
  private fileBrowser: RemoteFileBrowserState = createInitialFileBrowserState();
  private portPreview: PortPreviewState = createInitialPortPreviewState();
  private gitDiff: GitDiffModalState = createInitialGitDiffState();
  private showPromptComposer = false;
  private toastTimer: ReturnType<typeof setTimeout> | null = null;
  private notificationSettings: NotificationWebhookSettings = createDefaultNotificationSettings();
  private draftInput = "";
  private imeComposing = false;
  private showSidebar = typeof window === "undefined" ? true : window.innerWidth >= 900;
  private lastIsNarrowViewport = typeof window !== "undefined" && window.innerWidth < 900;
  private showSettingsModal = false;
  private settingsTab: "devices" | "alerts" | "security" | "voice" = "devices";
  private showSpawnModal = false;
  private spawnAgentType = "shell";
  private spawnCwd = "";
  private spawnInitialPrompt = "";
  private spawnDirPicker: {
    open: boolean;
    rootPath: string;
    subPath: string;
    entries: RemoteFileEntry[];
    loading: boolean;
    error: string | null;
    reqId: string | null;
  } = {
    open: false,
    rootPath: "",
    subPath: "",
    entries: [],
    loading: false,
    error: null,
    reqId: null,
  };
  private spawnBootstrapOpen = false;
  private spawnBootstrapParentDir = "~";
  private spawnBootstrapFolderName = "";
  private spawnBootstrapGitUrl = "";
  private bootFolderManuallyEdited = false;
  private spawnBootstrapLoading = false;
  private spawnBootstrapReqId: string | null = null;
  private spawnBootstrapPeerId: string | null = null;
  private spawnBootstrapMsg: { kind: "ok" | "err"; text: string } | null = null;
  private showKeyboardInputBar = false;
  private isRecordingVoice = false;
  private isTranscribingVoice = false;
  private pendingTranscribeReqId: string | null = null;
  private pendingTranscribeTimer: ReturnType<typeof setTimeout> | null = null;
  private mediaRecorder: MediaRecorder | null = null;
  private mediaStream: MediaStream | null = null;
  private voiceChunks: Blob[] = [];
  private speechRecognition: any = null;
  private rpcSeq = 0;
  private nextFileInputTarget: "pty" | "draft" = "pty";
  private pendingUploadTargets = new Map<
    string,
    { target: "pty" | "draft"; daemonId: string; terminalAddr: string }
  >();
  private settingsDirtyFields = new Set<string>();
  private webhookTestInFlight = false;
  private webhookSaveInFlight = false;
  private pruneDevicesInFlight = false;
  private pairCommand: string | null = null;
  private latestInviteCode: string | null = null;
  private rttMs: number | null = null;
  private termCols = 108;
  private termRows = 32;
  private ws: WebSocket | null = null;
  private eventsWs: WebSocket | null = null;
  private directWs: WebSocket | null = null;
  private directWsPeerId: string | null = null;
  private activeTransportMode: TransportMode = "relay_ws";
  private negotiatingPeerId: string | null = null;
  private directProbeGeneration = 0;
  private relayGeneration = 0;
  private uploadCounter = 0;
  private relayKeepAliveTimer: ReturnType<typeof setInterval> | null = null;
  private relayReconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private eventsReconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private directProbeTimer: ReturnType<typeof setTimeout> | null = null;
  private relayBackoffMs = 1000;
  private eventsBackoffMs = 1000;
  private smartPath = new SmartPathBackoffPolicy();
  private notificationsEnabled: boolean = readLocalPref("coda_notify_enabled", "0") === "1";
  private notifyTracker = new TerminalNotificationTracker(20_000);
  private termPaintRafId: number | null = null;
  private pendingSelectionCatchup = false;
  private lastRenderedTermKey = "";
  private termResizeObserver: ResizeObserver | null = null;
  private resizeDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  private e2eeSessions = new Map<string, { session: BrowserE2EESession; auth: RelaySessionAuth; confirmed: boolean }>();
  private seenRelayNonces = new RelayNonceReplayCache();
  private directE2eeSessions = new Map<string, BrowserE2EESession>();
  private vtTerminals = new Map<string, WebVtTerminal>();
  private vtStreamActive = new Set<string>();
  private lastWorkspacesQueryAt = 0;
  private webIdentity = loadOrCreateWebIdentity();
  private errorMsg = "";
  private authInviteCode = "";
  private authPrefillHandle = "";
  private authSuccessMsg = "";
  private resetPasswordToken = "";
  private resetPasswordSubmitting = false;
  private showResetPassword = false;
  private swRegistration: ServiceWorkerRegistration | null = null;
  private vapidInfo: VapidPublicKeyInfo = { enabled: false, public_key: "" };
  private vapidLoaded = false;
  private webPushCount = 0;
  private webPushThisDeviceSubscribed = false;
  private webPushEndpoint = "";
  private webPushBusy = false;
  private adminUsers: AdminUserSummary[] = [];
  private adminResetBusyUserId: string | null = null;
  private latestAdminResetLink: { handle: string; resetUrl: string } | null = null;
  private onboardingDismissed = false;
  private onboardingCopiedInstall = false;
  private promptSnippets: PromptSnippet[] = this.loadInitialCachedSnippets();
  private savingSnippetOpen = false;
  private savingSnippetTitle = "";
  private savingSnippetBusy = false;

  // Bento motion scheduler state (never triggers full DOM re-render per frame)
  private bento = new BentoMotionController();

  private resolveRouteFromLocation(): AppRoute {
    return parseLocationRouteAndParams(window.location.pathname, window.location.search).route;
  }

  private syncLocationRouteAndParams(pathname: string, search: string): {
    route: AppRoute;
    inviteCode: string;
    resetToken: string;
    activationCode: string;
  } {
    const parsed = parseLocationRouteAndParams(pathname, search);
    this.route = parsed.route;
    if (parsed.inviteCode) {
      this.authInviteCode = parsed.inviteCode;
      this.authMode = "register";
      const cleanPath = (pathname || "/").replace(/\/+$/, "") || "/";
      if (cleanPath === "/" && !this.user) {
        window.history.replaceState({}, "", `/register${search}`);
      }
    }
    if (parsed.resetToken) {
      this.resetPasswordToken = parsed.resetToken;
    }
    if (this.route === "login") this.authMode = "login";
    if (this.route === "register") this.authMode = "register";
    return parsed;
  }

  private navigate(target: string, replace = false, preserveAuthSuccess = false) {
    const url = new URL(target, window.location.origin);
    const nextUrl = `${url.pathname}${url.search}${url.hash}`;
    if (replace) {
      window.history.replaceState({}, "", nextUrl);
    } else {
      window.history.pushState({}, "", nextUrl);
    }

    const parsed = this.syncLocationRouteAndParams(url.pathname, url.search);
    this.errorMsg = "";
    if (!preserveAuthSuccess) {
      this.authSuccessMsg = "";
    }
    this.showCmdPalette = false;

    const codeParam = parsed.activationCode;
    if (codeParam && codeParam !== this.pendingActivationCode) {
      this.pendingActivationCode = codeParam;
      void this.lookupActivationCode(codeParam).then(() => this.render());
    }

    if (url.hash && this.route === "home") {
      this.render();
      requestAnimationFrame(() => {
        const targetId = decodeURIComponent(url.hash.slice(1));
        if (targetId) {
          document.getElementById(targetId)?.scrollIntoView({ behavior: "smooth" });
        }
      });
      return;
    }

    window.scrollTo(0, 0);
    this.render();
  }

  private resolveInitialLocale(): Locale {
    try {
      if (typeof window === "undefined") {
        return readLocalPref("coda_locale", "en") === "zh" ? "zh" : "en";
      }
      const qLang = new URLSearchParams(window.location.search).get("lang");
      if (qLang === "zh" || qLang === "zh-CN") {
        writeLocalPref("coda_locale", "zh");
        return "zh";
      }
      if (qLang === "en") {
        writeLocalPref("coda_locale", "en");
        return "en";
      }
      return readLocalPref("coda_locale", "en") === "zh" ? "zh" : "en";
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
    document.querySelectorAll<HTMLInputElement>("#reset-pw-form input[name]").forEach((inp) => {
      if (inp.name) draftValues[`reset:${inp.name}`] = inp.value;
    });
    const activateInp = document.querySelector<HTMLInputElement>('#activate-lookup-form input[name="code"]');
    if (activateInp) {
      draftValues["activate:code"] = activateInp.value;
    }

    this.locale = loc;
    if (persist) {
      writeLocalPref("coda_locale", loc);
    }
    this.syncDocumentLocaleMeta();
    this.render();

    document.querySelectorAll<HTMLInputElement>("#auth-form input[name]").forEach((inp) => {
      const k = `auth:${inp.name}`;
      if (inp.name && k in draftValues) inp.value = draftValues[k];
    });
    document.querySelectorAll<HTMLInputElement>("#reset-pw-form input[name]").forEach((inp) => {
      const k = `reset:${inp.name}`;
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
    this.syncLocationRouteAndParams(window.location.pathname, window.location.search);
    void this.registerServiceWorkerIfSupported();

    window.matchMedia?.("(prefers-color-scheme: light)")?.addEventListener("change", () => {
      if (this.themePref === "system") {
        this.applyTheme("system", false);
      }
    });

    window.addEventListener("popstate", () => {
      const parsed = this.syncLocationRouteAndParams(
        window.location.pathname,
        window.location.search
      );
      this.errorMsg = "";
      this.authSuccessMsg = "";
      this.showCmdPalette = false;
      const codeParam = parsed.activationCode;
      if (codeParam && codeParam !== this.pendingActivationCode) {
        this.pendingActivationCode = codeParam;
        void this.lookupActivationCode(codeParam).then(() => this.render());
      } else {
        this.render();
      }
    });

    window.addEventListener("keydown", (e) => {
      const isJumpNeedsYou = isJumpNeedsYouShortcut(e);
      // V3-24: Only intercept ⌘K / Ctrl+K on home & console (not login/register/activate forms),
      // and never double-handle events already consumed by `#term-box` (e.g. Ctrl+K kill-line).
      if (
        !e.defaultPrevented &&
        (e.metaKey || e.ctrlKey) &&
        !e.altKey &&
        !e.shiftKey &&
        e.key.toLowerCase() === "k" &&
        (this.route === "home" || this.route === "console")
      ) {
        e.preventDefault();
        this.showCmdPalette = !this.showCmdPalette;
        if (this.showCmdPalette) {
          this.cmdPaletteQuery = "";
          this.cmdPaletteSelectedIdx = 0;
        }
        this.render();
        if (this.showCmdPalette) {
          setTimeout(() => {
            const inp = document.getElementById("console-cmd-input") as HTMLInputElement | null;
            inp?.focus();
          }, 0);
        }
      } else if (
        this.route === "home" &&
        this.showCmdPalette &&
        (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "Enter")
      ) {
        const homeRows = Array.from(
          document.querySelectorAll<HTMLElement>("#cmd-mask .cmd-list .cmd-row")
        );
        if (homeRows.length > 0) {
          e.preventDefault();
          if (e.key === "Enter") {
            const target = homeRows[Math.min(this.cmdPaletteSelectedIdx, homeRows.length - 1)];
            target?.click();
          } else {
            this.cmdPaletteSelectedIdx = stepPaletteSelectionIndex(
              this.cmdPaletteSelectedIdx,
              homeRows.length,
              e.key === "ArrowDown" ? 1 : -1
            );
            homeRows.forEach((row, idx) => {
              row.classList.toggle("active", idx === this.cmdPaletteSelectedIdx);
            });
            homeRows[this.cmdPaletteSelectedIdx]?.scrollIntoView({ block: "nearest" });
          }
        }
      } else if (
        !e.defaultPrevented &&
        isJumpNeedsYou &&
        this.route === "console" &&
        !this.isAnyModalOpen()
      ) {
        e.preventDefault();
        this.jumpToNextNeedsYouTerminal();
      } else if (
        !e.defaultPrevented &&
        isToggleSplitViewShortcut(e) &&
        this.route === "console" &&
        !this.isAnyModalOpen()
      ) {
        e.preventDefault();
        this.toggleSplitView();
      } else if (
        !e.defaultPrevented &&
        (e.metaKey || e.ctrlKey) &&
        !e.altKey &&
        !e.shiftKey &&
        e.key.toLowerCase() === "f" &&
        this.route === "console" &&
        !this.isAnyModalOpen()
      ) {
        e.preventDefault();
        this.showTermSearch = true;
        this.render();
        setTimeout(() => {
          const inp = document.getElementById("term-search-input") as HTMLInputElement | null;
          inp?.focus();
          inp?.select();
        }, 0);
      } else if (e.key === "Escape" && this.showCmdPalette) {
        e.preventDefault();
        this.showCmdPalette = false;
        this.render();
      } else if (e.key === "Escape" && this.route === "console" && this.pendingActivationCode) {
        e.preventDefault();
        this.pendingActivationCode = "";
        this.pendingDeviceInfo = null;
        this.errorMsg = "";
        window.history.replaceState({}, "", "/console");
        this.render();
      } else if (e.key === "Escape" && this.gitDiff.open) {
        e.preventDefault();
        this.gitDiff.open = false;
        this.render();
      } else if (e.key === "Escape" && this.fileBrowser.open) {
        e.preventDefault();
        this.fileBrowser.open = false;
        this.render();
      } else if (e.key === "Escape" && this.portPreview.open) {
        e.preventDefault();
        this.portPreview.open = false;
        this.render();
      } else if (e.key === "Escape" && this.showSpawnModal) {
        e.preventDefault();
        this.showSpawnModal = false;
        this.render();
      } else if (e.key === "Escape" && this.showSettingsModal) {
        e.preventDefault();
        this.showSettingsModal = false;
        this.render();
      } else if (e.key === "Escape" && this.route === "console" && this.showTermSearch) {
        e.preventDefault();
        this.showTermSearch = false;
        this.render();
      } else if (e.key === "Escape" && this.route === "console" && this.showPromptComposer) {
        e.preventDefault();
        this.showPromptComposer = false;
        this.render();
      }
    });

    // Global clipboard screenshot paste (`Ctrl+V` / `⌘V`) when an active terminal session is open.
    // Never hijack paste inside `<input>`/`<textarea>`/`[contenteditable]`, `#term-search-input`, or open modals.
    window.addEventListener("paste", (e) => {
      if (e.defaultPrevented) return;
      if (this.route !== "console" || !this.selectedDaemon || !this.activeTerminalAddr) return;
      if (this.isAnyModalOpen() || this.isEditablePasteTarget(e.target)) {
        return;
      }
      this.handleClipboardImagePaste(e, "pty", this.getFocusedTerminalAddr());
    });

    // V3-23: Schedule catch-up paint as soon as user clears/collapses a text selection that deferred row patches.
    document.addEventListener("selectionchange", () => {
      if (!this.pendingSelectionCatchup || this.route !== "console") return;
      const sel = window.getSelection();
      const boxEl = document.getElementById("term-box");
      const screenEl = document.getElementById("term-screen-output");
      const stillSelectingInside = Boolean(
        sel &&
          !sel.isCollapsed &&
          sel.rangeCount > 0 &&
          (boxEl?.contains(sel.anchorNode) ||
            boxEl?.contains(sel.focusNode) ||
            (screenEl && typeof sel.containsNode === "function" && sel.containsNode(screenEl, true)))
      );
      if (!stillSelectingInside) {
        this.pendingSelectionCatchup = false;
        this.scheduleTerminalPaint();
      }
    });

    const params = new URLSearchParams(window.location.search);
    const codeParam = params.get("code") || "";
    if (this.route === "activate" || codeParam) {
      this.pendingActivationCode = codeParam.trim().toUpperCase();
    }

    const triggerNetworkHotSwitch = () => {
      this.smartPath.resetForNetworkChange();
      if (this.token) {
        if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
          void this.connectRelayWebSocket();
        }
        if (!this.eventsWs || this.eventsWs.readyState !== WebSocket.OPEN) {
          this.connectAccountEventsWebSocket();
        }
      }
      if (this.selectedDaemon?.is_online && this.activeTransportMode !== "lan_ws") {
        this.scheduleDirectLanProbe(1000);
      }
    };

    window.addEventListener("online", triggerNetworkHotSwitch);
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden && this.route === "console" && this.selectedDaemon?.is_online && this.activeTransportMode !== "lan_ws") {
        this.smartPath.resetForNetworkChange(this.selectedDaemon.id);
        this.scheduleDirectLanProbe(1200);
      }
    });
    const navConn = (navigator as unknown as { connection?: EventTarget }).connection;
    navConn?.addEventListener?.("change", triggerNetworkHotSwitch);

    const syncVisualViewport = () => {
      const vv = window.visualViewport;
      if (!vv) return;
      if (vv.height > 0) {
        document.documentElement.style.setProperty("--coda-vvh", `${Math.round(vv.height)}px`);
      }
      const shellRoot = document.querySelector(".shell-root");
      if (shellRoot) {
        const isSmallScreen = window.innerWidth <= 768;
        const softKbOpen = isSmallScreen && window.innerHeight - vv.height > 120;
        shellRoot.classList.toggle("is-softkb-open", softKbOpen);
      }
    };
    window.visualViewport?.addEventListener("resize", syncVisualViewport);
    window.visualViewport?.addEventListener("scroll", syncVisualViewport);
    syncVisualViewport();

    window.addEventListener("resize", () => {
      syncVisualViewport();
      if (this.route === "console") {
        const nowNarrow = window.innerWidth < 900;
        if (nowNarrow !== this.lastIsNarrowViewport) {
          this.lastIsNarrowViewport = nowNarrow;
          if (nowNarrow) this.showSidebar = false;
          this.render();
          return;
        }
        if (this.selectedDaemon && this.activeTerminalAddr) {
          this.scheduleTerminalResizeSync();
        }
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
        this.syncUserScopedPrefs();
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
        this.user = null;
        clearSessionToken();
        const cleaned = clearAccountScopedSignOutState(
          typeof localStorage !== "undefined" ? localStorage : null
        );
        this.promptSnippets = cleaned.promptSnippets;
        this.onboardingDismissed = cleaned.onboardingDismissed;
      }
    }
    this.render();
  }

  private applyTheme(pref: ThemePreference, persist = true) {
    this.themePref = pref;
    if (persist) {
      writeLocalPref("coda_theme", pref);
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
    this.bento.stop();
  }

  private startBentoLoop() {
    this.bento.start("bento-stage", () => this.route === "home");
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
    const rawText = await resp.text();
    let data: any = {};
    if (rawText) {
      try {
        data = JSON.parse(rawText);
      } catch {
        data = { error: rawText.slice(0, 180) };
      }
    }
    if (!resp.ok) throw new Error(data?.error || `HTTP ${resp.status}`);
    return data;
  }

  private nextReqId(prefix: string): string {
    this.rpcSeq += 1;
    return `${prefix}-${Date.now()}-${this.rpcSeq}`;
  }

  private isAnyModalOpen(): boolean {
    return Boolean(
      this.showSettingsModal ||
        this.showCmdPalette ||
        this.showSpawnModal ||
        this.gitDiff.open ||
        this.fileBrowser.open ||
        this.portPreview.open ||
        (this.route === "console" && this.pendingActivationCode)
    );
  }

  private isEditablePasteTarget(target: EventTarget | null): boolean {
    if (!target || typeof target !== "object") return false;
    const el = target as HTMLElement;
    const tag = String(el.tagName || "").toUpperCase();
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
    if (el.isContentEditable) return true;
    if (typeof el.closest === "function") {
      if (
        el.closest(
          'input, textarea, select, [contenteditable=""], [contenteditable="true"], .modal-mask, #term-search-strip'
        )
      ) {
        return true;
      }
    }
    return false;
  }

  private getFocusedTerminalAddr(): string | null {
    if (this.splitMode && this.focusedPane === "secondary" && this.secondaryTerminalAddr) {
      return this.secondaryTerminalAddr;
    }
    return this.activeTerminalAddr;
  }

  private isTypingIntoInput(): boolean {
    if (this.imeComposing) return true;
    const active = typeof document !== "undefined" ? document.activeElement : null;
    if (!active) return false;
    const tag = active.tagName;
    return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || (active as HTMLElement).isContentEditable;
  }

  private vtKey(daemonId: string, terminalAddr: string): string {
    return `${daemonId}:${terminalAddr}`;
  }

  private syncVtAgentDefaultMode(daemonId: string, term?: HerdrTerminal | null) {
    if (!term) return;
    const vt = this.vtTerminals.get(this.vtKey(daemonId, term.address));
    if (vt && vt.bracketedPasteExplicit === null && isKnownTuiAgentTerminal(term)) {
      vt.bracketedPaste = true;
    }
  }

  private getOrCreateVt(
    daemonId: string,
    terminalAddr: string,
    termHint?: HerdrTerminal | null
  ): WebVtTerminal {
    const key = this.vtKey(daemonId, terminalAddr);
    let vt = this.vtTerminals.get(key);
    const knownTerm =
      termHint ??
      (this.selectedDaemon?.id === daemonId
        ? this.terminals.find((t) => t.address === terminalAddr)
        : this.terminalsByPeer.get(daemonId)?.find((t) => t.address === terminalAddr));
    if (!vt) {
      vt = new WebVtTerminal(this.termCols, this.termRows);
      if (isKnownTuiAgentTerminal(knownTerm)) {
        vt.bracketedPaste = true;
      }
      this.vtTerminals.set(key, vt);
    } else if (vt.bracketedPasteExplicit === null && isKnownTuiAgentTerminal(knownTerm)) {
      vt.bracketedPaste = true;
    }
    return vt;
  }

  private pruneStaleVtTerminalsForPeer(peerId: string, liveTerminals: readonly HerdrTerminal[]) {
    const keepAddrs = new Set<string>();
    for (const t of liveTerminals) {
      if (t.alive) {
        keepAddrs.add(t.address);
      }
    }
    if (this.selectedDaemon?.id === peerId) {
      if (this.activeTerminalAddr && liveTerminals.some((t) => t.address === this.activeTerminalAddr)) {
        keepAddrs.add(this.activeTerminalAddr);
      }
      if (this.secondaryTerminalAddr && liveTerminals.some((t) => t.address === this.secondaryTerminalAddr)) {
        keepAddrs.add(this.secondaryTerminalAddr);
      }
    }
    const staleKeys = collectPrunableVtKeys(this.vtTerminals.keys(), peerId, keepAddrs);
    for (const k of staleKeys) {
      this.vtTerminals.delete(k);
      this.vtStreamActive.delete(k);
    }
  }

  private scheduleTerminalPaint() {
    if (typeof requestAnimationFrame === "undefined") return;
    if (this.termPaintRafId !== null) return;
    this.termPaintRafId = requestAnimationFrame(() => {
      this.termPaintRafId = null;
      this.paintActiveTerminalViewport();
    });
  }

  private isNeedsYouStatus(status: HerdrTerminal["status"] | null | undefined): boolean {
    return isNeedsYouState(status?.state);
  }

  private renderConsoleNoticeHtml(): string {
    if (!this.consoleNotice) return "";
    return `<div style="margin-bottom:10px;padding:8px 10px;border-radius:8px;font-size:12px;border:1px solid ${
      this.consoleNotice.kind === "err" ? "var(--rose-bd, rgba(244,63,94,0.35))" : "var(--emerald-bd)"
    };background:${
      this.consoleNotice.kind === "err" ? "rgba(244,63,94,0.1)" : "var(--emerald-bg)"
    };color:${this.consoleNotice.kind === "err" ? "var(--rose)" : "var(--emerald)"};">${this.escapeHtml(
      this.consoleNotice.text
    )}</div>`;
  }

  private renderFloatingToastHtml(): string {
    if (!this.consoleNotice) return "";
    const isErr = this.consoleNotice.kind === "err";
    return `<div class="console-floating-toast" style="border:1px solid ${
      isErr ? "var(--rose-bd)" : "var(--emerald-bd)"
    };background:${
      isErr ? "var(--bg-card)" : "var(--bg-card)"
    };color:${isErr ? "var(--rose)" : "var(--emerald)"};">${this.escapeHtml(
      this.consoleNotice.text
    )}</div>`;
  }

  private updateConsoleNoticeInPlace(): boolean {
    const slot = typeof document !== "undefined" ? document.getElementById("console-notice-slot") : null;
    const toastSlot = typeof document !== "undefined" ? document.getElementById("console-floating-toast-slot") : null;
    if (!slot && !toastSlot) return false;
    if (slot) slot.innerHTML = this.renderConsoleNoticeHtml();
    if (toastSlot) toastSlot.innerHTML = this.renderFloatingToastHtml();
    return true;
  }

  private showToastNotice(kind: "ok" | "err", text: string) {
    if (this.toastTimer) {
      clearTimeout(this.toastTimer);
      this.toastTimer = null;
    }
    this.consoleNotice = { kind, text };
    if (!this.updateConsoleNoticeInPlace()) {
      this.render();
    }
    if (kind === "ok") {
      this.toastTimer = setTimeout(() => {
        this.toastTimer = null;
        if (this.consoleNotice?.text === text) {
          this.consoleNotice = null;
          this.updateConsoleNoticeInPlace();
        }
      }, 4200);
    }
  }

  private renderDraftPreviewHtml(): string {
    if (!this.draftInput || this.showPromptComposer) return "";
    return `<div style="margin-top:14px;border-top:1px solid var(--border-primary);padding-top:6px;border-bottom:1px solid var(--border-primary);padding-bottom:6px;">&gt; ${this.formatCursorDraft(
      this.draftInput
    )}</div>`;
  }

  private updateDraftPreviewInPlace(): boolean {
    const slot = typeof document !== "undefined" ? document.getElementById("term-draft-slot") : null;
    if (!slot) return false;
    slot.innerHTML = this.renderDraftPreviewHtml();
    return true;
  }

  private syncVoiceButtonInPlace() {
    if (typeof document === "undefined") return;
    const micLabel = this.isTranscribingVoice
      ? this.tr("Transcribing…", "转写中…")
      : this.isRecordingVoice
        ? this.tr("Stop", "停止")
        : "";
    const desktopBtn = document.getElementById("btn-voice-mic-desktop");
    if (desktopBtn) {
      desktopBtn.classList.toggle("is-recording", this.isRecordingVoice);
      desktopBtn.innerHTML = `${svgIcon("mic", 14)}${micLabel ? ` <span>${this.escapeHtml(micLabel)}</span>` : ""}`;
    }
    const mobileBtn = document.getElementById("btn-voice-mic");
    if (mobileBtn) {
      mobileBtn.classList.toggle("is-recording", this.isRecordingVoice);
    }
    const composerVoiceBtn = document.getElementById("btn-composer-voice");
    if (composerVoiceBtn) {
      composerVoiceBtn.classList.toggle("is-recording", this.isRecordingVoice);
      composerVoiceBtn.innerHTML = `${svgIcon("mic", 13)} <span>${this.escapeHtml(
        micLabel || this.tr("Voice", "语音")
      )}</span>`;
    }
  }

  private syncNeedsYouBarInPlace(term: HerdrTerminal | undefined) {
    const bar = document.getElementById("needs-you-bar");
    if (!bar) return;
    const show = Boolean(term && this.isNeedsYouStatus(term.status));
    bar.classList.toggle("visible", show);
    if (show && term) {
      const msgEl = document.getElementById("needs-you-msg");
      if (msgEl) {
        const appTag = term.status?.app ? `[${term.status.app}] ` : "";
        const detail =
          (term.status?.msg || term.status?.title || "").trim() ||
          this.tr("Agent is waiting for your approval", "AI Agent 正在等待你的交互确认");
        msgEl.textContent = `${appTag}${detail}`;
      }
    }
  }

  private syncScrollBottomPill(boxEl: HTMLElement | null) {
    const btn = document.getElementById("btn-scroll-bottom");
    if (!btn || !boxEl) return;
    const distFromBottom = boxEl.scrollHeight - boxEl.scrollTop - boxEl.clientHeight;
    btn.classList.toggle("visible", distFromBottom > 120);
  }

  private paintActiveTerminalViewport() {
    if (this.route !== "console" || !this.selectedDaemon || !this.activeTerminalAddr) return;
    const key = this.vtKey(this.selectedDaemon.id, this.activeTerminalAddr);
    const vt = this.vtTerminals.get(key);
    const screenEl = document.getElementById("term-screen-output");
    const boxEl = document.getElementById("term-box");
    if (screenEl && vt && this.vtStreamActive.has(key)) {
      // Preserve user text selection inside the terminal viewport during live output:
      // row-level dirty-diff skips selected rows (including partial-outside selections) while keeping
      // non-selected rows streaming at 60fps.
      const sel = typeof window !== "undefined" ? window.getSelection() : null;
      const selectingInside = Boolean(
        sel &&
          !sel.isCollapsed &&
          sel.rangeCount > 0 &&
          (boxEl?.contains(sel.anchorNode) ||
            boxEl?.contains(sel.focusNode) ||
            (typeof sel.containsNode === "function" && sel.containsNode(screenEl, true)))
      );
      const prevScrollTop = boxEl ? boxEl.scrollTop : 0;
      const wasNearBottom = boxEl
        ? boxEl.scrollHeight - boxEl.scrollTop - boxEl.clientHeight <= 28
        : true;
      const showPrimaryCursor = !(
        this.splitMode &&
        this.focusedPane === "secondary" &&
        this.secondaryTerminalAddr
      );
      const stats = vt.patchDom(
        screenEl,
        showPrimaryCursor,
        selectingInside && sel
          ? {
              anchorNode: sel.anchorNode,
              focusNode: sel.focusNode,
              containsNode:
                typeof sel.containsNode === "function"
                  ? (node) => sel.containsNode(node as unknown as Node, true)
                  : undefined,
            }
          : null
      );
      if (stats.skippedProtected) {
        this.pendingSelectionCatchup = true;
      } else if (!selectingInside) {
        this.pendingSelectionCatchup = false;
      }
      if (boxEl && !selectingInside) {
        boxEl.scrollTop = wasNearBottom ? boxEl.scrollHeight : prevScrollTop;
      }
      this.syncScrollBottomPill(boxEl);
      this.applySearchHighlightsInPlace(screenEl, vt);
    }

    if (this.splitMode && this.secondaryTerminalAddr && this.secondaryTerminalAddr !== this.activeTerminalAddr) {
      const secKey = this.vtKey(this.selectedDaemon.id, this.secondaryTerminalAddr);
      const secVt = this.vtTerminals.get(secKey);
      const secScreen = document.getElementById("term-screen-output-secondary");
      const secBox = document.getElementById("term-box-secondary");
      if (secScreen && secVt && this.vtStreamActive.has(secKey)) {
        const secNearBottom = secBox
          ? secBox.scrollHeight - secBox.scrollTop - secBox.clientHeight <= 28
          : true;
        const secPrevTop = secBox ? secBox.scrollTop : 0;
        secVt.patchDom(secScreen, this.focusedPane === "secondary", null);
        if (secBox) {
          secBox.scrollTop = secNearBottom ? secBox.scrollHeight : secPrevTop;
        }
      }
    }

    const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
    const titleEl = document.getElementById("active-term-title");
    if (titleEl && activeTerm) {
      titleEl.textContent =
        activeTerm.status?.title ||
        activeTerm.name ||
        this.tr("Coda Interactive AI Terminal", "Coda 交互式 AI 终端会话");
    }
    this.syncNeedsYouBarInPlace(activeTerm);
  }

  private isRelayWsConnected(): boolean {
    return Boolean(this.ws && typeof WebSocket !== "undefined" && this.ws.readyState === WebSocket.OPEN);
  }

  private isDirectWsConnected(): boolean {
    return Boolean(
      this.activeTransportMode === "lan_ws" &&
        this.directWs &&
        typeof WebSocket !== "undefined" &&
        this.directWs.readyState === WebSocket.OPEN
    );
  }

  private computeConsoleConnectionVisual() {
    return deriveConsoleConnectionVisualState({
      devicesCount: this.devices.length,
      selectedDaemon: this.selectedDaemon,
      relayWsOpen: this.isRelayWsConnected(),
      directWsOpen: this.isDirectWsConnected(),
      transportMode: this.activeTransportMode,
      rttMs: this.rttMs,
      tr: (en, zh) => this.tr(en, zh),
    });
  }

  private computeConsoleEmptyStateHtml(installCmd: string, activeTerm?: HerdrTerminal): string {
    return renderConsoleEmptyStateHtml({
      devicesCount: this.devices.length,
      selectedDaemon: this.selectedDaemon,
      activeTerminalName: activeTerm ? activeTerm.name || activeTerm.address : null,
      relayWsOpen: this.isRelayWsConnected(),
      directWsOpen: this.isDirectWsConnected(),
      transportMode: this.activeTransportMode,
      rttMs: this.rttMs,
      installCmd,
      tr: (en, zh) => this.tr(en, zh),
    });
  }

  private updateTransportFooterInPlace() {
    if (typeof document === "undefined") return;
    const connVisual = this.computeConsoleConnectionVisual();
    const sidebarConn = document.getElementById("console-sidebar-conn");
    if (sidebarConn) {
      sidebarConn.style.color = connVisual.sidebarColorCss;
    }
    const sidebarIcon = document.getElementById("console-sidebar-conn-icon");
    if (sidebarIcon) {
      sidebarIcon.innerHTML = svgIcon(connVisual.icon, 11);
    }
    const rttEl = document.getElementById("console-rtt-label");
    if (rttEl) {
      rttEl.textContent = connVisual.sidebarLabel;
    }
    const railPill = document.getElementById("rail-status-pill");
    if (railPill) {
      railPill.style.borderColor = connVisual.railBorderCss;
      railPill.setAttribute("title", connVisual.railTitle);
      railPill.setAttribute("aria-label", connVisual.railTitle);
    }
    const railDot = document.getElementById("rail-status-dot");
    if (railDot) {
      railDot.style.background = connVisual.railDotBgCss;
      railDot.style.boxShadow = connVisual.railDotShadowCss;
    }
    const railMode = document.getElementById("rail-transport-mode");
    if (railMode) {
      railMode.innerHTML = connVisual.railTransportBadgeHtml;
    }
    const metaEl = document.getElementById("term-footer-meta");
    if (metaEl) {
      const hint = this.tr(
        "⌘+Click URL · Ctrl+V/⌘V screenshot to upload",
        "⌘+点击打开链接 · Ctrl+V/⌘V 粘贴截图直传终端"
      );
      metaEl.textContent = `${connVisual.footerTransport} · E2EE X25519 + ChaCha20 · ${this.termCols}×${this.termRows} · ${this.terminals.length} PTY · ${hint}`;
    }
    const screenEl = document.getElementById("term-screen-output");
    if (screenEl && screenEl.getAttribute("data-vt-mode") === "placeholder") {
      const baseUrl = this.siteInfo?.public_base_url || window.location.origin;
      const installCmd =
        this.siteInfo?.install_command || `curl -fsSL ${baseUrl}/install.sh | sh`;
      const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
      screenEl.innerHTML = this.computeConsoleEmptyStateHtml(installCmd, activeTerm);
    }
  }

  private computeTerminalDimsFromDom(): { cols: number; rows: number } {
    const box = typeof document !== "undefined" ? document.getElementById("term-box") : null;
    if (box && box.clientWidth > 120 && box.clientHeight > 80) {
      let charW = 7.85;
      let lineH = 21.0;
      try {
        const probe = document.createElement("span");
        probe.textContent = "0000000000";
        probe.style.cssText =
          "position:absolute;visibility:hidden;pointer-events:none;white-space:pre;font:inherit;line-height:inherit;";
        box.appendChild(probe);
        const rect = probe.getBoundingClientRect();
        box.removeChild(probe);
        if (rect.width > 20) charW = rect.width / 10;
        if (rect.height > 8) lineH = rect.height;
      } catch {
        // fallback to defaults
      }
      const cols = Math.max(40, Math.min(240, Math.floor((box.clientWidth - 20) / charW)));
      const rows = Math.max(12, Math.min(90, Math.floor((box.clientHeight - 76) / lineH)));
      return { cols, rows };
    }
    const vw = typeof window !== "undefined" ? window.innerWidth : 1280;
    const vh = typeof window !== "undefined" ? window.innerHeight : 800;
    const cols = Math.max(40, Math.min(240, Math.floor((vw - (this.showSidebar && vw >= 900 ? 390 : 80)) / 8.2)));
    const rows = Math.max(12, Math.min(90, Math.floor((vh - 210) / 21)));
    return { cols, rows };
  }

  private scheduleTerminalResizeSync() {
    if (this.resizeDebounceTimer) clearTimeout(this.resizeDebounceTimer);
    this.resizeDebounceTimer = setTimeout(() => {
      this.resizeDebounceTimer = null;
      this.measureAndSyncTerminalSize(true);
    }, 120);
  }

  private measureAndSyncTerminalSize(sendRemote = true) {
    // Avoid SIGWINCH row thrashing when mobile virtual keyboard shrinks visualViewport temporarily.
    const vv = typeof window !== "undefined" ? window.visualViewport : null;
    if (
      sendRemote &&
      vv &&
      typeof window !== "undefined" &&
      window.innerWidth <= 768 &&
      window.innerHeight - vv.height > 120
    ) {
      return;
    }
    const { cols, rows } = this.computeTerminalDimsFromDom();
    const changed = cols !== this.termCols || rows !== this.termRows;
    this.termCols = cols;
    this.termRows = rows;
    if (this.selectedDaemon && this.activeTerminalAddr) {
      const vt = this.getOrCreateVt(this.selectedDaemon.id, this.activeTerminalAddr);
      vt.resize(cols, rows);
      this.updateTransportFooterInPlace();
      if (changed && sendRemote) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "pty_resize",
          terminal: this.activeTerminalAddr,
          cols,
          rows,
        });
      }
    }
    if (
      this.selectedDaemon &&
      this.splitMode &&
      this.secondaryTerminalAddr &&
      this.secondaryTerminalAddr !== this.activeTerminalAddr
    ) {
      const secVt = this.getOrCreateVt(this.selectedDaemon.id, this.secondaryTerminalAddr);
      secVt.resize(cols, rows);
      if (changed && sendRemote) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "pty_resize",
          terminal: this.secondaryTerminalAddr,
          cols,
          rows,
        });
      }
    }
  }

  /** V3-08 / V3-25: Tears down active Direct LAN WebSocket & cancels in-flight LAN probes. */
  private teardownDirectWs() {
    this.directProbeGeneration += 1;
    if (this.directProbeTimer) {
      clearTimeout(this.directProbeTimer);
      this.directProbeTimer = null;
    }
    if (this.directWs) {
      this.directWs.onopen = null;
      this.directWs.onmessage = null;
      this.directWs.onerror = null;
      this.directWs.onclose = null;
      try { this.directWs.close(); } catch {}
      this.directWs = null;
    }
    this.directWsPeerId = null;
    this.directE2eeSessions.clear();
    this.activeTransportMode = "relay_ws";
    this.negotiatingPeerId = null;
  }

  /** V3-08: Single centralized daemon switch ensuring clean detach + voice cleanup + directWs teardown before switching. */
  private selectDaemon(peer: Device | null, opts: { initialTerminalAddr?: string | null } = {}) {
    const prevId = this.selectedDaemon?.id ?? null;
    const nextId = peer?.id ?? null;
    if (prevId !== nextId || !nextId) {
      this.cancelActiveVoiceCapture();
    }
    if (prevId && prevId !== nextId) {
      this.detachAllOpenTerminals();
      this.teardownDirectWs();
    } else if (!nextId) {
      if (prevId) {
        this.detachAllOpenTerminals();
      }
      this.teardownDirectWs();
    }
    this.selectedDaemon = peer;
    if (peer) {
      writeLocalPref("coda_last_daemon_id", peer.id);
    }
    if (prevId !== nextId) {
      this.terminals = peer ? [...(this.terminalsByPeer.get(peer.id) || [])] : [];
      this.repos = [];
      let resolvedActive = opts.initialTerminalAddr ?? null;
      if (!resolvedActive && this.terminals.length > 0) {
        const savedTerm = readLocalPref("coda_last_terminal_addr", "");
        const restored = savedTerm ? this.terminals.find((t) => t.address === savedTerm) : undefined;
        resolvedActive = restored ? restored.address : this.terminals[0].address;
      }
      this.activeTerminalAddr = resolvedActive;
      this.secondaryTerminalAddr =
        this.splitMode && resolvedActive
          ? this.terminals.find((t) => t.address !== resolvedActive)?.address ?? null
          : null;
      this.focusedPane = "primary";
      this.fileBrowser = createInitialFileBrowserState();
      this.portPreview = createInitialPortPreviewState();
      this.gitDiff = createInitialGitDiffState();
      this.rttMs = null;
      this.lastWorkspacesQueryAt = 0;
      this.pendingSelectionCatchup = false;
      if (this.activeTerminalAddr) {
        writeLocalPref("coda_last_terminal_addr", this.activeTerminalAddr);
      }
      if (peer?.is_online) {
        this.queryDaemonOverview(peer, true);
        if (this.activeTerminalAddr) {
          this.attachAndReadTerminal(this.activeTerminalAddr);
        }
        if (
          this.splitMode &&
          this.secondaryTerminalAddr &&
          this.secondaryTerminalAddr !== this.activeTerminalAddr
        ) {
          this.attachAndReadTerminal(this.secondaryTerminalAddr);
        }
        void this.tryNegotiateDirectLanWs(peer);
      }
    } else if (peer?.is_online) {
      if (opts.initialTerminalAddr && opts.initialTerminalAddr !== this.activeTerminalAddr) {
        this.detachActiveTerminal();
        this.activeTerminalAddr = opts.initialTerminalAddr;
        writeLocalPref("coda_last_terminal_addr", this.activeTerminalAddr);
        this.attachAndReadTerminal(this.activeTerminalAddr);
      }
      this.queryDaemonOverview(peer, true);
      void this.tryNegotiateDirectLanWs(peer);
    }
  }

  private jumpToNextNeedsYouTerminal() {
    const items = collectNeedsAttentionItems(
      this.devices,
      this.terminalsByPeer,
      this.selectedDaemon?.id ?? null,
      this.terminals
    );
    const next = pickNextNeedsAttentionItem(
      items,
      this.selectedDaemon?.id ?? null,
      this.activeTerminalAddr
    );
    if (!next) {
      this.showToastNotice(
        "ok",
        this.tr("No terminals currently need your attention", "当前没有等待审批 (Needs-You) 的终端")
      );
      return;
    }
    this.focusNotifiedTerminal(next.daemonId, next.terminalAddr);
    this.showToastNotice(
      "ok",
      this.tr(
        `Jumped to ${next.terminalName} (${next.daemonName})`,
        `已跳转至待审批终端 ${next.terminalName} (${next.daemonName})`
      )
    );
  }

  private stopAllTransportsAndTimers() {
    this.relayGeneration += 1;
    this.cancelActiveVoiceCapture();
    if (this.toastTimer) {
      clearTimeout(this.toastTimer);
      this.toastTimer = null;
    }
    if (this.relayKeepAliveTimer) {
      clearInterval(this.relayKeepAliveTimer);
      this.relayKeepAliveTimer = null;
    }
    if (this.relayReconnectTimer) {
      clearTimeout(this.relayReconnectTimer);
      this.relayReconnectTimer = null;
    }
    if (this.eventsReconnectTimer) {
      clearTimeout(this.eventsReconnectTimer);
      this.eventsReconnectTimer = null;
    }
    this.teardownDirectWs();
    this.notifyTracker.reset();
    if (this.resizeDebounceTimer) {
      clearTimeout(this.resizeDebounceTimer);
      this.resizeDebounceTimer = null;
    }
    if (this.termPaintRafId !== null && typeof cancelAnimationFrame !== "undefined") {
      cancelAnimationFrame(this.termPaintRafId);
      this.termPaintRafId = null;
    }
    if (this.termResizeObserver) {
      this.termResizeObserver.disconnect();
      this.termResizeObserver = null;
    }
    if (this.eventsWs) {
      this.eventsWs.onopen = null;
      this.eventsWs.onmessage = null;
      this.eventsWs.onerror = null;
      this.eventsWs.onclose = null;
      try { this.eventsWs.close(); } catch {}
      this.eventsWs = null;
    }
    if (this.ws) {
      this.ws.onopen = null;
      this.ws.onmessage = null;
      this.ws.onerror = null;
      this.ws.onclose = null;
      try { this.ws.close(); } catch {}
      this.ws = null;
    }
  }

  private syncUserScopedPrefs() {
    if (!this.user?.id) {
      this.onboardingDismissed = false;
      return;
    }
    this.onboardingDismissed =
      readLocalPref(onboardingDismissedStorageKey(this.user.id), "0") === "1";
  }

  private loadInitialCachedSnippets(): PromptSnippet[] {
    try {
      const raw = readLocalPref(SNIPPETS_CACHE_STORAGE_KEY, "");
      if (raw) {
        const parsedJson = JSON.parse(raw);
        if (Array.isArray(parsedJson)) {
          return normalizePromptSnippetsList(parsedJson);
        }
      }
    } catch {
      // ignore cache parse errors
    }
    return [...DEFAULT_PROMPT_SNIPPETS];
  }

  private cacheSnippetsLocally(snippets: readonly PromptSnippet[]) {
    try {
      writeLocalPref(SNIPPETS_CACHE_STORAGE_KEY, JSON.stringify(snippets));
    } catch {
      // ignore storage errors
    }
  }

  private async resolvePushRegistration(
    regHint?: ServiceWorkerRegistration | null
  ): Promise<ServiceWorkerRegistration | null> {
    if (regHint) {
      this.swRegistration = regHint;
      return regHint;
    }
    if (this.swRegistration) return this.swRegistration;
    if (typeof navigator === "undefined" || typeof navigator.serviceWorker?.getRegistration !== "function") {
      return null;
    }
    try {
      const reg =
        (await navigator.serviceWorker.getRegistration("/sw.js")) ??
        (await navigator.serviceWorker.getRegistration("/")) ??
        (await navigator.serviceWorker.getRegistration()) ??
        null;
      if (reg) this.swRegistration = reg;
      return reg;
    } catch {
      return null;
    }
  }

  private async unsubscribeWebPushForSignOut(): Promise<void> {
    const res = await teardownWebPushOnSignOut({
      getSubscription: async () => {
        const reg = await this.resolvePushRegistration();
        return await reg?.pushManager?.getSubscription();
      },
      knownEndpoint: this.webPushEndpoint,
      hasAuthToken: Boolean(this.token),
      unsubscribeServer: (endpoint) =>
        this.api("/api/v1/me/web-push/unsubscribe", "POST", { endpoint }),
    });
    this.webPushThisDeviceSubscribed = res.webPushThisDeviceSubscribed;
    this.webPushEndpoint = res.webPushEndpoint;
    this.webPushCount = res.webPushCount;
  }

  private isWebPushApiSupported(): boolean {
    if (typeof window === "undefined" || typeof navigator === "undefined") {
      return false;
    }
    return Boolean(
      "serviceWorker" in navigator &&
        typeof navigator.serviceWorker?.register === "function" &&
        "PushManager" in window &&
        typeof Notification !== "undefined"
    );
  }

  private async registerServiceWorkerIfSupported(): Promise<ServiceWorkerRegistration | null> {
    if (typeof navigator === "undefined" || typeof navigator.serviceWorker?.register !== "function") {
      return null;
    }
    try {
      const reg = await navigator.serviceWorker.register("/sw.js", { scope: "/" });
      this.swRegistration = reg;
      await this.syncLocalPushSubscriptionState(reg);
      return reg;
    } catch {
      return null;
    }
  }

  private async syncLocalPushSubscriptionState(
    regHint?: ServiceWorkerRegistration | null
  ): Promise<void> {
    if (!this.isWebPushApiSupported()) {
      this.webPushThisDeviceSubscribed = false;
      this.webPushEndpoint = "";
      return;
    }
    try {
      const reg = await this.resolvePushRegistration(regHint);
      const sub = await reg?.pushManager?.getSubscription();
      this.webPushThisDeviceSubscribed = Boolean(sub);
      this.webPushEndpoint = sub?.endpoint || "";
    } catch {
      this.webPushThisDeviceSubscribed = false;
      this.webPushEndpoint = "";
    }
  }

  private async refreshWebPushMetadata(): Promise<void> {
    const reqToken = this.token;
    let serverCount: number | null = null;
    const tasks: Array<Promise<void>> = [
      this.syncLocalPushSubscriptionState(),
      this.api("/api/v1/push/vapid-public-key")
        .then((res) => {
          if (res && typeof res === "object") {
            this.vapidInfo = {
              enabled: Boolean(res.enabled),
              public_key: String(res.public_key || ""),
            };
            this.vapidLoaded = true;
          }
        })
        .catch(() => {
          // ignore transient error
        }),
    ];
    if (reqToken) {
      tasks.push(
        this.api("/api/v1/me/notifications")
          .then((notif) => {
            if (this.token === reqToken && notif && typeof notif === "object") {
              if (typeof notif.web_push_count === "number") {
                serverCount = notif.web_push_count;
                this.webPushCount = notif.web_push_count;
              }
            }
          })
          .catch(() => {
            // ignore
          })
      );
    }
    await Promise.allSettled(tasks);
    if (this.token === reqToken && serverCount === 0 && !this.webPushBusy) {
      this.webPushThisDeviceSubscribed = false;
    }
  }

  private async enableLockScreenWebPush(): Promise<void> {
    if (this.webPushBusy || !this.token) return;
    if (!this.isWebPushApiSupported()) {
      const msg = detectBrowserIosNonStandalone()
        ? this.tr(
            "On iPhone, tap Share → Add to Home Screen first, then open Coda from your Home Screen to enable lock-screen push.",
            "在 iPhone 上请先轻点「分享 → 添加到主屏幕」，再从桌面图标打开 Coda 开启锁屏推送。"
          )
        : this.tr(
            "This browser context does not support Service Worker Web Push (HTTPS or localhost required).",
            "当前浏览器上下文不支持 Service Worker Web Push（需使用 HTTPS 或 localhost）。"
          );
      this.settingsFeedback = { kind: "err", text: msg };
      this.showToastNotice("err", msg);
      this.render();
      return;
    }

    this.webPushBusy = true;
    this.render();

    try {
      if (!this.vapidLoaded || !this.vapidInfo.public_key) {
        const vRes = await this.api("/api/v1/push/vapid-public-key");
        this.vapidInfo = {
          enabled: Boolean(vRes?.enabled),
          public_key: String(vRes?.public_key || ""),
        };
        this.vapidLoaded = true;
      }
      if (!this.vapidInfo.enabled || !this.vapidInfo.public_key) {
        throw new Error(
          this.tr(
            "Server VAPID Web Push is not enabled on this coordinator.",
            "当前主控服务端未启用 VAPID Web Push。"
          )
        );
      }

      const applicationServerKey = vapidBase64UrlToUint8Array(this.vapidInfo.public_key);
      const perm = await requestBrowserNotificationPermission();
      if (perm !== "granted") {
        throw new Error(
          this.tr(
            "Notification permission was not granted in browser settings.",
            "未授予浏览器系统通知权限，请在地址栏权限设置中允许通知。"
          )
        );
      }

      let reg = this.swRegistration ?? (await this.registerServiceWorkerIfSupported());
      if (navigator.serviceWorker?.ready) {
        reg = await navigator.serviceWorker.ready;
        this.swRegistration = reg;
      }
      if (!reg?.pushManager) {
        throw new Error(
          this.tr("Service Worker PushManager unavailable.", "Service Worker PushManager 不可用。")
        );
      }

      let pushSub = await reg.pushManager.getSubscription();
      if (
        pushSub &&
        isPushSubscriptionVapidKeyMismatch(
          pushSub.options?.applicationServerKey,
          applicationServerKey
        )
      ) {
        await pushSub.unsubscribe().catch(() => false);
        pushSub = null;
      }
      if (!pushSub) {
        try {
          pushSub = await reg.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey,
          });
        } catch {
          const stale = await reg.pushManager.getSubscription();
          if (stale) {
            await stale.unsubscribe().catch(() => false);
          }
          pushSub = await reg.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey,
          });
        }
      }

      const { endpoint, p256dh, auth } = extractPushSubscriptionKeys(pushSub);
      const subRes = await this.api("/api/v1/me/web-push/subscribe", "POST", {
        endpoint,
        keys: { p256dh, auth },
        user_agent: typeof navigator !== "undefined" ? navigator.userAgent || "" : "",
      });

      this.webPushThisDeviceSubscribed = true;
      this.webPushEndpoint = endpoint;
      this.webPushCount = Number(subRes?.web_push_count ?? Math.max(1, this.webPushCount));
      this.notificationsEnabled = true;
      writeLocalPref("coda_notify_enabled", "1");

      const okText = this.tr(
        `Lock-screen push enabled (${this.webPushCount} active device(s)).`,
        `已开启锁屏系统推送（当前共 ${this.webPushCount} 台订阅设备）。`
      );
      this.settingsFeedback = { kind: "ok", text: okText };
      this.showToastNotice("ok", okText);
    } catch (err: any) {
      const errText = String(
        err?.message || this.tr("Failed enabling lock-screen push", "开启锁屏推送失败")
      );
      this.settingsFeedback = { kind: "err", text: errText };
      this.showToastNotice("err", errText);
    } finally {
      this.webPushBusy = false;
      this.renderSettingsPreservingInputs();
      if (!this.showSettingsModal) {
        this.render();
      }
    }
  }

  private async disableLockScreenWebPush(): Promise<void> {
    if (this.webPushBusy || !this.token) return;
    this.webPushBusy = true;
    this.render();

    try {
      const reg = await this.resolvePushRegistration();
      const pushSub = await reg?.pushManager?.getSubscription();
      const endpoint = String(pushSub?.endpoint || this.webPushEndpoint || "");
      if (pushSub) {
        await pushSub.unsubscribe().catch(() => false);
      }
      const res = await this.api("/api/v1/me/web-push/unsubscribe", "POST", {
        endpoint,
      });
      this.webPushThisDeviceSubscribed = false;
      this.webPushEndpoint = "";
      this.webPushCount = Number(res?.web_push_count ?? 0);
      const msg = this.tr(
        "Disabled lock-screen push on this device.",
        "已在当前设备停用锁屏推送。"
      );
      this.settingsFeedback = { kind: "ok", text: msg };
      this.showToastNotice("ok", msg);
    } catch (err: any) {
      const errText = String(
        err?.message || this.tr("Failed disabling lock-screen push", "停用锁屏推送失败")
      );
      this.settingsFeedback = { kind: "err", text: errText };
      this.showToastNotice("err", errText);
    } finally {
      this.webPushBusy = false;
      this.renderSettingsPreservingInputs();
      if (!this.showSettingsModal) {
        this.render();
      }
    }
  }

  private async loadPromptSnippets(): Promise<void> {
    const reqToken = this.token;
    if (!reqToken) return;
    try {
      const res = await this.api("/api/v1/me/snippets");
      if (!this.token || this.token !== reqToken) return;
      if (Array.isArray(res?.snippets)) {
        this.promptSnippets = normalizePromptSnippetsList(res.snippets);
        this.cacheSnippetsLocally(this.promptSnippets);
      }
    } catch {
      // keep cached/default snippets on offline/error
    }
  }

  private async savePromptSnippetsToServer(nextSnippets: PromptSnippet[]): Promise<boolean> {
    if (!this.token) return false;
    const reqToken = this.token;
    const res = await this.api("/api/v1/me/snippets", "PUT", {
      snippets: nextSnippets,
    });
    if (!this.token || this.token !== reqToken) return false;
    const saved = normalizePromptSnippetsList(res?.snippets);
    this.promptSnippets = saved.length > 0 || nextSnippets.length === 0 ? saved : nextSnippets;
    this.cacheSnippetsLocally(this.promptSnippets);
    return true;
  }

  private async copyTextWithFallback(text: string): Promise<boolean> {
    try {
      if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
        return true;
      }
    } catch {
      // fall through to execCommand fallback
    }
    if (typeof document !== "undefined") {
      try {
        const ta = document.createElement("textarea");
        ta.value = text;
        ta.style.cssText = "position:fixed;left:-9999px;top:-9999px;opacity:0;";
        document.body.appendChild(ta);
        ta.focus();
        ta.select();
        const ok = document.execCommand("copy");
        document.body.removeChild(ta);
        return Boolean(ok);
      } catch {
        return false;
      }
    }
    return false;
  }

  private async bootstrapMesh() {
    this.syncUserScopedPrefs();
    this.myClientDevice = await this.api("/api/v1/devices/register-client", "POST", {
      name: this.webIdentity.name,
      platform: "web",
      ed25519_pubkey: this.webIdentity.ed25519PublicB64,
      x25519_pubkey: this.webIdentity.x25519PublicB64,
    });
    await this.refreshDevices();
    void this.refreshWebPushMetadata().then(() => {
      if (this.route === "console" && !this.isTypingIntoInput() && !this.isAnyModalOpen()) {
        this.render();
      }
    });
    void this.loadPromptSnippets().then(() => {
      if (
        this.route === "console" &&
        this.showPromptComposer &&
        !this.isTypingIntoInput() &&
        !this.isAnyModalOpen()
      ) {
        this.render();
      }
    });
    this.connectAccountEventsWebSocket();
    await this.connectRelayWebSocket();
  }

  private async refreshDevices() {
    const reqToken = this.token;
    if (!reqToken) return;
    const all: Device[] = await this.api("/api/v1/devices");
    if (!this.token || this.token !== reqToken) return;
    this.allDevices = all;
    this.devices = all.filter((d) => d.role === "daemon" || d.role === "hybrid");
    if (!this.selectedDaemon && this.devices.length > 0) {
      const savedId = readLocalPref("coda_last_daemon_id", "");
      const restored = savedId ? this.devices.find((d) => d.id === savedId) : undefined;
      this.selectedDaemon =
        (restored && restored.is_online ? restored : undefined) ||
        this.devices.find((d) => d.is_online) ||
        restored ||
        this.devices[0];
    } else if (this.selectedDaemon) {
      const stillPresent = this.devices.find((d) => d.id === this.selectedDaemon!.id) || null;
      if (!stillPresent) {
        const fallback = this.devices.find((d) => d.is_online) || this.devices[0] || null;
        this.selectDaemon(fallback);
      } else {
        this.selectedDaemon = stillPresent;
      }
    }
  }

  private async loadAccountSettingsData() {
    if (!this.token) return;
    const reqToken = this.token;
    const tasks: Array<Promise<void>> = [
      this.refreshDevices().catch(() => {
        // ignore transient error
      }),
      this.refreshWebPushMetadata(),
      this.loadPromptSnippets(),
      this.api("/api/v1/auth/sessions")
        .then((sessions) => {
          if (this.token === reqToken) {
            this.authSessions = Array.isArray(sessions) ? sessions : [];
          }
        })
        .catch(() => {
          if (this.token === reqToken) this.authSessions = [];
        }),
      this.api("/api/v1/me/notifications")
        .then((notif) => {
          if (this.token === reqToken && notif && typeof notif === "object") {
            if (typeof notif.web_push_count === "number") {
              this.webPushCount = notif.web_push_count;
            }
            if (!this.settingsDirtyFields.has("webhooks")) {
              this.notificationSettings = {
                bark_url: String(notif.bark_url || ""),
                telegram_bot_token: String(notif.telegram_bot_token || ""),
                telegram_chat_id: String(notif.telegram_chat_id || ""),
                feishu_webhook_url: String(notif.feishu_webhook_url || ""),
                discord_webhook_url: String(notif.discord_webhook_url || ""),
                generic_webhook_url: String(notif.generic_webhook_url || ""),
                notify_on_needs_you: notif.notify_on_needs_you !== false,
                notify_on_finished: notif.notify_on_finished !== false,
              };
            }
          }
        })
        .catch(() => {
          // keep local notificationSettings untouched if endpoint fails
        }),
    ];
    if (this.user?.is_admin) {
      tasks.push(
        this.api("/api/v1/invites")
          .then((list) => {
            if (this.token === reqToken) {
              this.invitesList = Array.isArray(list) ? list : [];
            }
          })
          .catch(() => {
            if (this.token === reqToken) this.invitesList = [];
          }),
        this.api("/api/v1/admin/users")
          .then((res) => {
            if (this.token === reqToken) {
              this.adminUsers = Array.isArray(res?.users)
                ? res.users
                : Array.isArray(res)
                  ? res
                  : [];
            }
          })
          .catch(() => {
            if (this.token === reqToken) this.adminUsers = [];
          })
      );
    }
    await Promise.allSettled(tasks);
  }

  private renderSettingsPreservingInputs() {
    if (!this.showSettingsModal) return;
    if (this.settingsTab === "alerts" && this.settingsDirtyFields.has("webhooks")) {
      return;
    }
    if (this.settingsTab === "voice" && this.settingsDirtyFields.has("voice")) {
      return;
    }
    const modal = typeof document !== "undefined" ? document.getElementById("modal-bg") : null;
    const savedValues = new Map<string, string>();
    const savedChecked = new Map<string, boolean>();
    let activeKey: string | null = null;
    let selStart: number | null = null;
    let selEnd: number | null = null;

    if (modal) {
      const activeEl = document.activeElement as HTMLInputElement | null;
      modal.querySelectorAll<HTMLInputElement>("input[name]").forEach((inp) => {
        const key = `${inp.form?.id || "form"}:${inp.name}`;
        if (inp.type === "checkbox") {
          savedChecked.set(key, inp.checked);
        } else {
          savedValues.set(key, inp.value);
        }
        if (inp === activeEl) {
          activeKey = key;
          try {
            selStart = inp.selectionStart;
            selEnd = inp.selectionEnd;
          } catch {
            selStart = null;
            selEnd = null;
          }
        }
      });
    }

    this.render();

    const nextModal = typeof document !== "undefined" ? document.getElementById("modal-bg") : null;
    if (nextModal && (savedValues.size > 0 || savedChecked.size > 0)) {
      nextModal.querySelectorAll<HTMLInputElement>("input[name]").forEach((inp) => {
        const key = `${inp.form?.id || "form"}:${inp.name}`;
        if (inp.type === "checkbox" && savedChecked.has(key) && this.settingsDirtyFields.has("webhooks")) {
          inp.checked = savedChecked.get(key)!;
        } else if (inp.type !== "checkbox" && savedValues.has(key)) {
          const prevVal = savedValues.get(key)!;
          if (
            prevVal &&
            (key === activeKey ||
              this.settingsDirtyFields.has("webhooks") ||
              this.settingsDirtyFields.has("voice") ||
              this.settingsDirtyFields.has(key))
          ) {
            inp.value = prevVal;
          }
        }
        if (key === activeKey) {
          inp.focus();
          if (selStart !== null && selEnd !== null) {
            try {
              inp.setSelectionRange(selStart, selEnd);
            } catch {
              // ignore unsupported input types
            }
          }
        }
      });
    }
  }

  private connectAccountEventsWebSocket() {
    if (!this.token || typeof WebSocket === "undefined") return;
    if (this.eventsReconnectTimer) {
      clearTimeout(this.eventsReconnectTimer);
      this.eventsReconnectTimer = null;
    }
    if (this.eventsWs) {
      this.eventsWs.onopen = null;
      this.eventsWs.onmessage = null;
      this.eventsWs.onerror = null;
      this.eventsWs.onclose = null;
      try { this.eventsWs.close(); } catch {}
      this.eventsWs = null;
    }

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const sock = new WebSocket(`${proto}//${location.host}/api/v1/events/ws`, [
      "coda-auth",
      `coda-bearer.${this.token}`,
    ]);
    this.eventsWs = sock;

    sock.onopen = () => {
      this.eventsBackoffMs = 1000;
    };

    sock.onmessage = async (ev) => {
      let evt: any;
      try {
        evt = JSON.parse(String(ev.data));
      } catch {
        return;
      }
      if (evt.kind === "devices_changed" || evt.kind === "peer_presence" || evt.kind === "connected") {
        try {
          const prevSelectedId = this.selectedDaemon?.id;
          const wasOnline = this.selectedDaemon?.is_online ?? false;
          await this.refreshDevices();
          if (!this.token) return;
          if (this.selectedDaemon?.is_online && (this.selectedDaemon.id !== prevSelectedId || !wasOnline)) {
            this.queryDaemonOverview(this.selectedDaemon);
            void this.tryNegotiateDirectLanWs(this.selectedDaemon);
          }
          if (!this.isTypingIntoInput() && !this.isAnyModalOpen()) {
            this.render();
          }
        } catch {
          // ignore transient refresh failure
        }
      }
    };

    const scheduleReconnect = () => {
      if (this.eventsWs !== sock) return;
      this.eventsWs = null;
      if (!this.token) return;
      if (this.eventsReconnectTimer) return;
      const delay = this.eventsBackoffMs;
      this.eventsBackoffMs = Math.min(15_000, this.eventsBackoffMs * 2);
      this.eventsReconnectTimer = setTimeout(() => {
        this.eventsReconnectTimer = null;
        this.connectAccountEventsWebSocket();
      }, delay);
    };

    sock.onerror = scheduleReconnect;
    sock.onclose = scheduleReconnect;
  }

  private scheduleRelayReconnect() {
    if (!this.token || !this.myClientDevice || this.relayReconnectTimer) return;
    const delay = this.relayBackoffMs;
    this.relayBackoffMs = Math.min(15_000, this.relayBackoffMs * 2);
    this.relayReconnectTimer = setTimeout(() => {
      this.relayReconnectTimer = null;
      void this.connectRelayWebSocket();
    }, delay);
  }

  private collectRelayWsCandidates(relays: RelayNodeInfo[] | undefined): string[] {
    const out: string[] = [];
    const pushUnique = (u: string) => {
      if (!u) return;
      if (location.protocol === "https:") {
        const isLocalWs = u.startsWith("ws://127.0.0.1") || u.startsWith("ws://localhost");
        if (!u.startsWith("wss://") && !isLocalWs) return;
      }
      if (!out.includes(u)) out.push(u);
    };
    const sorted = [...(relays || [])].sort((a, b) => a.priority - b.priority);
    for (const r of sorted) {
      const ep = String(r.endpoint || "").trim();
      if (!ep) continue;
      if (ep.startsWith("wss://") || ep.startsWith("ws://")) {
        pushUnique(ep);
      } else if (r.mode === "wss") {
        pushUnique(`wss://${ep.replace(/^https?:\/\//, "")}`);
      } else if (r.mode === "ws") {
        pushUnique(`ws://${ep.replace(/^https?:\/\//, "")}`);
      }
    }
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    pushUnique(`${proto}//${location.host}/v1/tunnel/ws`);
    return out;
  }

  private enrollBackgroundDaemonsForNotifications(onlyPeerId?: string) {
    if (!this.token) return;
    for (const dev of this.devices) {
      if (!dev.is_online || dev.id === this.selectedDaemon?.id) continue;
      if (onlyPeerId && dev.id.toLowerCase() !== onlyPeerId.toLowerCase()) continue;
      this.sendEncryptedViaRelay(dev, {
        op: "list_terminals",
        req_id: `bg-ls-${Date.now()}-${dev.id.slice(0, 6)}`,
        include_finished: false,
      });
    }
  }

  private async connectRelayWebSocket() {
    if (!this.myClientDevice || !this.token || typeof WebSocket === "undefined") return;
    if (this.relayReconnectTimer) {
      clearTimeout(this.relayReconnectTimer);
      this.relayReconnectTimer = null;
    }

    const gen = Math.max(this.relayGeneration + 1, Date.now());
    this.relayGeneration = gen;
    const reqToken = this.token;

    let ticketRes: { ticket: string; relays?: RelayNodeInfo[] };
    try {
      ticketRes = await this.api("/api/v1/relay/ticket", "POST", {
        device_id: this.myClientDevice.id,
        peer_device_id: null,
      });
    } catch {
      if (this.token === reqToken && gen === this.relayGeneration) {
        this.scheduleRelayReconnect();
      }
      return;
    }

    if (!this.token || this.token !== reqToken || gen !== this.relayGeneration) return;
    const candidates = this.collectRelayWsCandidates(ticketRes.relays);

    let connected = false;
    for (const wsUrl of candidates) {
      if (!this.token || this.token !== reqToken || gen !== this.relayGeneration) return;
      const ok = await this.tryOpenRelayCandidate(wsUrl, ticketRes.ticket, gen);
      if (ok) {
        connected = true;
        break;
      }
    }

    if (!connected && this.token === reqToken && gen === this.relayGeneration) {
      this.scheduleRelayReconnect();
    }
  }

  private tryOpenRelayCandidate(wsUrl: string, ticket: string, gen: number): Promise<boolean> {
    return new Promise((resolve) => {
      let settled = false;
      let sock: WebSocket;
      try {
        sock = new WebSocket(wsUrl);
      } catch {
        resolve(false);
        return;
      }

      const failCandidate = () => {
        if (settled) return;
        settled = true;
        clearTimeout(authTimer);
        sock.onopen = null;
        sock.onmessage = null;
        sock.onerror = null;
        sock.onclose = null;
        try { sock.close(); } catch {}
        resolve(false);
      };

      const authTimer = setTimeout(failCandidate, 5000);

      sock.onopen = () => {
        if (!this.token || gen !== this.relayGeneration) {
          failCandidate();
          return;
        }
        sock.send(
          JSON.stringify({
            frame_type: "auth_bind",
            ticket,
            lane: "control_p0",
            generation: gen,
          })
        );
      };

      sock.onmessage = async (ev) => {
        let frame: any;
        try {
          frame = JSON.parse(String(ev.data));
        } catch {
          return;
        }

        if (!settled) {
          if (frame.frame_type === "auth_ack" && frame.ok) {
            if (!this.token || gen !== this.relayGeneration) {
              failCandidate();
              return;
            }
            settled = true;
            clearTimeout(authTimer);
            this.relayBackoffMs = 1000;
            if (this.relayKeepAliveTimer) {
              clearInterval(this.relayKeepAliveTimer);
              this.relayKeepAliveTimer = null;
            }
            if (this.ws && this.ws !== sock) {
              this.ws.onopen = null;
              this.ws.onmessage = null;
              this.ws.onerror = null;
              this.ws.onclose = null;
              try { this.ws.close(); } catch {}
            }
            this.ws = sock;
            this.updateTransportFooterInPlace();
            this.relayKeepAliveTimer = setInterval(() => {
              if (this.ws === sock && sock.readyState === WebSocket.OPEN) {
                try {
                  sock.send(JSON.stringify({ frame_type: "keep_alive", ts_ms: Date.now() }));
                } catch {
                  // ignore
                }
              }
            }, 25_000);
            if (this.selectedDaemon?.is_online) {
              this.queryDaemonOverview(this.selectedDaemon);
              void this.tryNegotiateDirectLanWs(this.selectedDaemon);
            }
            // V3-32: Enroll online background daemons for OSC 7501 status notifications.
            this.enrollBackgroundDaemonsForNotifications();
            resolve(true);
          } else {
            failCandidate();
          }
          return;
        }

        await this.handleRelayFrame(frame);
      };

      const onDrop = () => {
        if (!settled) {
          failCandidate();
          return;
        }
        if (this.ws !== sock) return;
        this.ws = null;
        this.updateTransportFooterInPlace();
        if (this.relayKeepAliveTimer) {
          clearInterval(this.relayKeepAliveTimer);
          this.relayKeepAliveTimer = null;
        }
        if (this.token) {
          this.scheduleRelayReconnect();
        }
      };

      sock.onerror = onDrop;
      sock.onclose = onDrop;
    });
  }

  private resetPeerE2eeAndReattach(peerId: string) {
    const peer = this.devices.find((d) => d.id.toLowerCase() === peerId.toLowerCase());
    if (!peer) return;
    this.e2eeSessions.delete(peer.id);
    this.directE2eeSessions.delete(peer.id);
    if (this.directWsPeerId === peer.id && this.directWs) {
      try { this.directWs.close(); } catch {}
      this.directWs = null;
      this.directWsPeerId = null;
      this.activeTransportMode = "relay_ws";
    }
    if (this.selectedDaemon?.id === peer.id && peer.is_online) {
      this.queryDaemonOverview(peer, true);
      if (this.activeTerminalAddr) {
        const key = this.vtKey(peer.id, this.activeTerminalAddr);
        this.vtStreamActive.delete(key);
        this.attachAndReadTerminal(this.activeTerminalAddr);
      }
      if (
        this.splitMode &&
        this.secondaryTerminalAddr &&
        this.secondaryTerminalAddr !== this.activeTerminalAddr
      ) {
        const secKey = this.vtKey(peer.id, this.secondaryTerminalAddr);
        this.vtStreamActive.delete(secKey);
        this.attachAndReadTerminal(this.secondaryTerminalAddr);
      }
      void this.tryNegotiateDirectLanWs(peer);
    }
  }

  private async handleRelayFrame(frame: any) {
    if (frame.frame_type === "session_reset") {
      const srcId = String(frame.src_device_id || "");
      if (srcId) {
        this.resetPeerE2eeAndReattach(srcId);
      }
      return;
    }
    if (frame.frame_type === "peer_presence") {
      const devId = String(frame.device_id || "");
      const wasOnline = this.devices.find((d) => d.id === devId)?.is_online ?? false;
      if (Array.isArray(frame.endpoints)) {
        const idx = this.devices.findIndex((d) => d.id === devId);
        if (idx >= 0) {
          this.devices[idx].endpoints_json = frame.endpoints;
          this.devices[idx].is_online = Boolean(frame.is_online);
        }
      }
      try {
        await this.refreshDevices();
      } catch {
        // ignore transient refresh failure
      }
      if (!this.token) return;
      if (frame.is_online) {
        // Host reconnected or restarted: always clear stale E2EE session state and re-handshake/re-attach.
        if (!wasOnline || this.e2eeSessions.has(devId)) {
          this.resetPeerE2eeAndReattach(devId);
          if (this.selectedDaemon?.id !== devId) {
            this.enrollBackgroundDaemonsForNotifications(devId);
          }
        } else if (this.selectedDaemon?.is_online) {
          void this.tryNegotiateDirectLanWs(this.selectedDaemon);
        }
      } else if (this.directWsPeerId === devId) {
        this.directWs?.close();
      }
      if (!this.isTypingIntoInput() && !this.isAnyModalOpen()) {
        this.render();
      }
    } else if (frame.frame_type === "route_packet") {
      this.openRelayPacket(frame);
    }
  }

  private openRelayPacket(frame: any) {
    if (!this.myClientDevice || !frame?.e2ee) return;
    const srcId = String(frame.src_device_id || "").toLowerCase();
    const peer = this.devices.find((d) => d.id.toLowerCase() === srcId);
    if (!peer) return;

    try {
      const existing = this.e2eeSessions.get(peer.id);
      const pktNonce = frame.e2ee.session_nonce_b64 || null;
      const incomingAuth = frame.session_auth as RelaySessionAuth | undefined;

      if (
        existing &&
        existing.session.sessionNonceB64 === pktNonce &&
        (!incomingAuth || incomingAuth.session_nonce_b64 === existing.session.sessionNonceB64)
      ) {
        const msg = existing.session.openMessage(frame.e2ee);
        existing.confirmed = true;
        // V3-01: Drop duplicate Relay PTY output chunks once LAN Direct WS is active for this peer.
        if (
          msg?.op === "pty_output_chunk" &&
          this.activeTransportMode === "lan_ws" &&
          this.directWsPeerId === peer.id &&
          this.directWs?.readyState === WebSocket.OPEN
        ) {
          return;
        }
        this.handlePeerMessage(peer.id, msg);
        return;
      }

      if (incomingAuth && peer.ed25519_pubkey && peer.x25519_pubkey && pktNonce === incomingAuth.session_nonce_b64) {
        verifyRelaySessionAuth(
          incomingAuth,
          peer.id,
          this.myClientDevice.id,
          peer.ed25519_pubkey,
          peer.x25519_pubkey
        );
        this.seenRelayNonces.checkAndRemember(peer.id, incomingAuth.session_nonce_b64);
        const fresh = BrowserE2EESession.deriveRelayV2({
          myStaticSecretB64: this.webIdentity.x25519SecretB64,
          peerStaticPublicB64: peer.x25519_pubkey,
          sessionNonceB64: incomingAuth.session_nonce_b64,
          clientDeviceId: peer.id,
          daemonDeviceId: this.myClientDevice.id,
          isInitiator: false,
        });
        const msg = fresh.openMessage(frame.e2ee);
        this.e2eeSessions.set(peer.id, { session: fresh, auth: incomingAuth, confirmed: true });
        if (
          msg?.op === "pty_output_chunk" &&
          this.activeTransportMode === "lan_ws" &&
          this.directWsPeerId === peer.id &&
          this.directWs?.readyState === WebSocket.OPEN
        ) {
          return;
        }
        this.handlePeerMessage(peer.id, msg);
      }
    } catch {
      // drop unauthenticated, replayed, or tampered packet without poisoning active state
    }
  }

  private queryDaemonOverview(peer: Device, includeWorkspaces = true) {
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
    const now = Date.now();
    if (includeWorkspaces || now - this.lastWorkspacesQueryAt >= 30_000) {
      this.lastWorkspacesQueryAt = now;
      this.sendEncrypted(peer, {
        op: "list_workspaces",
        req_id: `ws-${now}`,
        root_dirs: [],
      });
    }
  }

  private scheduleDirectLanProbe(delayMs?: number) {
    if (this.directProbeTimer) {
      clearTimeout(this.directProbeTimer);
      this.directProbeTimer = null;
    }
    const peer = this.selectedDaemon;
    if (!peer || !peer.is_online || !this.token || !this.myClientDevice) return;
    if (this.activeTransportMode === "lan_ws" && this.directWsPeerId === peer.id && this.directWs?.readyState === WebSocket.OPEN) {
      return;
    }
    const wait = delayMs ?? this.smartPath.nextDelayMs(peer.id);
    this.directProbeTimer = setTimeout(() => {
      this.directProbeTimer = null;
      const currentPeer = this.selectedDaemon;
      if (
        currentPeer &&
        currentPeer.id === peer.id &&
        currentPeer.is_online &&
        this.activeTransportMode !== "lan_ws" &&
        this.smartPath.canProbeNow(currentPeer.id, this.activeTransportMode)
      ) {
        void this.tryNegotiateDirectLanWs(currentPeer);
      }
    }, wait);
  }

  private reattachOpenTerminalsPreserveScreen() {
    if (!this.selectedDaemon) return;
    const { cols, rows } = this.computeTerminalDimsFromDom();
    this.termCols = cols;
    this.termRows = rows;
    if (this.activeTerminalAddr) {
      this.sendEncrypted(this.selectedDaemon, {
        op: "pty_attach",
        req_id: this.nextReqId("hot"),
        terminal: this.activeTerminalAddr,
        cols,
        rows,
      });
    }
    if (
      this.splitMode &&
      this.secondaryTerminalAddr &&
      this.secondaryTerminalAddr !== this.activeTerminalAddr
    ) {
      this.sendEncrypted(this.selectedDaemon, {
        op: "pty_attach",
        req_id: this.nextReqId("hot2"),
        terminal: this.secondaryTerminalAddr,
        cols,
        rows,
      });
    }
  }

  private async tryNegotiateDirectLanWs(peer: Device, manual = false) {
    if (!this.token || !peer.is_online || !this.myClientDevice || !peer.ed25519_pubkey) return;
    if (this.directWs && this.directWs.readyState === WebSocket.OPEN && this.directWsPeerId === peer.id) {
      return;
    }
    if (!manual && !this.smartPath.canProbeNow(peer.id, this.activeTransportMode)) {
      this.scheduleDirectLanProbe();
      return;
    }
    if (this.negotiatingPeerId === peer.id) return;
    this.negotiatingPeerId = peer.id;
    const probeGen = this.directProbeGeneration;

    try {
      const candidates = collectDirectWsCandidates(peer, location);
      let upgraded = false;
      for (const wsUrl of candidates) {
        if (!this.token || probeGen !== this.directProbeGeneration || this.selectedDaemon?.id !== peer.id) {
          return;
        }
        const ok = await this.probeSingleDirectWs(peer, wsUrl, probeGen);
        if (ok) {
          if (!this.token || probeGen !== this.directProbeGeneration || this.selectedDaemon?.id !== peer.id) {
            return;
          }
          upgraded = true;
          this.smartPath.recordUpgradeSuccess(peer.id);
          if (this.directProbeTimer) {
            clearTimeout(this.directProbeTimer);
            this.directProbeTimer = null;
          }
          // V3-01: Detach old Relay PTY subscriptions BEFORE attaching on LAN Direct to prevent dual streaming.
          if (this.selectedDaemon?.id === peer.id) {
            if (this.activeTerminalAddr) {
              this.sendEncryptedViaRelay(peer, {
                op: "pty_detach",
                req_id: this.nextReqId("hdet"),
                terminal: this.activeTerminalAddr,
              });
            }
            if (
              this.splitMode &&
              this.secondaryTerminalAddr &&
              this.secondaryTerminalAddr !== this.activeTerminalAddr
            ) {
              this.sendEncryptedViaRelay(peer, {
                op: "pty_detach",
                req_id: this.nextReqId("hdet2"),
                terminal: this.secondaryTerminalAddr,
              });
            }
            this.reattachOpenTerminalsPreserveScreen();
          }
          this.sendEncrypted(peer, {
            op: "ping",
            req_id: `lan-p-${Date.now()}`,
            sent_ms: Date.now(),
          });
          this.consoleNotice = {
            kind: "ok",
            text: this.tr("Hot-switched seamlessly to LAN Direct", "已无感热切至局域网直连"),
          };
          this.updateConsoleNoticeInPlace();
          this.updateTransportFooterInPlace();
          break;
        }
      }
      const livePeer = this.devices.find((d) => d.id === peer.id);
      if (
        !upgraded &&
        this.token &&
        probeGen === this.directProbeGeneration &&
        this.selectedDaemon?.id === peer.id &&
        livePeer?.is_online
      ) {
        const plan = this.smartPath.recordProbeFailure(peer.id);
        this.scheduleDirectLanProbe(plan.delayMs);
        if (manual) {
          this.consoleNotice = {
            kind: "ok",
            text: this.tr(
              "LAN Direct unreachable on current network — staying on Smart Relay",
              "当前网络暂无法直连该主机，继续通过智能中继稳定传输"
            ),
          };
          if (!this.updateConsoleNoticeInPlace()) this.render();
        }
      }
    } finally {
      if (this.negotiatingPeerId === peer.id && probeGen === this.directProbeGeneration) {
        this.negotiatingPeerId = null;
      }
    }
  }

  private probeSingleDirectWs(peer: Device, wsUrl: string, probeGen: number): Promise<boolean> {
    return new Promise((resolve) => {
      if (
        !this.token ||
        probeGen !== this.directProbeGeneration ||
        this.selectedDaemon?.id !== peer.id ||
        !this.myClientDevice ||
        !peer.ed25519_pubkey ||
        !peer.x25519_pubkey
      ) {
        resolve(false);
        return;
      }
      const myDevId = this.myClientDevice.id;
      const expectedEd = peer.ed25519_pubkey;
      const expectedX = peer.x25519_pubkey;
      const eph = generateEphemeralX25519();

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
          sock.onopen = null;
          sock.onmessage = null;
          sock.onerror = null;
          sock.onclose = null;
          try { sock.close(); } catch {}
          done(false);
        }
      }, 1200);

      sock.onopen = () => {
        if (!this.token || probeGen !== this.directProbeGeneration || this.selectedDaemon?.id !== peer.id) {
          sock.onopen = null;
          sock.onmessage = null;
          sock.onerror = null;
          sock.onclose = null;
          try { sock.close(); } catch {}
          done(false);
          return;
        }
        const tsMs = Date.now();
        const signedMsg = p2pHelloV2SigningTextWithZstd(
          myDevId,
          peer.id,
          this.webIdentity.x25519PublicB64,
          eph.publicB64,
          tsMs,
          true
        );
        const sigB64 = signWebMessage(this.webIdentity, signedMsg);
        sock.send(
          JSON.stringify({
            frame_type: "direct_p2p_hello",
            client_device_id: myDevId,
            target_device_id: peer.id,
            client_ed25519_pub_b64: this.webIdentity.ed25519PublicB64,
            client_x25519_pub_b64: this.webIdentity.x25519PublicB64,
            client_ephemeral_x25519_pub_b64: eph.publicB64,
            supports_zstd: true,
            timestamp_ms: tsMs,
            signature_b64: sigB64,
          })
        );
      };

      sock.onmessage = (ev) => {
        let frame: any;
        try {
          frame = JSON.parse(String(ev.data));
        } catch {
          return;
        }

        if (!settled) {
          try {
            if (!this.token || probeGen !== this.directProbeGeneration || this.selectedDaemon?.id !== peer.id) {
              throw new Error("stale direct probe");
            }
            if (frame.frame_type !== "direct_p2p_ack" || !frame.ok) {
              throw new Error("rejected");
            }
            if (String(frame.daemon_device_id || "").toLowerCase() !== peer.id.toLowerCase()) {
              throw new Error("daemon_device_id mismatch");
            }
            if (frame.daemon_ed25519_pub_b64 !== expectedEd || frame.daemon_x25519_pub_b64 !== expectedX) {
              throw new Error("daemon pinned pubkey mismatch");
            }
            const daemonEph = String(frame.daemon_ephemeral_x25519_pub_b64 || "");
            const ackTs = Number(frame.timestamp_ms || 0);
            const ackSig = String(frame.signature_b64 || "");
            if (!daemonEph || !isTimestampFresh(ackTs)) {
              throw new Error("missing ephemeral pubkey or stale ack timestamp");
            }
            const ackSigning = p2pAckV2SigningText(myDevId, peer.id, eph.publicB64, daemonEph, ackTs);
            if (!verifyEd25519Signature(expectedEd, ackSigning, ackSig)) {
              throw new Error("DirectP2pAck Ed25519 signature invalid");
            }
            const directSess = BrowserE2EESession.deriveDirectP2Pv2({
              myStaticSecretB64: this.webIdentity.x25519SecretB64,
              peerStaticPublicB64: expectedX,
              myEphemeralSecret: eph.secret,
              peerEphemeralPublicB64: daemonEph,
              clientDeviceId: myDevId,
              daemonDeviceId: peer.id,
              isInitiator: true,
            });
            if (this.directWs && this.directWs !== sock) {
              this.directWs.onopen = null;
              this.directWs.onmessage = null;
              this.directWs.onerror = null;
              this.directWs.onclose = null;
              try { this.directWs.close(); } catch {}
            }
            this.directWs = sock;
            this.directWsPeerId = peer.id;
            this.directE2eeSessions.set(peer.id, directSess);
            this.activeTransportMode = "lan_ws";
            done(true);
          } catch {
            sock.onopen = null;
            sock.onmessage = null;
            sock.onerror = null;
            sock.onclose = null;
            try { sock.close(); } catch {}
            done(false);
          }
          return;
        }

        if (frame.frame_type === "route_packet") {
          const directSess = this.directE2eeSessions.get(peer.id);
          if (!directSess) return;
          try {
            const msg = directSess.openMessage(frame.e2ee);
            this.handlePeerMessage(peer.id, msg);
          } catch {
            // ignore malformed/replayed/tampered frame
          }
        }
      };

      const onDrop = () => {
        if (this.directWs === sock) {
          this.directWs = null;
          this.directWsPeerId = null;
          this.directE2eeSessions.delete(peer.id);
          this.activeTransportMode = "relay_ws";
          this.updateTransportFooterInPlace();
          const plan = this.smartPath.recordDirectDrop(peer.id);
          const livePeer = this.devices.find((d) => d.id === peer.id);
          // Seamless failover back to Smart Relay without clearing VT screen buffer.
          if (this.selectedDaemon?.id === peer.id && livePeer?.is_online && this.ws?.readyState === WebSocket.OPEN) {
            this.reattachOpenTerminalsPreserveScreen();
            this.consoleNotice = {
              kind: "ok",
              text: this.tr(
                "LAN Direct dropped — failed over seamlessly to Smart Relay",
                "局域网直连断开，已无缝切回智能中继"
              ),
            };
            this.updateConsoleNoticeInPlace();
          }
          if (this.selectedDaemon?.id === peer.id && livePeer?.is_online) {
            this.scheduleDirectLanProbe(plan.delayMs);
          }
        }
        done(false);
      };

      sock.onerror = onDrop;
      sock.onclose = onDrop;
    });
  }

  private getOrCreateRelaySessionState(peer: Device): {
    session: BrowserE2EESession;
    auth: RelaySessionAuth;
    confirmed: boolean;
  } {
    let state = this.e2eeSessions.get(peer.id);
    if (!state) {
      if (!this.myClientDevice) {
        throw new Error("missing client device identity");
      }
      const auth = createRelaySessionAuth(this.webIdentity, this.myClientDevice.id, peer.id);
      const session = BrowserE2EESession.deriveRelayV2({
        myStaticSecretB64: this.webIdentity.x25519SecretB64,
        peerStaticPublicB64: peer.x25519_pubkey,
        sessionNonceB64: auth.session_nonce_b64,
        clientDeviceId: this.myClientDevice.id,
        daemonDeviceId: peer.id,
        isInitiator: true,
      });
      state = { session, auth, confirmed: false };
      this.e2eeSessions.set(peer.id, state);
    }
    return state;
  }

  private sendEncryptedViaRelay(peer: Device, msg: unknown) {
    if (!this.myClientDevice || !this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    const state = this.getOrCreateRelaySessionState(peer);
    const e2ee = state.session.sealMessage(msg);
    this.ws.send(
      JSON.stringify({
        frame_type: "route_packet",
        src_device_id: this.myClientDevice.id,
        dst_device_id: peer.id,
        lane: "control_p0",
        e2ee,
        sender_x25519_pub_b64: this.webIdentity.x25519PublicB64,
        session_auth: state.confirmed ? undefined : state.auth,
      })
    );
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
          })
        );
        return;
      }
    }

    this.sendEncryptedViaRelay(peer, msg);
  }

  private focusNotifiedTerminal(daemonId: string, terminalAddr: string) {
    const peer = this.devices.find((d) => d.id === daemonId);
    if (!peer) return;
    if (this.selectedDaemon?.id !== peer.id) {
      this.selectDaemon(peer, { initialTerminalAddr: terminalAddr });
    } else {
      if (this.splitMode && this.secondaryTerminalAddr === terminalAddr) {
        this.focusedPane = "secondary";
      } else {
        if (this.activeTerminalAddr && this.activeTerminalAddr !== terminalAddr) {
          this.cancelActiveVoiceCapture();
          this.detachActiveTerminal();
        }
        this.activeTerminalAddr = terminalAddr;
        this.focusedPane = "primary";
        writeLocalPref("coda_last_terminal_addr", terminalAddr);
      }
    }
    if (this.route !== "console") {
      this.navigate("/console");
    } else {
      this.render();
    }
    if (this.selectedDaemon?.id === daemonId) {
      this.attachAndReadTerminal(terminalAddr);
    }
  }

  private observeTerminalStatusNotification(peerId: string, term: HerdrTerminal) {
    const peer = this.devices.find((d) => d.id === peerId);
    const payload = this.notifyTracker.observe(
      {
        daemonId: peerId,
        daemonName: peer?.name || "Host",
        terminalAddr: term.address,
        terminalName: term.name,
        status: term.status,
      },
      {
        activeDaemonId: this.selectedDaemon?.id ?? null,
        activeTerminalAddr: this.activeTerminalAddr,
        documentHidden: typeof document !== "undefined" ? Boolean(document.hidden) : false,
        locale: this.locale,
      }
    );
    if (!payload) return;
    if (this.notificationsEnabled) {
      dispatchBrowserNotification(payload, () => {
        this.focusNotifiedTerminal(payload.daemonId, payload.terminalAddr);
      });
    }
  }

  private handlePeerMessage(peerId: string, msg: any) {
    const isSelected = this.selectedDaemon?.id === peerId;

    if (msg.op === "pong") {
      if (!isSelected) return;
      this.rttMs = Math.max(1, Date.now() - Number(msg.sent_ms || Date.now()));
      this.updateTransportFooterInPlace();
      return;
    }

    if (msg.op === "workspaces_snapshot") {
      if (msg.host_health) {
        this.hostHealthByPeer.set(peerId, msg.host_health as HostHealthSnapshot);
      }
      if (!isSelected) return;
      this.repos = msg.repos || [];
      if (msg.voice_config) {
        const incomingKey = String(msg.voice_config.api_key || "");
        this.voiceConfig = {
          provider: msg.voice_config.provider || "openai",
          base_url: msg.voice_config.base_url || "https://api.openai.com/v1",
          api_key: this.voiceApiKeyLocal || incomingKey,
          model: msg.voice_config.model || "gpt-4o-mini-transcribe",
        };
      }
      if (!this.isTypingIntoInput() && !this.isAnyModalOpen()) this.render();
      return;
    }

    if (msg.op === "host_health_result") {
      if (msg.health) {
        this.hostHealthByPeer.set(peerId, msg.health as HostHealthSnapshot);
        if (!this.isTypingIntoInput() && !this.isAnyModalOpen()) this.render();
      }
      return;
    }

    if (msg.op === "upgrade_daemon_result") {
      const peerDev = this.devices.find((d) => d.id === peerId);
      const label = peerDev?.name || "Host";
      const summary = msg.ok
        ? `${label}: v${msg.previous_version} -> v${msg.target_version || "latest"} (${msg.message})`
        : `${label} OTA failed: ${msg.message || "error"}`;
      this.consoleNotice = { kind: msg.ok ? "ok" : "err", text: summary };
      if (this.showSettingsModal) {
        this.settingsFeedback = { kind: msg.ok ? "ok" : "err", text: summary };
      }
      if (msg.ok) {
        this.e2eeSessions.delete(peerId);
        this.directE2eeSessions.delete(peerId);
        if (this.directWsPeerId === peerId) {
          this.teardownDirectWs();
        }
        setTimeout(() => {
          const live = this.devices.find((d) => d.id === peerId);
          if (live && this.token) {
            this.queryDaemonOverview(live, true);
          }
        }, 1600);
      }
      this.render();
      return;
    }

    if (msg.op === "directory_listing") {
      if (!isSelected) return;
      if (String(msg.req_id || "").startsWith("spdir-")) {
        if (
          !this.showSpawnModal ||
          !this.spawnDirPicker.open ||
          !this.spawnDirPicker.reqId ||
          msg.req_id !== this.spawnDirPicker.reqId
        ) {
          return;
        }
        this.captureSpawnFormDraftFromDom();
        this.spawnDirPicker.loading = false;
        this.spawnDirPicker.rootPath = String(msg.root_path || this.spawnDirPicker.rootPath);
        this.spawnDirPicker.subPath = String(msg.sub_path || "");
        const allEntries: RemoteFileEntry[] = Array.isArray(msg.entries) ? msg.entries : [];
        this.spawnDirPicker.entries = allEntries.filter((e) => e.is_dir);
        this.spawnDirPicker.error = msg.error ? String(msg.error) : null;
        this.render();
        return;
      }
      if (
        !this.fileBrowser.open ||
        !this.fileBrowser.dirReqId ||
        msg.req_id !== this.fileBrowser.dirReqId
      ) {
        return;
      }
      this.fileBrowser.dirReqId = null;
      if (!this.fileBrowser.previewReqId) {
        this.fileBrowser.loading = false;
      }
      this.fileBrowser.rootPath = String(msg.root_path || this.fileBrowser.rootPath);
      this.fileBrowser.subPath = String(msg.sub_path || "");
      this.fileBrowser.entries = Array.isArray(msg.entries) ? msg.entries : [];
      if (msg.error) {
        this.fileBrowser.error = String(msg.error);
      } else if (!this.fileBrowser.previewReqId) {
        this.fileBrowser.error = null;
      }
      this.render();
      return;
    }

    if (msg.op === "git_diff_response") {
      if (
        !isSelected ||
        !this.gitDiff.open ||
        !this.gitDiff.reqId ||
        msg.req_id !== this.gitDiff.reqId
      ) {
        return;
      }
      this.gitDiff.loading = false;
      this.gitDiff.isGitRepo = msg.is_git_repo !== false;
      this.gitDiff.repoRoot = msg.repo_root ? String(msg.repo_root) : null;
      this.gitDiff.branch = String(msg.branch || "");
      this.gitDiff.statusShort = String(msg.status_short || "");
      this.gitDiff.diffUnified = String(msg.diff_unified || "");
      this.gitDiff.files = parseGitStatusShort(this.gitDiff.statusShort);
      if (
        this.gitDiff.selectedFile &&
        !this.gitDiff.files.some((f) => f.path === this.gitDiff.selectedFile)
      ) {
        this.gitDiff.selectedFile = null;
      }
      if (msg.error) {
        this.gitDiff.error = String(msg.error);
      } else if (!this.gitDiff.isGitRepo) {
        this.gitDiff.error = this.tr(
          `Directory "${this.gitDiff.cwd}" is not inside a Git repository.`,
          `目录「${this.gitDiff.cwd}」不是有效的 Git 仓库。`
        );
      } else {
        this.gitDiff.error = null;
      }
      this.render();
      return;
    }

    if (msg.op === "bootstrap_workspace_result") {
      if (!this.spawnBootstrapReqId || msg.req_id !== this.spawnBootstrapReqId) return;
      const matchesPeer = !this.spawnBootstrapPeerId || this.spawnBootstrapPeerId === peerId;
      this.spawnBootstrapReqId = null;
      this.spawnBootstrapPeerId = null;
      this.spawnBootstrapLoading = false;
      if (!matchesPeer) return;
      if (isSelected) {
        this.captureSpawnFormDraftFromDom();
      }
      let toastSummary = "";
      if (msg.ok && msg.workspace_path) {
        toastSummary = String(
          msg.detail ||
            this.tr(
              `Workspace ready: ${msg.workspace_path}`,
              `工作区已就绪：${msg.workspace_path}`
            )
        );
        if (isSelected) {
          this.spawnCwd = String(msg.workspace_path);
          this.spawnBootstrapMsg = { kind: "ok", text: toastSummary };
          if (this.selectedDaemon?.is_online) {
            this.sendEncrypted(this.selectedDaemon, {
              op: "list_workspaces",
              req_id: this.nextReqId("ws"),
              root_dirs: [],
            });
          }
        }
      } else {
        toastSummary = String(
          msg.detail || this.tr("Failed bootstrapping workspace", "初始化工作区失败")
        );
        if (isSelected) {
          this.spawnBootstrapMsg = { kind: "err", text: toastSummary };
        }
      }
      this.showToastNotice(msg.ok ? "ok" : "err", toastSummary);
      if (this.showSpawnModal && isSelected) {
        this.render();
      }
      return;
    }

    if (msg.op === "file_preview_response") {
      if (
        !isSelected ||
        !this.fileBrowser.open ||
        !this.fileBrowser.previewReqId ||
        msg.req_id !== this.fileBrowser.previewReqId
      ) {
        return;
      }
      this.fileBrowser.previewReqId = null;
      if (!this.fileBrowser.dirReqId) {
        this.fileBrowser.loading = false;
      }
      if (msg.error) {
        this.fileBrowser.error = String(msg.error);
      } else {
        this.fileBrowser.error = null;
        this.fileBrowser.previewContent = String(msg.content || "");
        this.fileBrowser.previewTruncated = Boolean(msg.truncated);
      }
      this.render();
      return;
    }

    if (msg.op === "http_local_preview_response") {
      if (
        !isSelected ||
        !this.portPreview.open ||
        !this.portPreview.reqId ||
        msg.req_id !== this.portPreview.reqId
      ) {
        return;
      }
      this.portPreview.reqId = null;
      this.portPreview.loading = false;
      this.portPreview.error = null;
      this.portPreview.statusCode = Number(msg.status_code || 0);
      this.portPreview.contentType = String(msg.content_type || "text/plain");
      this.portPreview.headers = Array.isArray(msg.headers) ? msg.headers : [];
      const decoded = decodePortPreviewPayload(
        String(msg.body_b64 || ""),
        this.portPreview.contentType,
        Boolean(msg.truncated)
      );
      this.portPreview.bodyText = decoded.bodyText;
      this.portPreview.rawHtml = decoded.rawHtml;
      this.portPreview.truncated = decoded.truncated;
      this.render();
      return;
    }

    if (msg.op === "transcribe_audio_result") {
      if (
        !isSelected ||
        !this.pendingTranscribeReqId ||
        msg.req_id !== this.pendingTranscribeReqId
      ) {
        return;
      }
      this.pendingTranscribeReqId = null;
      if (this.pendingTranscribeTimer) {
        clearTimeout(this.pendingTranscribeTimer);
        this.pendingTranscribeTimer = null;
      }
      this.isTranscribingVoice = false;
      if (msg.ok && String(msg.text || "").trim()) {
        this.insertTranscribedText(String(msg.text));
      } else {
        this.syncVoiceButtonInPlace();
        this.showToastNotice(
          "err",
          String(
            msg.error ||
              this.tr("Empty or failed voice transcription on host.", "远端主机未识别到有效语音内容。")
          )
        );
      }
      return;
    }

    if (msg.op === "voice_config_ack" && msg.config) {
      if (!isSelected) return;
      this.voiceConfig = {
        ...msg.config,
        api_key: this.voiceApiKeyLocal || msg.config.api_key || "",
      };
      this.consoleNotice = {
        kind: msg.ok ? "ok" : "err",
        text: msg.ok
          ? this.tr("Saved voice model config on host.", "已同步语音模型配置至主机。")
          : this.tr("Failed saving voice model config.", "保存语音模型配置失败。"),
      };
      this.render();
      return;
    }

    if (msg.op === "terminals_snapshot") {
      const incomingTerms: HerdrTerminal[] = msg.terminals || [];
      const prevPeerTerms = this.terminalsByPeer.get(peerId) || [];
      const prevNeedsCount = prevPeerTerms.filter((t) => t.alive && this.isNeedsYouStatus(t.status)).length;
      const nextNeedsCount = incomingTerms.filter((t) => t.alive && this.isNeedsYouStatus(t.status)).length;
      this.terminalsByPeer.set(peerId, incomingTerms);

      if (!isSelected) {
        this.pruneStaleVtTerminalsForPeer(peerId, incomingTerms);
        // V3-08 / V3-32: Observe notifications for background daemons without polluting active daemon UI.
        for (const term of incomingTerms) {
          this.syncVtAgentDefaultMode(peerId, term);
          this.observeTerminalStatusNotification(peerId, term);
        }
        if (
          prevNeedsCount !== nextNeedsCount &&
          this.route === "console" &&
          !this.isTypingIntoInput() &&
          !this.isAnyModalOpen()
        ) {
          this.render();
        }
        return;
      }
      this.terminals = incomingTerms;
      if (this.activeTerminalAddr && !this.terminals.some((t) => t.address === this.activeTerminalAddr)) {
        this.activeTerminalAddr = null;
      }
      if (this.secondaryTerminalAddr && !this.terminals.some((t) => t.address === this.secondaryTerminalAddr)) {
        this.secondaryTerminalAddr = null;
        if (this.focusedPane === "secondary") {
          this.focusedPane = "primary";
        }
      }
      if (this.splitMode && !this.secondaryTerminalAddr && this.terminals.length > 1) {
        const alt = this.terminals.find((t) => t.address !== this.activeTerminalAddr);
        if (alt) {
          this.secondaryTerminalAddr = alt.address;
          this.attachAndReadTerminal(alt.address);
        }
      }
      let newlyRestoredAddr: string | null = null;
      if (!this.activeTerminalAddr && this.terminals.length > 0) {
        const savedTerm = readLocalPref("coda_last_terminal_addr", "");
        const restored = savedTerm ? this.terminals.find((t) => t.address === savedTerm) : undefined;
        this.activeTerminalAddr = restored ? restored.address : this.terminals[0].address;
        writeLocalPref("coda_last_terminal_addr", this.activeTerminalAddr);
        newlyRestoredAddr = this.activeTerminalAddr;
      }
      this.pruneStaleVtTerminalsForPeer(peerId, this.terminals);
      // V3-32: Observe notifications AFTER activeTerminalAddr is restored so switching back does not false-alert.
      for (const term of this.terminals) {
        this.syncVtAgentDefaultMode(peerId, term);
        this.observeTerminalStatusNotification(peerId, term);
      }
      if (newlyRestoredAddr) {
        this.attachAndReadTerminal(newlyRestoredAddr);
      }
      if (!this.isTypingIntoInput() && !this.isAnyModalOpen()) this.render();
      return;
    }

    if (msg.op === "launch_agent_result") {
      if (!isSelected) return;
      if (msg.ok && msg.terminal) {
        this.detachActiveTerminal();
        this.terminals = [msg.terminal, ...this.terminals.filter((t) => t.address !== msg.terminal.address)];
        this.terminalsByPeer.set(peerId, this.terminals);
        this.activeTerminalAddr = msg.terminal.address;
        writeLocalPref("coda_last_terminal_addr", msg.terminal.address);
        this.render();
        setTimeout(() => {
          if (this.selectedDaemon?.id === peerId && this.activeTerminalAddr === msg.terminal.address) {
            this.attachAndReadTerminal(msg.terminal.address);
          }
        }, 180);
      } else if (!msg.ok) {
        this.showToastNotice("err", msg.error || this.tr("Failed spawning terminal", "新建终端失败"));
      }
      return;
    }

    if (msg.op === "terminal_screen_snapshot") {
      if (!isSelected) return;
      const addr = String(msg.terminal || this.activeTerminalAddr || "");
      if (!addr) return;
      const key = this.vtKey(peerId, addr);
      if (!this.vtStreamActive.has(key)) {
        const vt = this.getOrCreateVt(peerId, addr);
        vt.loadSnapshot(msg.text || "");
        this.vtStreamActive.add(key);
        if (msg.status) {
          const term = this.terminals.find((t) => t.address === addr);
          if (term) {
            term.status = msg.status;
            this.terminalsByPeer.set(peerId, [...this.terminals]);
            this.observeTerminalStatusNotification(peerId, term);
          }
        }
        if (addr === this.activeTerminalAddr || (this.splitMode && addr === this.secondaryTerminalAddr)) {
          this.scheduleTerminalPaint();
        }
      }
      return;
    }

    if (msg.op === "pty_output_chunk") {
      const addr = String(msg.terminal || "");
      if (!addr) return;
      try {
        const key = this.vtKey(peerId, addr);
        const vt = this.getOrCreateVt(peerId, addr);
        this.vtStreamActive.add(key);
        vt.feedBytes(fromB64(String(msg.data_b64 || "")));
        // V3-10: Consume ONLY newly parsed OSC 7501 status so cleared NeedsYou never resurrects on plain output.
        const freshStatus = vt.consumeLatestStatus();
        if (freshStatus && isSelected) {
          const term = this.terminals.find((t) => t.address === addr);
          if (term) {
            const wasNeedsYou = this.isNeedsYouStatus(term.status);
            term.status = {
              state: freshStatus.state,
              kind: freshStatus.kind || null,
              app: freshStatus.app || null,
              title: freshStatus.title || null,
              msg: freshStatus.msg || null,
            };
            const nowNeedsYou = this.isNeedsYouStatus(term.status);
            this.terminalsByPeer.set(peerId, [...this.terminals]);
            this.observeTerminalStatusNotification(peerId, term);
            if (
              wasNeedsYou !== nowNeedsYou &&
              this.route === "console" &&
              this.showSidebar &&
              !this.isTypingIntoInput() &&
              !this.showSettingsModal &&
              !this.showSpawnModal
            ) {
              this.render();
            }
          }
        }
        if (isSelected && (addr === this.activeTerminalAddr || (this.splitMode && addr === this.secondaryTerminalAddr))) {
          this.scheduleTerminalPaint();
        }
      } catch {
        // ignore malformed chunk
      }
      return;
    }

    if (msg.op === "herdr_live_event") {
      if (msg.terminal) {
        const peerList = [...(this.terminalsByPeer.get(peerId) || [])];
        const pIdx = peerList.findIndex((t) => t.address === msg.terminal.address);
        const wasNeedsYou = pIdx >= 0 ? this.isNeedsYouStatus(peerList[pIdx].status) : false;
        const nowNeedsYou = this.isNeedsYouStatus(msg.terminal.status);
        if (pIdx >= 0) {
          peerList[pIdx] = msg.terminal;
        } else if (msg.terminal.alive) {
          peerList.unshift(msg.terminal);
        }
        this.terminalsByPeer.set(
          peerId,
          peerList.filter(
            (t) =>
              t.alive ||
              (isSelected &&
                (t.address === this.activeTerminalAddr || t.address === this.secondaryTerminalAddr))
          )
        );
        if (!msg.terminal.alive) {
          const isOpenHere =
            isSelected &&
            (msg.terminal.address === this.activeTerminalAddr ||
              msg.terminal.address === this.secondaryTerminalAddr);
          if (!isOpenHere) {
            const deadKey = this.vtKey(peerId, msg.terminal.address);
            this.vtTerminals.delete(deadKey);
            this.vtStreamActive.delete(deadKey);
          }
        }

        if (isSelected) {
          const idx = this.terminals.findIndex((t) => t.address === msg.terminal.address);
          if (idx >= 0) {
            this.terminals[idx] = msg.terminal;
          } else if (msg.terminal.alive) {
            this.terminals.unshift(msg.terminal);
          }
          this.syncVtAgentDefaultMode(peerId, msg.terminal);
        }
        this.observeTerminalStatusNotification(peerId, msg.terminal);
        if (
          wasNeedsYou !== nowNeedsYou &&
          this.route === "console" &&
          !this.isTypingIntoInput() &&
          !this.isAnyModalOpen()
        ) {
          this.render();
        } else if (
          isSelected &&
          (msg.terminal.address === this.activeTerminalAddr ||
            (this.splitMode && msg.terminal.address === this.secondaryTerminalAddr))
        ) {
          this.scheduleTerminalPaint();
        }
      }
      return;
    }

    if (msg.op === "upload_terminal_file_result") {
      const reqId = String(msg.req_id || "");
      const meta = reqId ? this.pendingUploadTargets.get(reqId) : undefined;
      if (reqId) {
        this.pendingUploadTargets.delete(reqId);
      }
      const peerDev = this.devices.find((d) => d.id === peerId);
      if (msg.ok && msg.remote_path) {
        const escapedForPty = formatUploadedPathForPty(String(msg.remote_path));
        if (meta?.target === "draft") {
          if (isSelected && meta.daemonId === peerId) {
            const sep = this.draftInput && !/\s$/.test(this.draftInput) ? " " : "";
            this.draftInput = `${this.draftInput}${sep}${escapedForPty}`;
            this.updateDraftPreviewInPlace();
            const composerArea = document.getElementById(
              "prompt-composer-textarea"
            ) as HTMLTextAreaElement | null;
            const desktopInp = document.getElementById(
              "desktop-prompt-input"
            ) as HTMLInputElement | null;
            const kbInp = document.getElementById("kb-input") as HTMLInputElement | null;
            if (composerArea) {
              composerArea.value = this.draftInput;
              composerArea.focus();
            } else if (desktopInp) {
              desktopInp.value = this.draftInput;
              desktopInp.focus();
            } else if (kbInp) {
              kbInp.value = this.draftInput;
              kbInp.focus();
            }
            this.showToastNotice(
              "ok",
              this.tr(
                `Uploaded & attached to prompt draft: ${msg.remote_path}`,
                `已加密上传并插入待发送草稿：${msg.remote_path}`
              )
            );
          }
          return;
        }
        const targetTerminal = String(msg.terminal || meta?.terminalAddr || this.activeTerminalAddr || "");
        if (peerDev && targetTerminal) {
          this.sendEncrypted(peerDev, {
            op: "pty_input",
            terminal: targetTerminal,
            data_b64: toB64(new TextEncoder().encode(escapedForPty)),
          });
        }
        if (isSelected) {
          this.showToastNotice(
            "ok",
            this.tr(
              `Uploaded & inserted into terminal: ${msg.remote_path}`,
              `已加密上传并自动输入终端：${msg.remote_path}`
            )
          );
        }
      } else if (isSelected) {
        this.showToastNotice("err", msg.error || this.tr("Upload failed", "上传失败"));
      }
      return;
    }

    if (msg.op === "action_ack" && !msg.ok) {
      if (!isSelected) return;
      this.showToastNotice("err", msg.detail || this.tr("Host action failed", "远端操作失败"));
    }
  }

  private detachActiveTerminal() {
    if (!this.selectedDaemon || !this.activeTerminalAddr) return;
    this.sendEncrypted(this.selectedDaemon, {
      op: "pty_detach",
      req_id: this.nextReqId("det"),
      terminal: this.activeTerminalAddr,
    });
  }

  private detachAllOpenTerminals() {
    if (!this.selectedDaemon) return;
    if (this.activeTerminalAddr) {
      this.sendEncrypted(this.selectedDaemon, {
        op: "pty_detach",
        req_id: this.nextReqId("det"),
        terminal: this.activeTerminalAddr,
      });
    }
    if (
      this.secondaryTerminalAddr &&
      this.secondaryTerminalAddr !== this.activeTerminalAddr
    ) {
      this.sendEncrypted(this.selectedDaemon, {
        op: "pty_detach",
        req_id: this.nextReqId("det2"),
        terminal: this.secondaryTerminalAddr,
      });
    }
  }

  private attachAndReadTerminal(addr: string) {
    if (!this.selectedDaemon) return;
    const { cols, rows } = this.computeTerminalDimsFromDom();
    this.termCols = cols;
    this.termRows = rows;
    const vt = this.getOrCreateVt(this.selectedDaemon.id, addr);
    vt.resize(cols, rows);
    const key = this.vtKey(this.selectedDaemon.id, addr);
    if (!this.vtStreamActive.has(key)) {
      this.requestScreen(addr);
    }
    this.sendEncrypted(this.selectedDaemon, {
      op: "pty_attach",
      req_id: `pty-${Date.now()}`,
      terminal: addr,
      cols,
      rows,
    });
  }

  private requestScreen(addr: string) {
    if (!this.selectedDaemon) return;
    this.sendEncrypted(this.selectedDaemon, {
      op: "read_terminal_screen",
      req_id: `rd-${Date.now()}`,
      terminal: addr,
      history_lines: 120,
    });
  }

  private applySearchHighlightsInPlace(screenEl: HTMLElement, vt: WebVtTerminal) {
    const q = this.showTermSearch ? this.termSearchQuery.trim() : "";
    const children = screenEl.children;
    if (!q) {
      for (let i = 0; i < children.length; i++) {
        children[i].classList.remove("vt-search-hit", "vt-search-active");
      }
      return;
    }
    const matches = findWebTerminalMatches(vt.renderPlainText(), q);
    const activeMatch = matches.length > 0 ? matches[this.termSearchIdx % matches.length] : null;
    const hitRows = new Set(matches.map((m) => m.rowIdx));
    for (let i = 0; i < children.length; i++) {
      const el = children[i];
      const isHit = hitRows.has(i);
      const isActive = Boolean(activeMatch && activeMatch.rowIdx === i);
      el.classList.toggle("vt-search-hit", isHit && !isActive);
      el.classList.toggle("vt-search-active", isActive);
    }
    const cntEl = document.getElementById("term-search-count");
    if (cntEl) {
      cntEl.textContent =
        matches.length === 0 ? "0/0" : `${(this.termSearchIdx % matches.length) + 1}/${matches.length}`;
    }
  }

  private stepWebTerminalSearch(forward: boolean) {
    if (!this.selectedDaemon || !this.activeTerminalAddr) return;
    const vt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, this.activeTerminalAddr));
    const screenEl = document.getElementById("term-screen-output");
    if (!vt || !screenEl) return;
    const matches = findWebTerminalMatches(vt.renderPlainText(), this.termSearchQuery);
    if (matches.length === 0) {
      this.termSearchIdx = 0;
      this.applySearchHighlightsInPlace(screenEl, vt);
      return;
    }
    const cur = this.termSearchIdx % matches.length;
    this.termSearchIdx = forward
      ? (cur + 1) % matches.length
      : (cur + matches.length - 1) % matches.length;
    this.applySearchHighlightsInPlace(screenEl, vt);
    const targetRow = screenEl.children[matches[this.termSearchIdx].rowIdx] as HTMLElement | undefined;
    targetRow?.scrollIntoView({ block: "center", behavior: "smooth" });
  }

  private exportActiveSessionMarkdown() {
    if (!this.selectedDaemon || !this.activeTerminalAddr) return;
    const term = this.terminals.find((t) => t.address === this.activeTerminalAddr);
    const vt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, this.activeTerminalAddr));
    const plain = vt ? vt.renderPlainText() : "";
    const title = term?.status?.title || term?.name || this.activeTerminalAddr;
    const md = buildWebSessionMarkdown(title, term?.cwd || "~", plain);
    triggerMarkdownDownload(title, md);
    this.showToastNotice("ok", this.tr("Exported session transcript (.md)", "已导出当前终端会话日志 (.md)"));
  }

  private toggleSplitView() {
    if (typeof window !== "undefined" && window.innerWidth < 900) {
      this.showToastNotice(
        "err",
        this.tr(
          "Split view requires a desktop or tablet viewport (≥ 900px).",
          "双栏分屏需要桌面或平板宽屏视口（宽度 ≥ 900px）。"
        )
      );
      return;
    }
    this.toggleSplitTerminals();
    this.render();
    this.scheduleTerminalResizeSync();
  }

  private toggleSplitTerminals() {
    this.splitMode = !this.splitMode;
    if (this.splitMode) {
      const alt = this.terminals.find((t) => t.address !== this.activeTerminalAddr);
      this.secondaryTerminalAddr = alt ? alt.address : null;
      this.focusedPane = "primary";
      if (this.secondaryTerminalAddr) {
        this.attachAndReadTerminal(this.secondaryTerminalAddr);
      }
    } else {
      this.focusedPane = "primary";
      if (this.secondaryTerminalAddr && this.selectedDaemon) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "pty_detach",
          req_id: this.nextReqId("det2"),
          terminal: this.secondaryTerminalAddr,
        });
        this.secondaryTerminalAddr = null;
      }
    }
  }

  private openRemoteGitDiff(cwdOverride?: string) {
    if (!this.selectedDaemon?.is_online) {
      this.showToastNotice("err", this.tr("Select an online host first.", "请先选择一台在线主机。"));
      return;
    }
    const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
    const targetCwd =
      cwdOverride ||
      activeTerm?.cwd ||
      this.repos[0]?.root_path ||
      ".";
    const reqId = this.nextReqId("gd");
    this.gitDiff = {
      open: true,
      cwd: targetCwd,
      branch: "",
      statusShort: "",
      diffUnified: "",
      files: [],
      selectedFile: null,
      isGitRepo: true,
      repoRoot: null,
      loading: true,
      error: null,
      reqId,
    };
    this.sendEncrypted(this.selectedDaemon, {
      op: "git_diff_request",
      req_id: reqId,
      cwd: targetCwd,
    });
    this.render();
  }

  private refreshRemoteGitDiff() {
    if (!this.selectedDaemon?.is_online || !this.gitDiff.open) return;
    const reqId = this.nextReqId("gd");
    this.gitDiff.loading = true;
    this.gitDiff.error = null;
    this.gitDiff.reqId = reqId;
    this.sendEncrypted(this.selectedDaemon, {
      op: "git_diff_request",
      req_id: reqId,
      cwd: this.gitDiff.cwd || ".",
    });
    this.render();
  }

  private openFileFromGitDiff(relPath: string) {
    if (!this.selectedDaemon?.is_online || !relPath) return;
    const baseRoot = resolveGitDiffFileRoot(this.gitDiff.repoRoot, this.gitDiff.cwd);
    const cleanRel = relPath.replace(/^\/+/, "");
    const containingSubPath = parentSubPath(cleanRel);
    const dirReqId = this.nextReqId("lsdir");
    const previewReqId = this.nextReqId("rfp");
    this.gitDiff.open = false;
    this.fileBrowser = {
      open: true,
      rootPath: baseRoot,
      subPath: containingSubPath,
      entries: [],
      selectedRelPath: cleanRel,
      previewContent: "",
      previewTruncated: false,
      loading: true,
      error: null,
      dirReqId,
      previewReqId,
    };
    this.sendEncrypted(this.selectedDaemon, {
      op: "list_directory",
      req_id: dirReqId,
      root_path: baseRoot,
      sub_path: containingSubPath,
    });
    this.sendEncrypted(this.selectedDaemon, {
      op: "read_file_preview",
      req_id: previewReqId,
      path: joinRootAndRel(baseRoot, cleanRel),
      max_bytes: 65536,
    });
    this.render();
  }

  private captureSpawnFormDraftFromDom() {
    if (typeof document === "undefined") return;
    const form = document.getElementById("spawn-agent-form") as HTMLFormElement | null;
    if (!form) return;
    const agEl = form.querySelector<HTMLSelectElement>('select[name="agent"]');
    const cwdEl = form.querySelector<HTMLInputElement>('input[name="cwd"]');
    const promptEl = form.querySelector<HTMLInputElement>('input[name="initial_prompt"]');
    if (agEl) this.spawnAgentType = agEl.value;
    if (cwdEl) this.spawnCwd = cwdEl.value;
    if (promptEl) this.spawnInitialPrompt = promptEl.value;
    const pDirEl = document.getElementById("spawn-boot-parent") as HTMLInputElement | null;
    const fNameEl = document.getElementById("spawn-boot-folder") as HTMLInputElement | null;
    const gitUrlEl = document.getElementById("spawn-boot-git-url") as HTMLInputElement | null;
    if (pDirEl) this.spawnBootstrapParentDir = pDirEl.value;
    if (fNameEl) this.spawnBootstrapFolderName = fNameEl.value;
    if (gitUrlEl) this.spawnBootstrapGitUrl = gitUrlEl.value;
  }

  private openNewAgentModal() {
    if (!this.selectedDaemon?.is_online) {
      this.showToastNotice("err", this.tr("Select an online host first.", "请先选择一台在线主机。"));
      return;
    }
    const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
    const defaultCwd = activeTerm?.cwd || this.repos[0]?.root_path || "";
    this.spawnAgentType = "shell";
    this.spawnCwd = defaultCwd;
    this.spawnInitialPrompt = "";
    this.spawnDirPicker = {
      open: false,
      rootPath: "",
      subPath: "",
      entries: [],
      loading: false,
      error: null,
      reqId: null,
    };
    this.spawnBootstrapOpen = false;
    this.spawnBootstrapParentDir =
      defaultCwd && defaultCwd.startsWith("/") ? parentAbsolutePath(defaultCwd) : "~";
    this.spawnBootstrapFolderName = "";
    this.spawnBootstrapGitUrl = "";
    this.bootFolderManuallyEdited = false;
    this.spawnBootstrapLoading = false;
    this.spawnBootstrapMsg = null;
    this.showSpawnModal = true;
    this.render();
  }

  private openSpawnDirectoryPicker(rootOverride?: string) {
    if (!this.selectedDaemon?.is_online) return;
    this.captureSpawnFormDraftFromDom();
    const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
    const candidate = resolveSpawnPickerInitialRoot({
      rootOverride,
      spawnCwd: this.spawnCwd,
      activeTermCwd: activeTerm?.cwd,
      firstRepoRoot: this.repos[0]?.root_path,
    });
    const reqId = this.nextReqId("spdir");
    this.spawnDirPicker = {
      open: true,
      rootPath: candidate,
      subPath: "",
      entries: [],
      loading: true,
      error: null,
      reqId,
    };
    this.sendEncrypted(this.selectedDaemon, {
      op: "list_directory",
      req_id: reqId,
      root_path: candidate,
      sub_path: "",
    });
    this.render();
  }

  private browseSpawnDirSubPath(subPath: string) {
    if (!this.selectedDaemon?.is_online || !this.spawnDirPicker.rootPath) return;
    this.captureSpawnFormDraftFromDom();
    const reqId = this.nextReqId("spdir");
    this.spawnDirPicker.subPath = subPath;
    this.spawnDirPicker.loading = true;
    this.spawnDirPicker.error = null;
    this.spawnDirPicker.reqId = reqId;
    this.sendEncrypted(this.selectedDaemon, {
      op: "list_directory",
      req_id: reqId,
      root_path: this.spawnDirPicker.rootPath,
      sub_path: subPath,
    });
    this.render();
  }

  private stepUpSpawnDirectoryPicker() {
    const cleanSub = this.spawnDirPicker.subPath.replace(/^\/+|\/+$/g, "");
    if (cleanSub) {
      this.browseSpawnDirSubPath(parentSubPath(cleanSub));
      return;
    }
    const root = this.spawnDirPicker.rootPath.trim().replace(/\/+$/, "");
    if ((root.startsWith("/") && root !== "") || (root.startsWith("~/") && root !== "~")) {
      this.openSpawnDirectoryPicker(parentAbsolutePath(root));
    }
  }

  private submitBootstrapWorkspace() {
    if (!this.selectedDaemon?.is_online || this.spawnBootstrapLoading) return;
    this.captureSpawnFormDraftFromDom();
    const parent_dir = this.spawnBootstrapParentDir.trim() || "~";
    const folder_name = this.spawnBootstrapFolderName.trim();
    const rawUrl = this.spawnBootstrapGitUrl.trim();

    const folderErr = validateSingleFolderName(folder_name);
    if (folderErr) {
      this.spawnBootstrapMsg = { kind: "err", text: folderErr };
      this.render();
      return;
    }
    const urlErr = validateGitCloneUrlClient(rawUrl);
    if (urlErr) {
      this.spawnBootstrapMsg = { kind: "err", text: urlErr };
      this.render();
      return;
    }

    const reqId = this.nextReqId("boot");
    this.spawnBootstrapLoading = true;
    this.spawnBootstrapReqId = reqId;
    this.spawnBootstrapPeerId = this.selectedDaemon.id;
    this.spawnBootstrapMsg = {
      kind: "ok",
      text: rawUrl
        ? this.tr(`Cloning ${rawUrl} on remote host…`, `正在远端主机执行 git clone ${rawUrl}…`)
        : this.tr(`Creating workspace folder ${folder_name}…`, `正在远端创建目录 ${folder_name}…`),
    };
    this.sendEncrypted(this.selectedDaemon, {
      op: "bootstrap_workspace",
      req_id: reqId,
      parent_dir,
      folder_name,
      git_clone_url: rawUrl || null,
    });
    this.render();
  }

  private openRemoteFileBrowser(rootOverride?: string) {
    if (!this.selectedDaemon?.is_online) {
      this.showToastNotice("err", this.tr("Select an online host first.", "请先选择一台在线主机。"));
      return;
    }
    const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
    const defaultRoot =
      rootOverride ||
      activeTerm?.cwd ||
      this.repos[0]?.root_path ||
      ".";
    const dirReqId = this.nextReqId("lsdir");
    this.fileBrowser = {
      open: true,
      rootPath: defaultRoot,
      subPath: "",
      entries: [],
      selectedRelPath: null,
      previewContent: "",
      previewTruncated: false,
      loading: true,
      error: null,
      dirReqId,
      previewReqId: null,
    };
    this.sendEncrypted(this.selectedDaemon, {
      op: "list_directory",
      req_id: dirReqId,
      root_path: defaultRoot,
      sub_path: "",
    });
    this.render();
  }

  private browseRemoteSubPath(subPath: string) {
    if (!this.selectedDaemon?.is_online || !this.fileBrowser.rootPath) return;
    const dirReqId = this.nextReqId("lsdir");
    this.fileBrowser.subPath = subPath;
    this.fileBrowser.loading = true;
    this.fileBrowser.error = null;
    this.fileBrowser.dirReqId = dirReqId;
    this.sendEncrypted(this.selectedDaemon, {
      op: "list_directory",
      req_id: dirReqId,
      root_path: this.fileBrowser.rootPath,
      sub_path: subPath,
    });
    this.render();
  }

  private previewRemoteFile(relPath: string) {
    if (!this.selectedDaemon?.is_online || !this.fileBrowser.rootPath) return;
    const previewReqId = this.nextReqId("rfp");
    this.fileBrowser.selectedRelPath = relPath;
    this.fileBrowser.previewContent = "";
    this.fileBrowser.previewTruncated = false;
    this.fileBrowser.loading = true;
    this.fileBrowser.error = null;
    this.fileBrowser.previewReqId = previewReqId;
    this.sendEncrypted(this.selectedDaemon, {
      op: "read_file_preview",
      req_id: previewReqId,
      path: joinRootAndRel(this.fileBrowser.rootPath, relPath),
      max_bytes: 65536,
    });
    this.render();
  }

  private openRemotePortPreview() {
    if (!this.selectedDaemon?.is_online) {
      this.showToastNotice("err", this.tr("Select an online host first.", "请先选择一台在线主机。"));
      return;
    }
    this.portPreview.open = true;
    this.portPreview.error = null;
    this.render();
  }

  private submitRemotePortPreview(portRaw?: string, pathRaw?: string) {
    if (!this.selectedDaemon?.is_online) return;
    const pStr = portRaw !== undefined ? portRaw : this.portPreview.portInput;
    const pathStr = pathRaw !== undefined ? pathRaw : this.portPreview.pathInput;
    const parsed = parseWebPreviewPortAndPath(pStr, pathStr);
    if ("error" in parsed) {
      this.portPreview.error = parsed.error;
      this.portPreview.loading = false;
      this.portPreview.reqId = null;
      this.render();
      return;
    }
    const reqId = this.nextReqId("pp");
    this.portPreview.portInput = String(parsed.port);
    this.portPreview.pathInput = parsed.path;
    this.portPreview.loading = true;
    this.portPreview.error = null;
    this.portPreview.reqId = reqId;
    this.sendEncrypted(this.selectedDaemon, {
      op: "http_local_preview_request",
      req_id: reqId,
      port: parsed.port,
      path: parsed.path,
    });
    this.render();
  }

  private upgradeRemoteDaemon(dev: Device) {
    if (!dev.is_online) return;
    const msg = this.tr(
      `Upgrading coda-daemon on ${dev.name} (zero-downtime OTA)…`,
      `正在为 ${dev.name} 无感升级 coda-daemon…`
    );
    this.consoleNotice = { kind: "ok", text: msg };
    if (this.showSettingsModal) {
      this.settingsFeedback = { kind: "ok", text: msg };
    }
    this.sendEncrypted(dev, {
      op: "upgrade_daemon",
      req_id: `ota-${Date.now()}`,
      target_version: null,
    });
    this.render();
  }

  private sendPtyInputBytesTo(terminalAddr: string | null, bytes: Uint8Array | string) {
    if (!this.selectedDaemon || !terminalAddr) return;
    const raw = typeof bytes === "string" ? new TextEncoder().encode(bytes) : bytes;
    if (raw.length === 0) return;
    this.sendEncrypted(this.selectedDaemon, {
      op: "pty_input",
      terminal: terminalAddr,
      data_b64: toB64(raw),
    });
  }

  private sendPtyInputBytes(bytes: Uint8Array | string) {
    this.sendPtyInputBytesTo(this.getFocusedTerminalAddr(), bytes);
  }

  private handleClipboardImagePaste(
    e: ClipboardEvent,
    targetMode: "pty" | "draft" = "pty",
    terminalOverride?: string | null
  ): boolean {
    const imageBlobs = extractPureClipboardImages(e.clipboardData);
    if (imageBlobs.length === 0) return false;
    e.preventDefault();
    const pinnedDaemon = this.selectedDaemon;
    const pinnedTerminal = terminalOverride ?? this.getFocusedTerminalAddr();
    const baseTs = Date.now();
    void (async () => {
      for (let idx = 0; idx < imageBlobs.length; idx += 1) {
        const blob = imageBlobs[idx];
        const origName = "name" in blob ? String((blob as File).name || "") : "";
        const fname = buildClipboardImageFilename(baseTs, idx, blob.type, origName);
        await this.uploadSingleFileToHost(blob, pinnedDaemon, pinnedTerminal, fname, targetMode);
      }
    })();
    return true;
  }

  private async uploadSingleFileToHost(
    file: Blob | File,
    pinnedDaemon?: Device | null,
    pinnedTerminal?: string | null,
    explicitFilename?: string,
    targetMode: "pty" | "draft" = "pty"
  ) {
    const targetDaemon = pinnedDaemon ?? this.selectedDaemon;
    const targetTerminal =
      pinnedTerminal !== undefined ? pinnedTerminal : this.getFocusedTerminalAddr();
    if (!targetDaemon || !targetDaemon.is_online || !targetTerminal) {
      this.showToastNotice(
        "err",
        targetDaemon && !targetDaemon.is_online
          ? this.tr(
              `Host ${targetDaemon.name} is currently offline — reconnect before uploading files.`,
              `主机 ${targetDaemon.name} 当前离线，请待主机上线后再上传文件。`
            )
          : this.tr(
              "Select an online host and open a terminal first.",
              "请先选择一台在线主机并打开终端会话。"
            )
      );
      return;
    }
    if (file.size > MAX_UPLOAD_BYTES) {
      this.showToastNotice(
        "err",
        this.tr(
          `File too large (${(file.size / (1024 * 1024)).toFixed(1)} MiB > 4 MiB browser wire limit)`,
          `文件过大（${(file.size / (1024 * 1024)).toFixed(1)} MiB 超过浏览器 4 MiB 单次上传限制）`
        )
      );
      return;
    }
    const seq = ++this.uploadCounter;
    const reqToken = this.token;
    const fileOrigName = "name" in file ? String(file.name || "") : "";
    const rawName = explicitFilename || fileOrigName;
    const cleanedName = rawName.trim().replace(/^\.+/, "").slice(0, 100);
    const safeName =
      cleanedName ||
      buildClipboardImageFilename(Date.now(), seq - 1, file.type, fileOrigName);
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      if (!this.token || this.token !== reqToken || this.selectedDaemon?.id !== targetDaemon.id) {
        return;
      }
      const liveDaemon = this.devices.find((d) => d.id === targetDaemon.id) ?? targetDaemon;
      if (!liveDaemon.is_online) {
        this.showToastNotice(
          "err",
          this.tr(
            `Host ${liveDaemon.name} went offline before upload could start.`,
            `主机 ${liveDaemon.name} 已离线，无法开始上传。`
          )
        );
        return;
      }
      const reqId = `up-${Date.now()}-${seq}`;
      if (this.pendingUploadTargets.size >= 64) {
        const oldest = this.pendingUploadTargets.keys().next().value;
        if (oldest) this.pendingUploadTargets.delete(oldest);
      }
      this.pendingUploadTargets.set(reqId, {
        target: targetMode,
        daemonId: liveDaemon.id,
        terminalAddr: targetTerminal,
      });
      const b64 = toB64(bytes);
      this.showToastNotice(
        "ok",
        this.tr(`Uploading ${safeName} over E2EE…`, `正在端到端加密上传 ${safeName}…`)
      );
      this.sendEncrypted(liveDaemon, {
        op: "upload_terminal_file",
        req_id: reqId,
        terminal: targetTerminal,
        filename: safeName,
        data_b64: b64,
      });
    } catch (err: any) {
      this.showToastNotice(
        "err",
        String(err?.message || this.tr("Failed reading upload file", "读取上传文件失败"))
      );
    }
  }

  private getConsolePaletteItems(query: string): Array<{
    id: string;
    group: string;
    label: string;
    hint: string;
    action: () => void;
  }> {
    const q = query.trim().toLowerCase();
    const items: Array<{ id: string; group: string; label: string; hint: string; action: () => void }> = [];

    const attentionItems = collectNeedsAttentionItems(
      this.devices,
      this.terminalsByPeer,
      this.selectedDaemon?.id ?? null,
      this.terminals
    );
    items.push({
      id: "action:jump-needs-you",
      group: this.tr("Attention", "待办审批"),
      label: this.tr(
        `Jump to Next Needs-You Session${attentionItems.length > 0 ? ` (${attentionItems.length})` : ""}`,
        `跳转至下一个待审批 (Needs-You) 终端${attentionItems.length > 0 ? ` (${attentionItems.length})` : ""}`
      ),
      hint: "⌘J / Ctrl+Shift+J",
      action: () => this.jumpToNextNeedsYouTerminal(),
    });

    for (const dev of this.devices) {
      items.push({
        id: `host:${dev.id}`,
        group: this.tr("Hosts", "切换主机"),
        label: dev.name,
        hint: `${dev.platform} · ${dev.is_online ? this.tr("Online", "在线") : this.tr("Offline", "离线")}`,
        action: () => {
          this.selectDaemon(dev);
        },
      });
    }

    for (const term of this.terminals) {
      const title = term.status?.title || term.name || term.address;
      const st = term.status?.state || "idle";
      items.push({
        id: `term:${term.address}`,
        group: this.tr("Terminals", "切换终端"),
        label: title,
        hint: `${st} · ${term.cwd || "~"}`,
        action: () => {
          if (this.activeTerminalAddr && this.activeTerminalAddr !== term.address) {
            this.cancelActiveVoiceCapture();
            this.detachActiveTerminal();
          }
          this.activeTerminalAddr = term.address;
          this.focusedPane = "primary";
          writeLocalPref("coda_last_terminal_addr", term.address);
          this.attachAndReadTerminal(term.address);
        },
      });
    }

    if (this.selectedDaemon?.is_online) {
      const agents: Array<{ key: string; label: string }> = [
        { key: "claude", label: "Claude Code" },
        { key: "codex", label: "Codex CLI" },
        { key: "grok", label: "Grok CLI" },
        { key: "agy", label: "Antigravity (AGY)" },
        { key: "gemini", label: "Gemini CLI" },
        { key: "shell", label: "Interactive Shell" },
      ];
      for (const ag of agents) {
        items.push({
          id: `spawn:${ag.key}`,
          group: this.tr("Launch Agent", "启动 Agent"),
          label: `${this.tr("New", "新建")} ${ag.label}`,
          hint: this.selectedDaemon.name,
          action: () => {
            if (!this.selectedDaemon?.is_online) return;
            const { cols, rows } = this.computeTerminalDimsFromDom();
            this.termCols = cols;
            this.termRows = rows;
            const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
            this.sendEncrypted(this.selectedDaemon, {
              op: "launch_agent",
              req_id: `run-${Date.now()}`,
              agent: ag.key,
              name: null,
              cwd: activeTerm?.cwd || null,
              initial_prompt: null,
              cols,
              rows,
            });
          },
        });
      }
      items.push(
        {
          id: "action:git-diff",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Review Remote Git Changes / Diff", "审查远端 Git 变更与 Diff"),
          hint: "E2EE · git diff",
          action: () => this.openRemoteGitDiff(),
        },
        {
          id: "action:composer",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Toggle Multi-Line Prompt Composer", "展开/收起多行 Prompt 编排器"),
          hint: this.showPromptComposer ? "ON · Bracketed Paste" : "Bracketed Paste",
          action: () => {
            this.showPromptComposer = !this.showPromptComposer;
          },
        },
        {
          id: "action:probe-lan",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Probe LAN Direct Now", "立即探测局域网直连"),
          hint: this.activeTransportMode === "lan_ws" ? "LAN Direct" : "Smart Relay",
          action: () => {
            if (!this.selectedDaemon?.is_online) return;
            this.smartPath.resetForNetworkChange(this.selectedDaemon.id);
            void this.tryNegotiateDirectLanWs(this.selectedDaemon, true);
          },
        },
        {
          id: "action:files",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Browse Remote Workspace Files", "浏览远端工作区文件树"),
          hint: "E2EE · 64KB preview",
          action: () => this.openRemoteFileBrowser(),
        },
        {
          id: "action:port-preview",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Preview Remote localhost Port", "探测远端本地开发端口 (localhost)"),
          hint: "127.0.0.1:PORT · E2EE",
          action: () => this.openRemotePortPreview(),
        },
        {
          id: "action:split",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Toggle Split Terminals", "切换双栏分屏终端"),
          hint: `${this.splitMode ? "ON · " : ""}⌘D / Ctrl+Shift+D`,
          action: () => this.toggleSplitView(),
        },
        {
          id: "action:search",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Find in Terminal…", "终端内搜索…"),
          hint: "⌘F / Ctrl+F",
          action: () => {
            this.showTermSearch = true;
          },
        },
        {
          id: "action:export-md",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr("Export Session Log (.md)", "导出当前会话日志 (.md)"),
          hint: "Markdown",
          action: () => this.exportActiveSessionMarkdown(),
        },
        {
          id: "action:ota-host",
          group: this.tr("Actions", "快捷操作"),
          label: this.tr(`Upgrade ${this.selectedDaemon.name} Daemon (Zero-Downtime OTA)`, `一键无感升级 ${this.selectedDaemon.name} 守护进程`),
          hint: formatWebHostHealth(this.hostHealthByPeer.get(this.selectedDaemon.id)) || "E2EE OTA",
          action: () => {
            if (this.selectedDaemon) this.upgradeRemoteDaemon(this.selectedDaemon);
          },
        }
      );
    }

    for (const snip of this.promptSnippets) {
      const compactPrompt = snip.prompt.replace(/\s+/g, " ").trim();
      items.push({
        id: `snippet:${snip.id}`,
        group: this.tr("Snippets", "常用片段"),
        label: `${this.tr("Snippet", "片段")}: ${snip.title}`,
        hint: compactPrompt.length > 40 ? `${compactPrompt.slice(0, 39)}…` : compactPrompt,
        action: () => {
          const targetAddr = this.getFocusedTerminalAddr();
          if (this.selectedDaemon?.is_online && targetAddr && !this.showPromptComposer) {
            const targetVt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, targetAddr));
            const targetTerm = this.terminals.find((t) => t.address === targetAddr);
            const useBp = resolveEffectiveBracketedPaste({
              bracketedPaste: targetVt?.bracketedPaste,
              bracketedPasteExplicit: targetVt?.bracketedPasteExplicit,
              term: targetTerm,
            });
            this.sendPtyInputBytesTo(targetAddr, encodeComposerSubmit(snip.prompt, useBp));
            this.showToastNotice(
              "ok",
              this.tr(`Sent snippet "${snip.title}"`, `已发送常用片段「${snip.title}」`)
            );
          } else {
            this.showPromptComposer = true;
            this.draftInput = insertSnippetIntoDraft(this.draftInput, snip.prompt);
            this.showToastNotice(
              "ok",
              this.tr(
                `Inserted snippet "${snip.title}" into composer`,
                `已将片段「${snip.title}」插入编排器`
              )
            );
          }
        },
      });
    }

    items.push(
      {
        id: "action:web-push",
        group: this.tr("Actions", "快捷操作"),
        label: this.tr(
          "Enable 1-Click Lock-Screen Push (Browser / iOS PWA)",
          "一键开启锁屏推送通知 (浏览器 / iOS PWA)"
        ),
        hint: this.webPushThisDeviceSubscribed ? "Active · VAPID" : "W3C VAPID",
        action: () => {
          void this.enableLockScreenWebPush();
        },
      },
      {
        id: "action:webhooks",
        group: this.tr("Actions", "快捷操作"),
        label: this.tr("Configure Offline Alert Webhooks", "配置离线告警推送 Webhook"),
        hint: "Bark · Telegram · Feishu · Discord",
        action: () => {
          this.showSettingsModal = true;
          this.settingsTab = "alerts";
          this.settingsFeedback = null;
          this.settingsDirtyFields.clear();
          void this.loadAccountSettingsData().then(() => {
            this.renderSettingsPreservingInputs();
          });
        },
      },
      {
        id: "action:settings",
        group: this.tr("Actions", "快捷操作"),
        label: this.tr("Open Workspace & Account Settings", "打开工作区与账号设置"),
        hint: "Devices · Alerts · Security · Voice",
        action: () => {
          this.showSettingsModal = true;
          this.settingsFeedback = null;
          this.settingsDirtyFields.clear();
          void this.loadAccountSettingsData().then(() => {
            this.renderSettingsPreservingInputs();
          });
        },
      },
      {
        id: "action:theme",
        group: this.tr("Actions", "快捷操作"),
        label: this.tr("Toggle Light / Dark Theme", "切换浅色 / 深色主题"),
        hint: this.themePref,
        action: () => {
          const next: ThemePreference = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
          this.applyTheme(next, true);
        },
      },
      {
        id: "action:lang",
        group: this.tr("Actions", "快捷操作"),
        label: this.tr("Switch Language (EN / 中文)", "切换界面语言 (中文 / EN)"),
        hint: this.locale === "zh" ? "中文" : "EN",
        action: () => {
          this.applyLocale(this.locale === "en" ? "zh" : "en", true);
        },
      }
    );

    if (!q) return items;
    return items.filter(
      (it) =>
        it.label.toLowerCase().includes(q) ||
        it.hint.toLowerCase().includes(q) ||
        it.group.toLowerCase().includes(q)
    );
  }

  private renderConsolePaletteRowsHtml(
    items: ReadonlyArray<{ id: string; group: string; label: string; hint: string }>
  ): string {
    if (items.length === 0) {
      return `<div style="padding:18px 10px;text-align:center;font-size:12px;color:var(--text-tertiary);">${this.tr("No matching commands", "无匹配指令")}</div>`;
    }
    const sel = Math.max(0, Math.min(items.length - 1, this.cmdPaletteSelectedIdx));
    return items
      .map(
        (it, idx) => `
          <button type="button" class="cmd-row ${idx === sel ? "active" : ""}" role="option" aria-selected="${idx === sel ? "true" : "false"}" data-cmd-idx="${idx}">
            <span style="display:inline-flex;align-items:center;gap:8px;min-width:0;">
              <span class="pm-badge" style="font-size:10px;padding:2px 6px;">${this.escapeHtml(it.group)}</span>
              <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${this.escapeHtml(it.label)}</span>
            </span>
            <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);flex-shrink:0;margin-left:8px;">${this.escapeHtml(it.hint)}</span>
          </button>`
      )
      .join("");
  }

  private renderConsoleCommandPaletteModal(): string {
    const items = this.getConsolePaletteItems(this.cmdPaletteQuery);
    return `
      <div class="modal-mask" id="console-cmd-mask">
        <div class="modal-card" style="max-width:540px;">
          <div style="display:flex;align-items:center;gap:8px;padding-bottom:10px;border-bottom:1px solid var(--border-primary);">
            ${svgIcon("search", 15)}
            <input id="console-cmd-input" class="pm-input" style="margin:0;border:none;background:transparent;padding:4px 0;flex:1;box-shadow:none;" value="${this.escapeHtml(
              this.cmdPaletteQuery
            )}" placeholder="${this.tr("Jump to host, terminal, git diff, or run action…", "搜索主机、终端会话、Git Diff 或执行快捷操作…")}" autocomplete="off" />
            <span class="pm-kbd">↑↓</span>
            <span class="pm-kbd">ESC</span>
          </div>
          <div class="cmd-list" id="console-cmd-list" role="listbox" style="max-height:340px;overflow-y:auto;">
            ${this.renderConsolePaletteRowsHtml(items)}
          </div>
        </div>
      </div>
    `;
  }

  private isSafeVoiceBaseUrl(raw: string): boolean {
    try {
      const u = new URL(raw.trim());
      if (u.protocol === "https:") return true;
      if (
        u.protocol === "http:" &&
        (u.hostname === "127.0.0.1" || u.hostname === "localhost" || u.hostname === "::1")
      ) {
        return true;
      }
      return false;
    } catch {
      return false;
    }
  }

  private hasBrowserSpeechRecognition(): boolean {
    if (typeof window === "undefined") return false;
    return Boolean((window as any).SpeechRecognition || (window as any).webkitSpeechRecognition);
  }

  private stopVoiceCaptureResources() {
    this.voiceChunks = [];
    if (this.pendingTranscribeTimer) {
      clearTimeout(this.pendingTranscribeTimer);
      this.pendingTranscribeTimer = null;
    }
    if (this.speechRecognition) {
      this.speechRecognition.onresult = null;
      this.speechRecognition.onerror = null;
      this.speechRecognition.onend = null;
      try { this.speechRecognition.stop(); } catch {}
      this.speechRecognition = null;
    }
    if (this.mediaRecorder) {
      this.mediaRecorder.ondataavailable = null;
      this.mediaRecorder.onstop = null;
      this.mediaRecorder.onerror = null;
      if (this.mediaRecorder.state !== "inactive") {
        try { this.mediaRecorder.stop(); } catch {}
      }
      this.mediaRecorder = null;
    }
    if (this.mediaStream) {
      for (const track of this.mediaStream.getTracks()) {
        try { track.stop(); } catch {}
      }
      this.mediaStream = null;
    }
  }

  private cancelActiveVoiceCapture() {
    this.pendingTranscribeReqId = null;
    const wasActive =
      this.isRecordingVoice ||
      this.isTranscribingVoice ||
      Boolean(this.speechRecognition || this.mediaRecorder || this.mediaStream);
    this.stopVoiceCaptureResources();
    this.isRecordingVoice = false;
    this.isTranscribingVoice = false;
    if (wasActive) {
      this.syncVoiceButtonInPlace();
    }
  }

  private insertTranscribedText(text: string) {
    if (!this.token) return;
    const cleaned = text.trim();
    if (!cleaned) {
      this.syncVoiceButtonInPlace();
      return;
    }
    this.draftInput = this.draftInput ? `${this.draftInput} ${cleaned}` : cleaned;
    this.syncVoiceButtonInPlace();
    this.updateDraftPreviewInPlace();

    const composerArea = document.getElementById("prompt-composer-textarea") as HTMLTextAreaElement | null;
    if (composerArea) {
      composerArea.value = this.draftInput;
      composerArea.focus();
      this.showToastNotice("ok", this.tr(`Voice transcribed: "${cleaned}"`, `语音识别完成：「${cleaned}」`));
      return;
    }
    const desktopInp = document.getElementById("desktop-prompt-input") as HTMLInputElement | null;
    if (desktopInp) {
      desktopInp.value = this.draftInput;
      desktopInp.focus();
      this.showToastNotice("ok", this.tr(`Voice transcribed: "${cleaned}"`, `语音识别完成：「${cleaned}」`));
      return;
    }
    const kbInp = document.getElementById("kb-input") as HTMLInputElement | null;
    if (kbInp) {
      kbInp.value = this.draftInput;
      kbInp.focus();
      this.showToastNotice("ok", this.tr(`Voice transcribed: "${cleaned}"`, `语音识别完成：「${cleaned}」`));
      return;
    }
    this.showToastNotice("ok", this.tr(`Voice transcribed: "${cleaned}"`, `语音识别完成：「${cleaned}」`));
    this.render();
  }

  private async toggleVoiceInput() {
    if (this.isTranscribingVoice) return;
    if (this.isRecordingVoice) {
      if (this.speechRecognition) {
        try { this.speechRecognition.stop(); } catch {
          this.isRecordingVoice = false;
          this.speechRecognition = null;
          this.render();
        }
        return;
      }
      if (this.mediaRecorder && this.mediaRecorder.state === "recording") {
        this.mediaRecorder.stop();
        return;
      }
      this.isRecordingVoice = false;
      this.stopVoiceCaptureResources();
      this.render();
      return;
    }

    const effectiveKey = (this.voiceApiKeyLocal || this.voiceConfig.api_key || "").trim();
    const SpeechRec =
      typeof window !== "undefined"
        ? (window as any).SpeechRecognition || (window as any).webkitSpeechRecognition
        : null;
    const hasMediaRec =
      typeof navigator !== "undefined" &&
      Boolean(navigator.mediaDevices?.getUserMedia) &&
      typeof MediaRecorder !== "undefined";

    const strategy = resolveVoiceCaptureStrategy({
      apiKey: effectiveKey,
      isSafeBaseUrl: this.isSafeVoiceBaseUrl(this.voiceConfig.base_url),
      hasMediaRecorder: hasMediaRec,
      hasSpeechRecognition: Boolean(SpeechRec),
      hasOnlineDaemon: Boolean(this.selectedDaemon?.is_online),
    });

    if (strategy === "whisper") {
      await this.startMediaRecorderVoice(effectiveKey, "direct");
      return;
    }

    if (strategy === "daemon_whisper") {
      await this.startMediaRecorderVoice("", "daemon");
      return;
    }

    if (strategy === "browser_speech" && SpeechRec) {
      this.startWebSpeechFallback(SpeechRec);
      return;
    }

    this.showToastNotice(
      "err",
      this.tr(
        "Configure a Whisper API key in Settings -> Voice, or use a browser supporting Web Speech / MediaRecorder.",
        "请在「设置 → 语音模型」中配置 Whisper API Key，或使用支持免配置语音识别的现代浏览器。"
      )
    );
  }

  private async startMediaRecorderVoice(
    apiKey: string,
    mode: "direct" | "daemon" = "direct"
  ) {
    const originDaemon = this.selectedDaemon;
    const originTerminalAddr = this.getFocusedTerminalAddr();
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      this.mediaStream = stream;
      this.voiceChunks = [];
      const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : MediaRecorder.isTypeSupported("audio/mp4")
          ? "audio/mp4"
          : "";
      const recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);
      this.mediaRecorder = recorder;
      this.isRecordingVoice = true;
      this.showToastNotice(
        "ok",
        this.tr("Recording voice… click Mic again to transcribe.", "正在录音…再次点击麦克风结束并转写。")
      );
      this.syncVoiceButtonInPlace();

      recorder.ondataavailable = (ev) => {
        if (ev.data && ev.data.size > 0) this.voiceChunks.push(ev.data);
      };
      recorder.onstop = async () => {
        const chunks = [...this.voiceChunks];
        this.voiceChunks = [];
        const actualType = recorder.mimeType || mimeType || "audio/webm";
        this.isRecordingVoice = false;
        this.mediaRecorder = null;
        if (this.mediaStream) {
          for (const t of this.mediaStream.getTracks()) {
            try { t.stop(); } catch {}
          }
          this.mediaStream = null;
        }
        if (
          chunks.length === 0 ||
          this.selectedDaemon?.id !== originDaemon?.id ||
          this.getFocusedTerminalAddr() !== originTerminalAddr
        ) {
          this.syncVoiceButtonInPlace();
          return;
        }
        const blob = new Blob(chunks, { type: actualType });
        if (mode === "daemon") {
          await this.transcribeAudioViaDaemon(blob, actualType, originDaemon);
        } else {
          await this.transcribeAudioBlob(blob, apiKey, actualType, originDaemon?.id ?? null, originTerminalAddr);
        }
      };
      recorder.start();
    } catch (err: any) {
      this.isRecordingVoice = false;
      this.stopVoiceCaptureResources();
      this.showToastNotice(
        "err",
        this.tr(
          `Microphone error: ${err?.message || "permission denied"}`,
          `麦克风启动失败：${err?.message || "未授予麦克风权限"}`
        )
      );
      this.syncVoiceButtonInPlace();
    }
  }

  private async transcribeAudioViaDaemon(
    blob: Blob,
    mimeType: string,
    targetDaemon: Device | null | undefined
  ) {
    if (!this.token || !targetDaemon?.is_online || this.selectedDaemon?.id !== targetDaemon.id) {
      this.syncVoiceButtonInPlace();
      return;
    }
    if (blob.size > MAX_UPLOAD_BYTES) {
      this.showToastNotice(
        "err",
        this.tr(
          "Voice recording exceeds 4 MiB wire limit — record a shorter segment.",
          "单段语音超过 4 MiB 传输上限，请缩短单次录音时长。"
        )
      );
      return;
    }
    const reqToken = this.token;
    const reqId = this.nextReqId("asr");
    this.pendingTranscribeReqId = reqId;
    this.isTranscribingVoice = true;
    this.showToastNotice(
      "ok",
      this.tr(
        "Transcribing audio over E2EE via host Whisper key…",
        "正在通过端到端加密通道由远端主机 Whisper Key 转写语音…"
      )
    );
    this.syncVoiceButtonInPlace();

    if (this.pendingTranscribeTimer) {
      clearTimeout(this.pendingTranscribeTimer);
    }
    this.pendingTranscribeTimer = setTimeout(() => {
      if (this.pendingTranscribeReqId === reqId) {
        this.pendingTranscribeReqId = null;
        this.pendingTranscribeTimer = null;
        this.isTranscribingVoice = false;
        this.syncVoiceButtonInPlace();
        this.showToastNotice(
          "err",
          this.tr("Host voice transcription timed out.", "远端主机语音转写响应超时。")
        );
      }
    }, 35_000);

    try {
      const bytes = new Uint8Array(await blob.arrayBuffer());
      if (
        !this.token ||
        this.token !== reqToken ||
        this.pendingTranscribeReqId !== reqId ||
        this.selectedDaemon?.id !== targetDaemon.id
      ) {
        return;
      }
      const ext = mimeType.includes("mp4") || mimeType.includes("m4a") ? "m4a" : "webm";
      this.sendEncrypted(targetDaemon, {
        op: "transcribe_audio",
        req_id: reqId,
        audio_b64: toB64(bytes),
        mime_type: mimeType,
        filename: `voice.${ext}`,
        language: this.locale === "zh" ? "zh" : "en",
      });
    } catch (err: any) {
      if (this.pendingTranscribeReqId === reqId) {
        this.pendingTranscribeReqId = null;
        if (this.pendingTranscribeTimer) {
          clearTimeout(this.pendingTranscribeTimer);
          this.pendingTranscribeTimer = null;
        }
        this.isTranscribingVoice = false;
        this.syncVoiceButtonInPlace();
        this.showToastNotice(
          "err",
          this.tr(
            `Failed encoding audio for E2EE Whisper: ${err?.message || "error"}`,
            `语音加密发送失败：${err?.message || "未知错误"}`
          )
        );
      }
    }
  }

  private async transcribeAudioBlob(
    blob: Blob,
    apiKey: string,
    mimeType: string,
    originDaemonId: string | null = null,
    originTerminalAddr: string | null = null
  ) {
    if (!this.token) return;
    const reqToken = this.token;
    if (!this.isSafeVoiceBaseUrl(this.voiceConfig.base_url)) {
      this.showToastNotice(
        "err",
        this.tr("Voice Base URL must use https:// (or localhost).", "语音接口 Base URL 必须为 https://（或本机 localhost）。")
      );
      return;
    }
    this.isTranscribingVoice = true;
    this.showToastNotice(
      "ok",
      this.tr("Transcribing audio via Whisper API…", "正在调用 Whisper 接口转写语音…")
    );
    this.syncVoiceButtonInPlace();

    try {
      const ext = mimeType.includes("mp4") || mimeType.includes("m4a") ? "m4a" : "webm";
      const base = this.voiceConfig.base_url.trim().replace(/\/+$/, "");
      const endpoint = base.endsWith("/audio/transcriptions") ? base : `${base}/audio/transcriptions`;
      const form = new FormData();
      form.append("file", blob, `voice.${ext}`);
      form.append("model", (this.voiceConfig.model || "gpt-4o-mini-transcribe").trim());
      const resp = await fetch(endpoint, {
        method: "POST",
        headers: { Authorization: `Bearer ${apiKey}` },
        body: form,
      });
      const textBody = await resp.text();
      if (
        !this.token ||
        this.token !== reqToken ||
        (originDaemonId !== null && this.selectedDaemon?.id !== originDaemonId) ||
        (originTerminalAddr !== null && this.getFocusedTerminalAddr() !== originTerminalAddr)
      ) {
        this.isTranscribingVoice = false;
        this.syncVoiceButtonInPlace();
        return;
      }
      if (!resp.ok) {
        throw new Error(`HTTP ${resp.status}: ${textBody.slice(0, 160)}`);
      }
      let transcribed = "";
      try {
        const parsed = JSON.parse(textBody);
        transcribed = String(parsed.text || "");
      } catch {
        transcribed = textBody.trim();
      }
      this.isTranscribingVoice = false;
      if (transcribed.trim()) {
        this.insertTranscribedText(transcribed);
      } else {
        this.syncVoiceButtonInPlace();
        this.showToastNotice("err", this.tr("Empty transcription returned.", "未识别到有效语音内容。"));
      }
    } catch (err: any) {
      this.isTranscribingVoice = false;
      this.syncVoiceButtonInPlace();
      this.showToastNotice(
        "err",
        this.tr(
          `Whisper transcription failed: ${err?.message || "network error"}`,
          `Whisper 语音转写失败：${err?.message || "网络错误"}`
        )
      );
    }
  }

  private startWebSpeechFallback(SpeechRecCtor: any) {
    try {
      const rec = new SpeechRecCtor();
      rec.lang = this.locale === "zh" ? "zh-CN" : "en-US";
      rec.continuous = true;
      rec.interimResults = true;
      rec.maxAlternatives = 1;
      this.speechRecognition = rec;
      this.isRecordingVoice = true;
      const baseDraftBeforeSpeech = this.draftInput;
      const originDaemonId = this.selectedDaemon?.id ?? null;
      const originTerminalAddr = this.getFocusedTerminalAddr();
      let latestCombined = "";
      let flushed = false;

      const applyLiveDraft = (combined: string) => {
        const nextDraft = combined
          ? baseDraftBeforeSpeech
            ? `${baseDraftBeforeSpeech} ${combined}`
            : combined
          : baseDraftBeforeSpeech;
        this.draftInput = nextDraft;
        this.updateDraftPreviewInPlace();
        const composerArea = document.getElementById(
          "prompt-composer-textarea"
        ) as HTMLTextAreaElement | null;
        const desktopInp = document.getElementById(
          "desktop-prompt-input"
        ) as HTMLInputElement | null;
        const kbInp = document.getElementById("kb-input") as HTMLInputElement | null;
        if (composerArea) composerArea.value = nextDraft;
        if (desktopInp) desktopInp.value = nextDraft;
        if (kbInp) kbInp.value = nextDraft;
      };

      const flushTranscript = () => {
        if (flushed) return;
        flushed = true;
        this.isRecordingVoice = false;
        this.speechRecognition = null;
        if (
          !this.token ||
          this.selectedDaemon?.id !== originDaemonId ||
          this.getFocusedTerminalAddr() !== originTerminalAddr
        ) {
          this.syncVoiceButtonInPlace();
          return;
        }
        const cleaned = latestCombined.trim();
        if (cleaned) {
          this.draftInput = baseDraftBeforeSpeech;
          this.insertTranscribedText(cleaned);
        } else {
          this.syncVoiceButtonInPlace();
        }
      };

      this.showToastNotice(
        "ok",
        this.tr(
          "Listening live via browser SpeechRecognition (zero API key)… click Mic to finish.",
          "正在使用浏览器本地免配置实时听写…再次点击麦克风完成输入。"
        )
      );
      this.syncVoiceButtonInPlace();

      rec.onresult = (ev: any) => {
        if (
          !this.token ||
          this.selectedDaemon?.id !== originDaemonId ||
          this.getFocusedTerminalAddr() !== originTerminalAddr
        ) {
          flushed = true;
          this.cancelActiveVoiceCapture();
          return;
        }
        // Rebuild full transcript cleanly from `0 .. results.length - 1` on every event
        // so Safari `resultIndex = 0` ticks never duplicate finalized segments.
        const { combined } = rebuildSpeechRecognitionTranscript(ev?.results);
        latestCombined = combined;
        applyLiveDraft(combined);
        if (combined) {
          this.consoleNotice = {
            kind: "ok",
            text: this.tr(`Dictating: "${combined}"`, `实时听写中：「${combined}」`),
          };
          this.updateConsoleNoticeInPlace();
        }
      };
      rec.onerror = (ev: any) => {
        if (ev?.error === "aborted") {
          flushTranscript();
          return;
        }
        flushed = true;
        this.isRecordingVoice = false;
        this.speechRecognition = null;
        this.syncVoiceButtonInPlace();
        this.showToastNotice(
          "err",
          this.tr(
            `Speech recognition error: ${ev?.error || "unavailable"}`,
            `浏览器语音识别失败：${ev?.error || "不可用"}`
          )
        );
      };
      rec.onend = () => {
        flushTranscript();
      };
      rec.start();
    } catch (err: any) {
      this.isRecordingVoice = false;
      this.speechRecognition = null;
      this.syncVoiceButtonInPlace();
      this.showToastNotice(
        "err",
        String(err?.message || this.tr("Voice input unavailable", "无法启动语音输入"))
      );
    }
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
                <span style="color:var(--emerald);">v1.1.1</span>
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
                <span>Lock-Screen Push</span>
                <span style="color:var(--emerald);">VAPID</span>
              </div>
              <div class="pm-wb-tree-row">
                <span>Voice + ⌘V Paste</span>
                <span>Snippets</span>
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
                <div style="color:#a1a1aa;margin-bottom:6px;">Voice + Saved Snippets + ⌘V Screenshot</div>
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
              <span>Claude Code · Codex</span>
              <span style="color:var(--emerald);">live</span>
            </div>
            <div class="pm-wb-tree-row">
              <span>Grok · AGY · Gemini</span>
              <span>ready</span>
            </div>
          </div>
          <div class="bento-well coda-mono" style="font-size:10.5px;color:var(--text-tertiary);">
            GPUI 120Hz · Self-Update OTA
          </div>
        </div>

        <div class="pm-wb-pane">
          <div class="pm-diff-box">
            <div class="pm-diff-head">
              <span>E2EE Git Diff · File Tree · Port Preview</span>
              <span class="coda-mono pm-num"><span style="color:var(--emerald);">+1,233</span> <span style="color:var(--rose);">-116</span></span>
            </div>
            <div class="pm-diff-line ctx"><span class="pm-diff-ln">42</span><span>  pub fn seal_frame(&amp;mut self, msg: &amp;PeerMessage) -&gt; Envelope {</span></div>
            <div class="pm-diff-line del"><span class="pm-diff-ln">43</span><span>-   let wire = serde_json::to_vec(msg)?;</span></div>
            <div class="pm-diff-line add"><span class="pm-diff-ln">43</span><span>+   let compressed = zstd::bulk::compress(&amp;serde_json::to_vec(msg)?, 3)?;</span></div>
            <div class="pm-diff-line add"><span class="pm-diff-ln">44</span><span>+   let cipher = self.aead.encrypt(&amp;nonce, compressed.as_ref())?;</span></div>
            <div class="pm-diff-line ctx"><span class="pm-diff-ln">45</span><span>    Envelope { ephem_pub, nonce, ciphertext: cipher }</span></div>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;font-size:11.5px;color:var(--text-secondary);">
            <span class="coda-mono">X25519 + ChaCha20-Poly1305 · 127.0.0.1 Preview</span>
            <span class="pm-badge green">Zero Server Plaintext</span>
          </div>
        </div>

        <div class="pm-wb-pane">
          <div class="pm-pty-box">
            <div>
              <div style="color:#a1a1aa;margin-bottom:6px;">Claude Code &middot; hc attach @mac/main</div>
              <div>&bull; Verified E2EE Git Diff, File Tree &amp; Port Preview</div>
              <div style="color:#22c55e;">&bull; 1-Click Lock-Screen Push &amp; Zero-Downtime OTA active</div>
              <div style="color:#3b82f6;">&bull; Voice + ⌘V screenshot paste + Saved Snippets ready</div>
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
          <span class="pm-badge green"><span class="pm-status-dot"></span>Zero-Downtime OTA</span>
        </div>
        <div class="coda-mono bento-muted" style="font-size:11px;line-height:1.65;">
          <div>&bull; 127.0.0.1:8080 &rarr; wss://codahub.dev/v1/tunnel/ws</div>
          <div>&bull; W3C VAPID Web Push + PostgreSQL 18 · Relay :24443</div>
          <div>&bull; Endpoint: ${this.escapeHtml(baseUrl)}</div>
        </div>
      `;
    }
    if (this.previewTab === "ios") {
      return `
        <div class="bento-well" style="display:flex;justify-content:space-between;align-items:center;">
          <span class="coda-mono" style="font-size:11.5px;font-weight:600;">iPhone Safari PWA / Swift 6 App</span>
          <span class="pm-badge blue"><span class="pm-status-dot"></span>Lock-Screen Push</span>
        </div>
        <div class="coda-mono bento-muted" style="font-size:11px;line-height:1.65;">
          <div>&bull; Happy Eyeballs 250ms &rarr; ws://192.168.1.20:24444</div>
          <div>&bull; Saved Snippets + Voice Dictation + Screenshot Paste</div>
          <div>&bull; E2EE Git Diff, File Tree &amp; Localhost Port Preview</div>
        </div>
      `;
    }
    return `
      <div class="bento-well" style="display:flex;justify-content:space-between;align-items:center;">
        <span class="coda-mono" style="font-size:11.5px;font-weight:600;">MacBook-Pro · GPUI 120Hz</span>
        <span class="pm-badge green"><span class="pm-status-dot"></span>3-Pane + Self-Update</span>
      </div>
      <div class="coda-mono bento-muted" style="font-size:11px;line-height:1.65;">
        <div>&bull; Claude Code · Codex · Grok CLI · AGY · Gemini CLI</div>
        <div>&bull; E2EE Git Diff (<span style="color:var(--emerald);">+1,233</span> <span style="color:var(--rose);">-116</span>) &amp; Live 127.0.0.1 Tunnel</div>
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
    this.imeComposing = false;
    const root = document.getElementById("app")!;
    const baseUrl = this.siteInfo?.public_base_url || window.location.origin;
    const installCmd = this.siteInfo?.install_command || `curl -fsSL ${baseUrl}/install.sh | sh`;
    const ver = this.siteInfo?.version || "v1.1.1";
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

    if (this.route === "reset-password") {
      this.stopBentoLoop();
      this.renderResetPasswordView(root, baseUrl, ver);
      return;
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
                <span>${this.tr("1-Click Lock-Screen Push · Zero-Downtime OTA · SmartPath <1ms LAN", "一键锁屏推送 (Web Push) · 无感守护进程 OTA · 局域网毫秒直连")}</span>
                ${svgIcon("arrowRight", 13)}
              </a>
              <div class="pm-hero-meta">
                <span class="pm-badge blue coda-mono">Claude Code · Codex · Grok CLI · Antigravity (AGY) · Gemini CLI</span>
              </div>
              <h1 class="pm-h1">${this.tr("The zero-knowledge control plane<br />for cross-device AI coding.", "跨设备 AI 编程的<br />零知识控制面。")}</h1>
              <p class="pm-hero-sub">
                ${this.tr(
                  "Orchestrate <strong>Claude Code · Codex · Grok CLI · Antigravity (AGY) · Gemini CLI</strong> across your Linux servers, 120Hz macOS GPUI desktop, and iPhone PWA. Inspect <strong>E2EE Git Diff &amp; File Tree</strong>, <strong>Localhost Port Preview</strong>, <strong>Voice + <code>⌘V</code> Screenshot Paste</strong>, and <strong>1-Click Lock-Screen Push &amp; Zero-Downtime OTA</strong>.",
                  "在同一张零知识加密网中统筹 Linux 云主机、Mac 原生 120Hz 桌面端与手机 PWA 上的 <strong>Claude Code · Codex · Grok CLI · Antigravity (AGY) · Gemini CLI</strong>。内置 <strong>端到端 Git Diff 与文件树</strong>、<strong>Localhost 端口预览</strong>、<strong>语音输入 + <code>⌘V</code> 粘贴截图直传</strong> 与 <strong>一键锁屏推送 &amp; 无感 OTA 升级</strong>。"
                )}
              </p>
              <div class="pm-hero-actions">
                ${
                  dl && safeHref(dl.desktop_macos_dmg) !== "#"
                    ? `<a class="pm-btn pm-btn-primary pm-btn-lg" href="${safeHref(dl.desktop_macos_dmg)}" target="_blank" rel="noopener noreferrer">${svgIcon("apple", 15)} ${this.tr("Download for macOS (.dmg)", "下载 macOS 桌面端 (.dmg)")}</a>`
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
                      <span>Claude Code &middot; E2EE Git Diff</span>
                      <span class="pm-num" style="color:var(--emerald);">+412 -38</span>
                    </div>
                    <div class="bento-pty-line" data-pty-idx="1">
                      <span>Codex &middot; Grok CLI &middot; AGY &middot; Gemini</span>
                      <span class="pm-num" style="color:var(--signal-blue);">+821 -78</span>
                    </div>
                    <div class="bento-pty-line" data-pty-idx="2">
                      <span>127.0.0.1 Port Preview &middot; OTA</span>
                      <span style="color:var(--emerald);">200 OK</span>
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
                    <a href="/activate" data-nav="/activate" style="color:var(--signal-blue);">/activate &rarr;</a>
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

                <!-- Card 6: Voice + Screenshot Paste + Lock-Screen Push (Dark Contrast) -->
                <article class="bento-card is-dark bento-span-3">
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
                    <div style="display:flex;align-items:center;gap:7px;">
                      ${svgIcon("mic", 14)}
                      <span class="bento-kicker">${this.tr("Voice · ⌘V Paste · Push", "语音 · ⌘V 截图 · 锁屏推送")}</span>
                    </div>
                    <span class="pm-badge coda-mono">VAPID</span>
                  </div>
                  <div class="bento-wave">
                    ${[0, 1, 2, 3, 4, 5, 6, 7, 8]
                      .map((i) => `<span class="bento-bar" data-voice-idx="${i}"></span>`)
                      .join("")}
                  </div>
                  <div class="bento-well coda-mono" style="font-size:10.5px;display:flex;justify-content:space-between;align-items:center;gap:6px;white-space:nowrap;">
                    <span><span style="color:var(--emerald);">&gt;</span> Snippets + ASR</span>
                    <span style="color:var(--emerald);">⌘V Upload</span>
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
                    <div class="coda-mono" style="font-size:11px;color:var(--text-tertiary);margin-bottom:8px;">01 / WORKBENCH &amp; OTA</div>
                    <div style="font-size:15px;font-weight:600;margin-bottom:6px;">${this.tr("5 AI Agents &middot; Zero-Downtime OTA", "五大 Agent 引擎 · 无感热升级")}</div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.6;">${this.tr("Run <code>Claude Code</code>, <code>Codex</code>, <code>Grok CLI</code>, <code>Antigravity (AGY)</code>, and <code>Gemini CLI</code> on persistent <code>herdr-core</code> PTYs with E2EE Git Diff, File Tree, Localhost Port Preview, and 1-click OTA.", "常驻托管 <code>Claude Code</code>、<code>Codex</code>、<code>Grok CLI</code>、<code>Antigravity (AGY)</code> 与 <code>Gemini CLI</code>，内置端到端 Git Diff、文件树、Localhost 端口预览与无感守护进程 OTA。")}</p>
                  </div>
                  <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">herdr-core · Git Diff · Port Preview</span>
                </div>
                <div class="pm-grid-cell">
                  <div>
                    <div class="coda-mono" style="font-size:11px;color:var(--text-tertiary);margin-bottom:8px;">02 / CRYPTO &amp; INPUT</div>
                    <div style="font-size:15px;font-weight:600;margin-bottom:6px;">${this.tr("Zero-Knowledge E2EE &middot; Voice + ⌘V Paste", "零知识加密 · 语音 + ⌘V 截图 + 片段")}</div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.6;">${this.tr("Streams, <code>⌘V</code> clipboard screenshots, voice dictation, and cross-device Prompt Snippets stay sealed end-to-end with <code>X25519 + ChaCha20-Poly1305</code>; relays see 0 plaintext.", "终端流、<code>⌘V</code> 剪贴板截图直传、实时语音听写与跨端同步的常用 Prompt 片段全程经 <code>X25519 + ChaCha20-Poly1305</code> 端到端加密，服务器零明文。")}</p>
                  </div>
                  <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">HKDF-SHA256 · Blind Relay</span>
                </div>
                <div class="pm-grid-cell">
                  <div>
                    <div class="coda-mono" style="font-size:11px;color:var(--text-tertiary);margin-bottom:8px;">03 / PUSH &amp; ROUTING</div>
                    <div style="font-size:15px;font-weight:600;margin-bottom:6px;">${this.tr("1-Click Lock-Screen Push &middot; &lt;1ms LAN", "一键锁屏推送 · 同网亚毫秒直连")}</div>
                    <p style="font-size:13px;color:var(--text-secondary);line-height:1.6;">${this.tr("Enable standard W3C VAPID Web Push on Browser / iOS PWA (or Bark / Telegram / Feishu / Discord) for instant Needs-You alerts, with automatic <code>:24444</code> LAN direct cutover.", "一键开启标准 W3C VAPID 浏览器 / iOS PWA 锁屏推送（兼容 Bark / Telegram / 飞书 / Discord），同 Wi-Fi 秒切 <code>:24444</code> 局域网直连（&lt;1ms）。")}</p>
                  </div>
                  <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">W3C VAPID · SmartPath · CF WSS</span>
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
              ${dl && safeHref(dl.releases_page) !== "#" ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${safeHref(dl.releases_page)}" target="_blank" rel="noopener noreferrer">${this.tr("GitHub Releases &amp; Checksums", "查看 GitHub Releases &amp; Checksums")} ${svgIcon("arrowRight", 13)}</a>` : ""}
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
                    ${dl && safeHref(dl.desktop_macos_dmg) !== "#" ? `<a class="pm-btn pm-btn-primary pm-btn-sm" href="${safeHref(dl.desktop_macos_dmg)}" target="_blank" rel="noopener noreferrer">${this.tr("Download .dmg", "下载 .dmg 安装包")}</a>` : ""}
                    ${dl && safeHref(dl.desktop_macos_tar) !== "#" ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${safeHref(dl.desktop_macos_tar)}" target="_blank" rel="noopener noreferrer">${this.tr("Full .tar.gz", "全套 .tar.gz")}</a>` : ""}
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
                    ${dl && safeHref(dl.daemon_linux_x64) !== "#" ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${safeHref(dl.daemon_linux_x64)}" target="_blank" rel="noopener noreferrer">Linux x86_64</a>` : ""}
                    ${dl && safeHref(dl.daemon_linux_arm64) !== "#" ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${safeHref(dl.daemon_linux_arm64)}" target="_blank" rel="noopener noreferrer">Linux ARM64</a>` : ""}
                    ${dl && safeHref(dl.daemon_macos_arm64) !== "#" ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${safeHref(dl.daemon_macos_arm64)}" target="_blank" rel="noopener noreferrer">macOS Daemon</a>` : ""}
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
                        'Open this site in iPhone Safari and choose "Add to Home Screen" for full-screen 2-row tactile terminal keys, Whisper voice, and encrypted relay / LAN access—no App Store gate required.',
                        "使用 iPhone Safari 打开本页面并「添加到主屏幕」，即可零审核全屏使用双排终端触控键盘、Whisper 语音与端到端加密访问。"
                      )}
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${
                      dl?.ios_beta_url && safeHref(dl.ios_beta_url) !== "#"
                        ? `<a class="pm-btn pm-btn-primary pm-btn-sm" href="${safeHref(dl.ios_beta_url)}" target="_blank" rel="noopener noreferrer">${this.tr("Open TestFlight Beta", "打开 TestFlight Beta")}</a>`
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
                    ${dl && safeHref(dl.deploy_bundle) !== "#" ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${safeHref(dl.deploy_bundle)}" target="_blank" rel="noopener noreferrer">${this.tr("Download deploy-bundle", "下载 deploy-bundle")}</a>` : ""}
                    ${dl && safeHref(dl.releases_page) !== "#" ? `<a class="pm-btn pm-btn-secondary pm-btn-sm" href="${safeHref(dl.releases_page)}" target="_blank" rel="noopener noreferrer">GitHub Releases</a>` : ""}
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
                ${dl && safeHref(dl.releases_page) !== "#" ? `<a href="${safeHref(dl.releases_page)}" target="_blank" rel="noopener noreferrer">${this.tr("GitHub Releases (Latest)", "GitHub Releases (最新版)")}</a>` : ""}
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
            <button type="button" class="cmd-row active" id="cmd-copy-install">
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
              dl && safeHref(dl.desktop_macos_dmg) !== "#"
                ? `<a class="cmd-row" href="${safeHref(dl.desktop_macos_dmg)}" target="_blank" rel="noopener noreferrer">
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
      this.cmdPaletteSelectedIdx = 0;
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
    const regQueryParts: string[] = [];
    if (this.authInviteCode) {
      regQueryParts.push(`invite=${encodeURIComponent(this.authInviteCode)}`);
    }
    if (this.pendingActivationCode) {
      regQueryParts.push(`code=${encodeURIComponent(this.pendingActivationCode)}`);
    }
    const regSuffix = regQueryParts.length > 0 ? `?${regQueryParts.join("&")}` : "";

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
              this.authSuccessMsg
                ? `<div style="margin-bottom:14px;padding:10px 12px;border-radius:8px;background:var(--emerald-bg);border:1px solid var(--emerald-bd);color:var(--emerald);font-size:12.5px;">${this.escapeHtml(this.authSuccessMsg)}</div>`
                : ""
            }
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
                  <label class="pm-field-label">${this.tr("Password", "登录密码")} <span>${this.tr("At least 8 characters", "至少 8 位")}</span></label>
                  <div class="pm-input-wrap has-right">
                    <span class="pm-input-icon">${svgIcon("lock", 14)}</span>
                    <input id="auth-password-input" type="${this.showPassword ? "text" : "password"}" name="password" class="pm-input" minlength="8" maxlength="256" required autocomplete="new-password" />
                    <button id="btn-toggle-pw" type="button" class="pm-pw-toggle" title="${this.tr("Show or hide password", "显示或隐藏密码")}">${svgIcon(this.showPassword ? "eyeOff" : "eye", 14)}</button>
                  </div>
                </div>
                ${
                  openReg && !this.authInviteCode
                    ? `<div style="font-size:12px;color:var(--emerald);margin-bottom:14px;display:flex;align-items:center;gap:6px;">${svgIcon("check", 13)} ${this.tr("Open registration is enabled &mdash; no invite code required.", "当前站点已开放自由注册，无需填写邀请码。")}</div>`
                    : `<div class="pm-field">
                        <label class="pm-field-label">${this.tr("Invite Code", "邀请码")} <span>${
                          this.authInviteCode
                            ? this.tr("Pre-filled from invite link", "已从邀请链接自动填入")
                            : this.tr("Leave blank for first admin", "首位管理员留空")
                        }</span></label>
                        <div class="pm-input-wrap">
                          <span class="pm-input-icon">${svgIcon("key", 14)}</span>
                          <input id="auth-invite-input" name="invite_code" class="pm-input coda-mono" value="${this.escapeHtml(this.authInviteCode)}" placeholder="CODA-XXXXXX" />
                        </div>
                      </div>`
                }
              `
                  : `
                <div class="pm-field">
                  <label class="pm-field-label">${this.tr("Email or Handle", "邮箱或用户名")}</label>
                  <div class="pm-input-wrap">
                    <span class="pm-input-icon">${svgIcon("mail", 14)}</span>
                    <input id="auth-login-handle" name="email_or_handle" class="pm-input" value="${this.escapeHtml(this.authPrefillHandle)}" required placeholder="${this.tr("alice or dev@example.com", "alice 或 dev@example.com")}" autocomplete="username" />
                  </div>
                </div>
                <div class="pm-field">
                  <label class="pm-field-label">
                    <span>${this.tr("Password", "密码")}</span>
                    <a href="/reset-password" data-nav="/reset-password" id="link-forgot-password" style="color:var(--signal-blue);font-size:11.5px;font-weight:500;">${this.tr("Forgot password?", "忘记密码？")}</a>
                  </label>
                  <div class="pm-input-wrap has-right">
                    <span class="pm-input-icon">${svgIcon("lock", 14)}</span>
                    <input id="auth-password-input" type="${this.showPassword ? "text" : "password"}" name="password" class="pm-input" required autocomplete="current-password" />
                    <button id="btn-toggle-pw" type="button" class="pm-pw-toggle" title="${this.tr("Show or hide password", "显示或隐藏密码")}">${svgIcon(this.showPassword ? "eyeOff" : "eye", 14)}</button>
                  </div>
                  <div style="font-size:11px;color:var(--text-tertiary);margin-top:5px;">
                    ${this.tr(
                      "Forgot your password? Ask the site admin for a one-time reset link/token and paste it on /reset-password.",
                      "忘记密码？可让站点管理员一键复制单次重置链接或令牌，前往 /reset-password 重置。"
                    )}
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

    const inviteInp = document.getElementById("auth-invite-input") as HTMLInputElement | null;
    inviteInp?.addEventListener("input", () => {
      this.authInviteCode = inviteInp.value.trim();
    });
    const loginHandleInp = document.getElementById("auth-login-handle") as HTMLInputElement | null;
    loginHandleInp?.addEventListener("input", () => {
      this.authPrefillHandle = loginHandleInp.value;
    });

    document.getElementById("tab-auth-login")?.addEventListener("click", () => {
      this.authMode = "login";
      this.navigate(`/login${codeSuffix}`);
    });
    document.getElementById("tab-auth-register")?.addEventListener("click", () => {
      this.authMode = "register";
      this.navigate(`/register${regSuffix}`);
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
      if (isReg) {
        const pwCheck = validateNewPasswordLength(
          String(payload.password || ""),
          (en, zh) => this.tr(en, zh)
        );
        if (!pwCheck.ok) {
          this.errorMsg = pwCheck.error;
          this.render();
          return;
        }
      }
      try {
        const res = isReg
          ? await this.api("/api/v1/auth/register", "POST", payload)
          : await this.api("/api/v1/auth/login", "POST", payload);
        this.token = res.token;
        this.user = res.user;
        this.syncUserScopedPrefs();
        this.errorMsg = "";
        this.authSuccessMsg = "";
        this.authInviteCode = "";
        saveSessionToken(res.token);
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

  private renderResetPasswordView(root: HTMLElement, baseUrl: string, ver: string) {
    root.innerHTML = `
      <div class="pm-auth-shell">
        <div class="pm-auth-main">
          <div class="pm-auth-topbar">
            <a href="/login" data-nav="/login" class="pm-btn pm-btn-ghost pm-btn-sm">
              ${svgIcon("arrowLeft", 14)} ${this.tr("Back to Sign In", "返回账号登录")}
            </a>
            <div style="display:flex;align-items:center;gap:8px;">
              ${this.renderLangSwitcher()}
              ${this.renderThemeSwitcher()}
            </div>
          </div>

          <div class="pm-auth-box" style="max-width:430px;">
            <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;">
              <span class="pm-badge blue">${svgIcon("key", 12)} ${this.tr("One-Time Reset Link", "单次密码重置令牌")}</span>
              <span class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">SHA-256 · 24h</span>
            </div>

            <h1 style="font-size:24px;font-weight:600;letter-spacing:-0.025em;margin-bottom:8px;">
              ${this.tr("Reset Forgotten Password", "重置你的 Coda 登录密码")}
            </h1>
            <p style="font-size:13px;color:var(--text-secondary);line-height:1.65;margin-bottom:18px;">
              ${this.tr(
                "Paste the one-time reset link or <code>rst_...</code> token sent by your site admin, then set a new password (paired host daemons stay intact).",
                "粘贴站点管理员发给你的一次性重置链接或 <code>rst_...</code> 令牌，并设置新密码（已绑定的远端主机守护进程不受影响）。"
              )}
            </p>

            ${
              this.errorMsg
                ? `<div style="margin-bottom:14px;padding:10px 12px;border-radius:8px;background:var(--rose-bg);border:1px solid var(--rose-bd);color:var(--rose);font-size:12.5px;">${this.escapeHtml(this.errorMsg)}</div>`
                : ""
            }

            <form id="reset-pw-form">
              <div class="pm-field">
                <label class="pm-field-label">${this.tr("Reset Token or Link", "重置令牌或完整链接")}</label>
                <div class="pm-input-wrap">
                  <span class="pm-input-icon">${svgIcon("key", 14)}</span>
                  <input
                    id="reset-token-input"
                    name="token"
                    class="pm-input coda-mono"
                    value="${this.escapeHtml(this.resetPasswordToken)}"
                    required
                    placeholder="rst_... / https://.../reset-password?token=rst_..."
                    autocomplete="off"
                  />
                </div>
              </div>
              <div class="pm-field">
                <label class="pm-field-label">${this.tr("New Password", "新密码")} <span>${this.tr("At least 8 characters", "至少 8 位")}</span></label>
                <div class="pm-input-wrap has-right">
                  <span class="pm-input-icon">${svgIcon("lock", 14)}</span>
                  <input
                    id="reset-new-pw"
                    type="${this.showResetPassword ? "text" : "password"}"
                    name="new_password"
                    class="pm-input"
                    minlength="8"
                    maxlength="256"
                    required
                    autocomplete="new-password"
                  />
                  <button id="btn-toggle-reset-pw" type="button" class="pm-pw-toggle" title="${this.tr("Show or hide password", "显示或隐藏密码")}">${svgIcon(this.showResetPassword ? "eyeOff" : "eye", 14)}</button>
                </div>
              </div>
              <div class="pm-field">
                <label class="pm-field-label">${this.tr("Confirm Password", "确认新密码")}</label>
                <div class="pm-input-wrap">
                  <span class="pm-input-icon">${svgIcon("lock", 14)}</span>
                  <input
                    id="reset-confirm-pw"
                    type="${this.showResetPassword ? "text" : "password"}"
                    name="confirm_password"
                    class="pm-input"
                    minlength="8"
                    maxlength="256"
                    required
                    autocomplete="new-password"
                  />
                </div>
              </div>
              <button type="submit" class="pm-btn pm-btn-primary pm-btn-lg" style="width:100%;margin-top:4px;" ${this.resetPasswordSubmitting ? "disabled" : ""}>
                ${
                  this.resetPasswordSubmitting
                    ? this.tr("Resetting Password…", "正在重置密码…")
                    : `${this.tr("Reset Password & Continue to Sign In", "确认重置并前往登录")} ${svgIcon("arrowRight", 14)}`
                }
              </button>
            </form>
          </div>
        </div>

        ${this.renderAuthShowcasePanel(baseUrl, ver, false)}
      </div>
    `;

    this.bindCommonNavigation(root);

    const tokenInp = document.getElementById("reset-token-input") as HTMLInputElement | null;
    tokenInp?.addEventListener("input", () => {
      this.resetPasswordToken = tokenInp.value;
    });

    document.getElementById("btn-toggle-reset-pw")?.addEventListener("click", () => {
      this.showResetPassword = !this.showResetPassword;
      const newInp = document.getElementById("reset-new-pw") as HTMLInputElement | null;
      const confInp = document.getElementById("reset-confirm-pw") as HTMLInputElement | null;
      const btn = document.getElementById("btn-toggle-reset-pw");
      const nextType = this.showResetPassword ? "text" : "password";
      if (newInp) newInp.type = nextType;
      if (confInp) confInp.type = nextType;
      if (btn) btn.innerHTML = svgIcon(this.showResetPassword ? "eyeOff" : "eye", 14);
    });

    document.getElementById("reset-pw-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (this.resetPasswordSubmitting) return;
      const fd = new FormData(e.target as HTMLFormElement);
      const rawToken = String(fd.get("token") || "");
      const newPw = String(fd.get("new_password") || "");
      const confirmPw = String(fd.get("confirm_password") || "");

      const validated = validateResetPasswordInputs(rawToken, newPw, confirmPw, (en, zh) =>
        this.tr(en, zh)
      );
      if (!validated.ok) {
        this.errorMsg = validated.error;
        this.render();
        return;
      }

      this.resetPasswordSubmitting = true;
      this.errorMsg = "";
      this.render();

      try {
        const res = await this.api("/api/v1/auth/reset-password", "POST", {
          token: validated.token,
          new_password: validated.newPassword,
        });
        const handle = String(res?.handle || "").trim();
        if (this.user && (!handle || this.user.handle === handle)) {
          this.stopAllTransportsAndTimers();
          clearSessionToken();
          this.token = null;
          this.user = null;
        }
        this.resetPasswordToken = "";
        this.authPrefillHandle = handle;
        const okMsg = handle
          ? this.tr(
              `Password reset for @${handle}! Please sign in with your new password.`,
              `已成功重置 @${handle} 的登录密码，请使用新密码登录。`
            )
          : this.tr(
              "Password reset! Please sign in with your new password.",
              "密码已重置，请使用新密码登录。"
            );
        this.authSuccessMsg = okMsg;
        this.showToastNotice("ok", okMsg);
        this.resetPasswordSubmitting = false;
        this.navigate("/login", false, true);
        setTimeout(() => {
          (document.getElementById("auth-password-input") as HTMLInputElement | null)?.focus();
        }, 0);
      } catch (err: any) {
        this.resetPasswordSubmitting = false;
        this.errorMsg = String(
          err?.message || this.tr("Failed resetting password", "重置密码失败，请检查令牌是否过期")
        );
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
                <button id="btn-cancel-activate" type="button" class="pm-btn pm-btn-secondary pm-btn-lg" style="width:100%;">
                  ${this.tr("Cancel &amp; Return to Console", "取消并返回在线控制台")}
                </button>
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

    document.getElementById("btn-cancel-activate")?.addEventListener("click", () => {
      this.pendingActivationCode = "";
      this.pendingDeviceInfo = null;
      this.errorMsg = "";
      this.navigate("/console");
    });

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
        const approved = this.devices.find((d) => d.id === dev.id) || null;
        if (approved) {
          this.selectDaemon(approved);
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
    // V3-22: Capture previous #term-box scroll position BEFORE tearing down DOM with root.innerHTML.
    const prevTermBox = document.getElementById("term-box");
    const prevScrollTop = prevTermBox ? prevTermBox.scrollTop : 0;
    const prevWasNearBottom = prevTermBox
      ? prevTermBox.scrollHeight - prevTermBox.scrollTop - prevTermBox.clientHeight <= 28
      : true;
    const activeTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr) || this.terminals[0];
    const nowTime = new Date().toTimeString().slice(0, 5);
    const isNarrowViewport = window.innerWidth < 900;
    const renderTouchKeypad = isNarrowViewport || this.showMobileKeypadOnDesktop;
    const allBranches = this.repos.flatMap((r) => r.branches || []);
    const branchForTerm = (t?: HerdrTerminal): BranchWorktreeItem | undefined => {
      if (!t) return undefined;
      return allBranches.find(
        (b) =>
          b.active_terminals?.some((bt) => bt.address === t.address) ||
          Boolean(t.cwd && b.worktree_path && (t.cwd === b.worktree_path || t.cwd.startsWith(`${b.worktree_path}/`)))
      );
    };
    const activeBranch = branchForTerm(activeTerm);
    const selectedVtKey =
      this.selectedDaemon && activeTerm ? this.vtKey(this.selectedDaemon.id, activeTerm.address) : "";
    const switchedTerminal = selectedVtKey !== this.lastRenderedTermKey;
    this.lastRenderedTermKey = selectedVtKey;
    const activeVt = selectedVtKey ? this.vtTerminals.get(selectedVtKey) : undefined;
    const isVtStream = Boolean(activeVt && this.vtStreamActive.has(selectedVtKey));
    const showPrimaryCursor = !(
      this.splitMode &&
      !isNarrowViewport &&
      this.focusedPane === "secondary" &&
      this.secondaryTerminalAddr
    );
    const focusedAddr = this.getFocusedTerminalAddr() || activeTerm?.address || null;
    const focusedTerm = this.terminals.find((t) => t.address === focusedAddr) || activeTerm;
    const focusedVt =
      this.selectedDaemon && focusedAddr
        ? this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, focusedAddr))
        : activeVt;
    const initialVtHtml =
      isVtStream && activeVt
        ? activeVt.renderInitialRowsHtml(showPrimaryCursor)
        : this.computeConsoleEmptyStateHtml(installCmd, activeTerm);
    const connVisual = this.computeConsoleConnectionVisual();
    const footerTransport = connVisual.footerTransport;
    const headerSubtitleHtml = formatConsoleHeaderSubtitle({
      devicesCount: this.devices.length,
      selectedDaemon: this.selectedDaemon,
      activeCwd: activeTerm?.cwd,
      activeBranch: activeBranch?.branch,
      tr: (en, zh) => this.tr(en, zh),
    });

    const feedbackBanner = this.settingsFeedback
      ? `<div style="padding:9px 11px;border-radius:8px;font-size:12px;margin-bottom:10px;border:1px solid ${
          this.settingsFeedback.kind === "err" ? "var(--rose-bd)" : "var(--emerald-bd)"
        };background:${
          this.settingsFeedback.kind === "err" ? "var(--rose-bg)" : "var(--emerald-bg)"
        };color:${this.settingsFeedback.kind === "err" ? "var(--rose)" : "var(--emerald)"};">${this.escapeHtml(
          this.settingsFeedback.text
        )}</div>`
      : this.activationSuccessMsg
        ? `<div style="background:var(--emerald-bg);border:1px solid var(--emerald-bd);color:var(--emerald);padding:9px 11px;border-radius:8px;font-size:12px;margin-bottom:10px;">${this.escapeHtml(
            this.activationSuccessMsg
          )}</div>`
        : "";

    const micBtnClass = this.isRecordingVoice ? "is-recording" : "";
    const micLabel = this.isTranscribingVoice
      ? this.tr("Transcribing…", "转写中…")
      : this.isRecordingVoice
        ? this.tr("Stop", "停止")
        : "";
    const needsAttentionItems = collectNeedsAttentionItems(
      this.devices,
      this.terminalsByPeer,
      this.selectedDaemon?.id ?? null,
      this.terminals
    );
    const offlineWebClientCount = this.allDevices.filter(
      (d) =>
        d.platform === "web" &&
        d.role === "client" &&
        !d.is_online &&
        d.id !== this.myClientDevice?.id
    ).length;
    const effectiveVoiceKey = (this.voiceApiKeyLocal || this.voiceConfig.api_key || "").trim();
    const hasWhisperKey = Boolean(
      effectiveVoiceKey && (!effectiveVoiceKey.startsWith("••") || this.selectedDaemon?.is_online)
    );
    const hasSpeechFallback = this.hasBrowserSpeechRecognition();
    const composerUseBracketedPaste = resolveEffectiveBracketedPaste({
      bracketedPaste: focusedVt?.bracketedPaste,
      bracketedPasteExplicit: focusedVt?.bracketedPasteExplicit,
      term: focusedTerm,
    });
    const anyModalOpen = this.isAnyModalOpen();
    const isComposerFocused =
      typeof document !== "undefined" && document.activeElement?.id === "prompt-composer-textarea";
    const workspaceRoots = [
      ...(activeTerm?.cwd ? [activeTerm.cwd] : []),
      ...this.repos.map((r) => r.root_path),
      ...allBranches.map((b) => b.worktree_path),
    ];

    root.innerHTML = `
      <div class="shell-root${isComposerFocused ? " is-composer-focused" : ""}"${anyModalOpen ? " inert" : ""}>
        ${
          this.showSidebar && isNarrowViewport
            ? `<div class="sidebar-backdrop" id="sidebar-backdrop"></div>`
            : ""
        }
        ${
          this.showSidebar
            ? `
          <!-- LEFT SIDEBAR: Account + Pinned Needs-Attention Inbox + Machine/Repo/Branch List -->
          <aside class="left-sidebar ${isNarrowViewport ? "drawer-mobile" : ""}">
            <div style="padding:14px 16px;display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid var(--border-primary);">
              <div style="display:flex;align-items:center;gap:10px;min-width:0;">
                ${codaLogoSVG(24)}
                <div style="min-width:0;">
                  <div style="font-size:13px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${this.escapeHtml(this.user.email)}</div>
                  <div id="console-sidebar-conn" style="font-size:11px;color:${connVisual.sidebarColorCss};margin-top:2px;display:inline-flex;align-items:center;gap:4px;">
                    <span id="console-sidebar-conn-icon" style="display:inline-flex;align-items:center;">${svgIcon(connVisual.icon, 11)}</span>
                    <span id="console-rtt-label">${this.escapeHtml(connVisual.sidebarLabel)}</span>
                  </div>
                </div>
              </div>
              <div style="display:flex;gap:6px;">
                <button id="btn-open-settings" class="circle-btn" title="${this.tr("Devices, webhooks, security & settings", "设备管理、离线推送、安全与设置")}">${svgIcon("settings", 14)}</button>
                <button id="btn-collapse-sidebar" class="circle-btn" title="${this.tr("Collapse sidebar", "收起侧边栏")}">${svgIcon("sidebar", 14)}</button>
              </div>
            </div>

            <div style="flex:1;overflow-y:auto;padding:12px;">
              ${renderNeedsAttentionSidebarSection(
                needsAttentionItems,
                this.selectedDaemon?.id ?? null,
                this.activeTerminalAddr,
                (en, zh) => this.tr(en, zh)
              )}
              ${
                this.devices.length === 0
                  ? `<div style="padding:12px;font-size:12px;color:var(--text-secondary);line-height:1.65;">
                      ${this.tr("No hosts paired to this account yet.", "当前账号下暂无在线主机。")}<br/><br/>
                      ${this.tr("Run on any Linux or macOS host:", "在任意 Linux / Mac 运行：")}<br/>
                      <code class="coda-mono" style="color:var(--emerald);word-break:break-all;">${this.escapeHtml(installCmd)}</code><br/><br/>
                      ${this.tr("Open the printed activation URL or click Settings at the top-right to approve.", "打开终端输出的激活链接即可接入，或点击右上角设置按钮手动授权。")}
                      ${
                        this.onboardingDismissed
                          ? `<div style="margin-top:10px;"><button type="button" id="btn-sidebar-show-onboarding" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("Show 3-Step Quickstart", "重新展开三步新手引导")}</button></div>`
                          : ""
                      }
                    </div>`
                  : this.devices
                      .map(
                        (d) => `
                  <div style="margin-bottom:18px;">
                    <div style="display:flex;align-items:center;gap:6px;padding:4px 6px;font-size:12px;color:var(--text-tertiary);cursor:pointer;" data-dev="${this.escapeHtml(d.id)}">
                      <span style="width:7px;height:7px;border-radius:99px;background:${d.is_online ? "var(--emerald)" : "var(--text-quaternary)"};"></span>
                      <span style="color:var(--text-primary);font-weight:500;">${this.escapeHtml(d.platform || "host")}</span>
                      <span>&middot; ${this.escapeHtml(d.name)}</span>
                    </div>
                    ${
                      this.hostHealthByPeer.has(d.id)
                        ? `<div class="coda-mono" style="padding:0 6px 4px 19px;font-size:10.5px;color:var(--text-tertiary);">${this.escapeHtml(formatWebHostHealth(this.hostHealthByPeer.get(d.id)))}</div>`
                        : ""
                    }
                    ${
                      this.selectedDaemon?.id === d.id && this.terminals.length > 0
                        ? this.terminals
                            .map((t) => {
                              const b = branchForTerm(t);
                              const isNeeds = this.isNeedsYouStatus(t.status);
                              const rightMeta =
                                b && (b.diff_added > 0 || b.diff_deleted > 0)
                                  ? `<div class="coda-mono pm-num" style="font-size:11px;color:var(--emerald);">+${b.diff_added} <span style="color:var(--rose);">-${b.diff_deleted}</span></div>`
                                  : `<div class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">${this.escapeHtml(
                                      b?.branch || (t.alive ? this.tr("live", "运行") : this.tr("exited", "已退出"))
                                    )}</div>`;
                              return `
                          <div class="branch-item ${this.activeTerminalAddr === t.address ? "active" : ""}" data-term="${this.escapeHtml(t.address)}">
                            <div style="display:flex;align-items:center;gap:10px;min-width:0;flex:1;">
                              ${codaNodeDotSVG(isNeeds ? "#f59e0b" : t.alive ? "#e06c50" : "#52525b")}
                              <div style="min-width:0;">
                                <div class="coda-mono" style="font-size:13px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                                  ${this.escapeHtml(t.name || t.address)}
                                </div>
                                <div style="font-size:11px;color:${isNeeds ? "var(--amber)" : "var(--emerald)"};margin-top:2px;">
                                  ${this.escapeHtml(t.status?.state || this.tr("Running", "终端运行中"))}
                                </div>
                              </div>
                            </div>
                            <div style="display:flex;align-items:center;gap:6px;flex-shrink:0;">
                              ${rightMeta}
                              <button type="button" data-kill-term="${this.escapeHtml(t.address)}" title="${this.tr("Close & terminate PTY", "终止并关闭终端")}" style="background:none;border:none;color:var(--text-tertiary);cursor:pointer;padding:2px 4px;border-radius:4px;font-size:13px;line-height:1;">&times;</button>
                            </div>
                          </div>
                        `;
                            })
                            .join("")
                        : `
                        <div class="branch-item ${this.selectedDaemon?.id === d.id ? "active" : ""}" data-dev="${this.escapeHtml(d.id)}">
                          <div style="display:flex;align-items:center;gap:10px;">
                            ${codaNodeDotSVG(d.is_online ? "#e06c50" : "#52525b")}
                            <div>
                              <div class="coda-mono" style="font-size:13px;">${this.escapeHtml(d.name)}</div>
                              <div style="font-size:11px;color:${d.is_online ? "var(--emerald)" : "var(--text-tertiary)"};">
                                ${d.is_online ? this.tr("Click to load Herdr PTYs", "点击加载 Herdr 终端") : this.tr("Host offline", "主机离线")}
                              </div>
                            </div>
                          </div>
                        </div>
                      `
                    }
                    ${
                      this.selectedDaemon?.id === d.id && this.repos.length > 0
                        ? this.repos
                            .slice(0, 6)
                            .map((r) => {
                              const sumAdded = (r.branches || []).reduce((acc, b) => acc + (b.diff_added || 0), 0);
                              const sumDel = (r.branches || []).reduce((acc, b) => acc + (b.diff_deleted || 0), 0);
                              return `
                          <div style="padding:4px 8px;margin-top:4px;font-size:11px;color:var(--text-tertiary);display:flex;justify-content:space-between;gap:8px;">
                            <span class="coda-mono" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${this.escapeHtml(r.repo_name)}</span>
                            <span class="coda-mono" style="color:var(--emerald);">+${sumAdded} <span style="color:var(--rose);">-${sumDel}</span></span>
                          </div>`;
                            })
                            .join("")
                        : ""
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
              <div id="active-term-title" style="font-size:15px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                ${this.escapeHtml(activeTerm?.status?.title || activeTerm?.name || this.tr("Coda Interactive AI Terminal", "Coda 交互式 AI 终端会话"))}
              </div>
              <div id="active-term-subtitle" class="coda-mono" style="font-size:12px;color:var(--text-tertiary);margin-top:2px;">
                ${headerSubtitleHtml}
              </div>
            </div>
            <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
              ${
                needsAttentionItems.length > 0
                  ? `<button type="button" id="btn-topbar-needs-you" class="pm-cmd-trigger" style="height:30px;padding:0 9px;border-color:var(--amber-bd);background:var(--amber-bg);color:var(--amber);" title="${this.tr("Jump to next Needs-You terminal (⌘J / Ctrl+Shift+J)", "跳转至下一个待审批终端 (⌘J / Ctrl+Shift+J)")}">
                      <span class="needs-you-dot"></span>
                      <span class="coda-mono">${needsAttentionItems.length}</span>
                      <span class="pm-kbd">⌘J</span>
                    </button>`
                  : ""
              }
              ${
                !isNarrowViewport
                  ? `<button type="button" id="btn-console-split" class="pm-cmd-trigger" style="height:30px;padding:0 9px;${this.splitMode ? "border-color:var(--signal-blue);color:var(--text-primary);" : ""}" title="${this.tr("Toggle Dual-Pane Split Terminal (⌘D / Ctrl+Shift+D)", "切换双栏分屏终端 (⌘D / Ctrl+Shift+D)")}">
                      <span>◫</span>
                      <span class="pm-cmd-label">${this.tr("Split", "分屏")}</span>
                      <span class="pm-kbd">⌘D</span>
                    </button>`
                  : ""
              }
              <button type="button" id="btn-console-diff" class="pm-cmd-trigger" style="height:30px;padding:0 9px;" title="${this.tr("Review Remote Git Changes & Diff over E2EE", "端到端加密审查远端 Git 变更与 Diff")}">
                ${svgIcon("gitBranch", 13)}
                <span class="pm-cmd-label">${this.tr("Diff", "变更")}</span>
              </button>
              <button type="button" id="btn-console-files" class="pm-cmd-trigger" style="height:30px;padding:0 9px;" title="${this.tr("Browse Remote Workspace Files (E2EE)", "远端工作区文件树 (端到端加密)")}">
                ${svgIcon("layers", 13)}
                <span class="pm-cmd-label">${this.tr("Files", "文件")}</span>
              </button>
              <button type="button" id="btn-console-port-preview" class="pm-cmd-trigger" style="height:30px;padding:0 9px;" title="${this.tr("Preview Remote localhost Port over E2EE", "远端 localhost 端口加密预览")}">
                ${svgIcon("globe", 13)}
                <span class="pm-cmd-label">${this.tr("Port", "端口")}</span>
              </button>
              <button type="button" id="btn-console-search" class="pm-cmd-trigger" style="height:30px;padding:0 9px;${this.showTermSearch ? "border-color:var(--amber);color:var(--text-primary);" : ""}" title="${this.tr("Find in Terminal & Export .md (⌘F)", "终端内搜索与导出 .md (⌘F)")}">
                ${svgIcon("search", 13)}
                <span class="pm-kbd">⌘F</span>
              </button>
              <button type="button" id="btn-console-cmd-k" class="pm-cmd-trigger" style="height:30px;padding:0 9px;" title="${this.tr("Quick Jump & Actions (⌘K / Ctrl+K)", "全局快捷跳转面板 (⌘K / Ctrl+K)")}">
                ${svgIcon("search", 13)}
                <span class="pm-kbd">⌘K</span>
              </button>
              ${this.terminals
                .map(
                  (t, i) => `
                <button data-term="${this.escapeHtml(t.address)}" title="${this.escapeHtml(t.name || t.address)}" style="margin-left:${i === 0 ? "0" : "-8px"};width:34px;height:34px;border-radius:999px;background:var(--bg-card);border:${
                    t.address === this.activeTerminalAddr ? "2px solid var(--text-primary)" : "1px solid var(--border-strong)"
                  };display:flex;align-items:center;justify-content:center;cursor:pointer;z-index:${10 - i};">
                  ${codaNodeDotSVG(this.isNeedsYouStatus(t.status) ? "#f59e0b" : i % 2 === 0 ? "#e06c50" : "#3b82f6")}
                </button>
              `
                )
                .join("")}
            </div>
          </div>

          ${
            this.devices.length === 0 && !this.onboardingDismissed
              ? renderFriendOnboardingCard({
                  installCmd,
                  copiedInstall: this.onboardingCopiedInstall,
                  thisDevicePushSubscribed: this.webPushThisDeviceSubscribed,
                  webPushCount: this.webPushCount,
                  webPushBusy: this.webPushBusy,
                  iosNeedsHomeScreenTip: detectBrowserIosNonStandalone(),
                  userHandle: this.user.handle,
                  tr: (en, zh) => this.tr(en, zh),
                })
              : ""
          }

          ${
            this.showTermSearch
              ? `<div class="term-search-strip" id="term-search-strip">
                  ${svgIcon("search", 13)}
                  <input id="term-search-input" class="pm-input" style="margin:0;border:none;background:transparent;padding:2px 4px;flex:1;min-width:90px;box-shadow:none;height:24px;" value="${this.escapeHtml(this.termSearchQuery)}" placeholder="${this.tr("Find in scrollback…", "在滚动缓存中搜索…")}" autocomplete="off" />
                  <span id="term-search-count" class="coda-mono" style="font-size:11px;color:var(--text-tertiary);">0/0</span>
                  <button type="button" id="term-search-prev" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 7px;" title="${this.tr("Previous match", "上一个匹配")}">&uarr;</button>
                  <button type="button" id="term-search-next" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 7px;" title="${this.tr("Next match", "下一个匹配")}">&darr;</button>
                  <button type="button" id="btn-export-session-md" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 8px;">${this.tr("Export .md", "导出 .md")}</button>
                  <button type="button" id="term-search-close" class="pm-btn pm-btn-ghost pm-btn-sm" style="height:24px;padding:0 6px;">&times;</button>
                </div>`
              : ""
          }

          <!-- Live Terminal Viewport (focusable for direct keyboard input, supports drag-drop & paste upload, optional desktop split) -->
          <div class="term-split-grid ${this.splitMode && !isNarrowViewport ? "is-split" : ""}">
            <div class="term-viewport-wrap">
              <div id="console-floating-toast-slot">${this.renderFloatingToastHtml()}</div>
              <div class="term-viewport ${showPrimaryCursor ? "pane-focused" : ""}" id="term-box" tabindex="${anyModalOpen ? "-1" : "0"}" aria-label="${this.tr("Interactive Terminal Viewport", "交互式终端视口")}"><div id="console-notice-slot">${this.renderConsoleNoticeHtml()}</div><div id="term-screen-output" data-vt-mode="${isVtStream ? "rows" : "placeholder"}" data-vt-evictions="${activeVt?.getScrollbackEvictions() ?? 0}" data-vt-alt="${activeVt?.isUsingAltScreen() ? "1" : "0"}" style="white-space:pre;overflow-x:auto;">${initialVtHtml}</div>
<div id="term-draft-slot">${this.renderDraftPreviewHtml()}</div>
<div id="term-footer-meta" style="font-size:12px;color:var(--text-tertiary);margin-top:8px;">${this.escapeHtml(footerTransport)} &middot; E2EE X25519 + ChaCha20 &middot; ${this.termCols}&times;${this.termRows} &middot; ${this.terminals.length} PTY &middot; ${this.tr("⌘+Click URL · Ctrl+V/⌘V screenshot to upload", "⌘+点击打开链接 · Ctrl+V/⌘V 粘贴截图直传终端")}</div></div>
              <button type="button" id="btn-scroll-bottom" class="term-scroll-bottom-btn" title="${this.tr("Jump to latest output", "回到底部最新输出")}">
                ↓ ${this.tr("Latest", "回到最新")}
              </button>
            </div>
            ${
              this.splitMode && !isNarrowViewport
                ? (() => {
                    const secTerm = this.terminals.find((t) => t.address === this.secondaryTerminalAddr);
                    const secKey = this.selectedDaemon && secTerm ? this.vtKey(this.selectedDaemon.id, secTerm.address) : "";
                    const secVt = secKey ? this.vtTerminals.get(secKey) : undefined;
                    const secIsStream = Boolean(secVt && this.vtStreamActive.has(secKey));
                    const secFocused = Boolean(secTerm && this.focusedPane === "secondary");
                    const secHtml = secIsStream && secVt
                      ? secVt.renderInitialRowsHtml(secFocused)
                      : this.escapeHtml(
                          secTerm
                            ? this.tr("Connecting secondary split terminal…", "正在连接副分屏终端…")
                            : this.tr("Launch a second terminal ('+' on right rail) to view side-by-side.", "点击右侧「+」新建第二个终端即可并排对比查看。")
                        );
                    return `
            <div class="term-viewport-wrap">
              <div class="term-viewport ${secFocused ? "pane-focused" : ""}" id="term-box-secondary" tabindex="${anyModalOpen ? "-1" : "0"}" aria-label="${this.tr("Secondary Split Terminal", "副分屏终端")}">
                <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;padding-bottom:6px;margin-bottom:6px;border-bottom:1px solid var(--border-primary);font-size:11.5px;">
                  <span class="coda-mono" style="color:var(--text-secondary);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${this.escapeHtml(secTerm?.name || secTerm?.address || this.tr("Secondary Pane", "副分屏"))}</span>
                  <div style="display:flex;gap:6px;align-items:center;">
                    ${
                      this.terminals.filter((t) => t.address !== this.activeTerminalAddr).length > 1
                        ? `<select id="split-secondary-select" class="pm-input coda-mono" style="margin:0;height:22px;padding:0 6px;font-size:11px;width:auto;">
                            ${this.terminals
                              .filter((t) => t.address !== this.activeTerminalAddr)
                              .map(
                                (t) =>
                                  `<option value="${this.escapeHtml(t.address)}" ${t.address === this.secondaryTerminalAddr ? "selected" : ""}>${this.escapeHtml(t.name || t.address)}</option>`
                              )
                              .join("")}
                          </select>`
                        : ""
                    }
                    ${
                      secTerm
                        ? `<button type="button" id="btn-swap-split" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:22px;padding:0 7px;font-size:11px;">&harr; ${this.tr("Focus", "主焦点")}</button>`
                        : ""
                    }
                  </div>
                </div>
                <div id="term-screen-output-secondary" data-vt-mode="${secIsStream ? "rows" : "placeholder"}" data-vt-evictions="${secVt?.getScrollbackEvictions() ?? 0}" data-vt-alt="${secVt?.isUsingAltScreen() ? "1" : "0"}" style="white-space:pre;overflow-x:auto;">${secHtml}</div>
              </div>
            </div>`;
                  })()
                : ""
            }
          </div>

          <!-- OSC 7501 Needs-You One-Tap Approval Capsule Bar -->
          ${renderNeedsYouCapsuleBar(focusedTerm, (en, zh) => this.tr(en, zh))}

          <!-- Toggleable Multi-Line Prompt Composer Drawer -->
          ${renderPromptComposerPanel({
            open: this.showPromptComposer,
            draft: this.draftInput,
            isRecordingVoice: this.isRecordingVoice,
            isTranscribingVoice: this.isTranscribingVoice,
            hasSpeechFallback,
            hasWhisperKey,
            useBracketedPaste: composerUseBracketedPaste,
            snippets: this.promptSnippets,
            savingSnippetOpen: this.savingSnippetOpen,
            savingSnippetTitle: this.savingSnippetTitle,
            savingSnippetBusy: this.savingSnippetBusy,
            tr: (en, zh) => this.tr(en, zh),
          })}

          ${
            this.showKeyboardInputBar && !this.showPromptComposer
              ? `
            <form id="kb-form" style="display:flex;gap:8px;margin-bottom:8px;">
              <input id="kb-input" class="pm-input" style="margin:0;flex:1;" value="${this.escapeHtml(
                this.draftInput
              )}" placeholder="${this.tr("Type command or prompt (sends directly)…", "输入命令或中文 Prompt（回车直发终端）…")}" autofocus />
              <button type="submit" class="pm-btn pm-btn-primary">${this.tr("Send", "发送")}</button>
            </form>
          `
              : ""
          }

          ${
            renderTouchKeypad
              ? `
          <!-- Mobile Quick-Command Chips + 2-Row Tactile Keypad + Tall Enter -->
          <div class="quick-chip-strip" role="toolbar" aria-label="${this.tr("Quick terminal commands", "快捷终端指令")}">
            <button type="button" id="btn-chip-composer" class="quick-chip-btn" style="${
              this.showPromptComposer ? "border-color:var(--signal-blue);color:var(--text-primary);" : ""
            }">✎ ${this.tr("Composer", "多行编排")}</button>
            <button type="button" id="btn-chip-diff" class="quick-chip-btn">${this.tr("Changes", "Git变更")}</button>
            ${MOBILE_QUICK_CHIPS.map(
              (chip) =>
                `<button type="button" class="quick-chip-btn" data-quick-cmd="${this.escapeHtml(chip)}">${this.escapeHtml(chip)}</button>`
            ).join("")}
          </div>
          ${
            !isNarrowViewport && this.showMobileKeypadOnDesktop
              ? `<div style="display:flex;justify-content:flex-end;margin-bottom:4px;">
                  <button type="button" id="btn-hide-touch-pad" class="pm-btn pm-btn-ghost pm-btn-sm">${svgIcon("keyboard", 13)} ${this.tr("Back to Desktop Input Bar", "切回桌面输入框")}</button>
                </div>`
              : ""
          }
          <div class="keypad-grid">
            <div class="keypad-left">
              <div class="keypad-row">
                <button type="button" data-k="esc" class="k-btn" style="grid-column:span 2;">esc</button>
                <button type="button" data-k="tab" class="k-btn" style="grid-column:span 2;">tab</button>
                <button type="button" id="btn-kb-toggle" class="k-btn" style="grid-column:span 2;" title="${this.tr("Toggle Multi-Line Prompt Composer", "展开/收起多行 Prompt 编排器")}">${svgIcon("keyboard", 14)}</button>
                <button type="button" id="btn-voice-mic" class="k-btn ${micBtnClass}" style="grid-column:span 1;" title="${this.tr("Voice input (Whisper / Zero-Config Speech)", "语音输入 (Whisper / 免配置实时语音)")}">${svgIcon("mic", 14)}</button>
                <button type="button" data-k="up" class="k-btn" style="grid-column:span 1;">↑</button>
                <button type="button" data-k="backspace" class="k-btn" style="grid-column:span 2;">⌫</button>
              </div>
              <div class="keypad-row">
                <button type="button" data-k="ctrl+c" class="k-btn" style="grid-column:span 2;color:var(--coral);font-weight:600;">^C</button>
                <button type="button" data-k="ctrl+d" class="k-btn" style="grid-column:span 2;font-weight:600;">^D</button>
                <button type="button" data-k="slash" class="k-btn" style="grid-column:span 2;font-weight:600;">/</button>
                <button type="button" data-k="left" class="k-btn" style="grid-column:span 1;">←</button>
                <button type="button" data-k="down" class="k-btn" style="grid-column:span 2;">↓</button>
                <button type="button" data-k="right" class="k-btn" style="grid-column:span 1;">→</button>
              </div>
            </div>
            <button type="button" data-k="enter" class="k-enter">↵</button>
          </div>`
              : !this.showPromptComposer
                ? `
          <!-- Desktop Prompt Composer Bar -->
          <form id="desktop-prompt-form" class="desktop-composer">
            <div class="desktop-composer-row">
              <span class="coda-mono" style="color:var(--emerald);font-weight:700;">&gt;</span>
              <input id="desktop-prompt-input" class="desktop-composer-inp" value="${this.escapeHtml(
                this.draftInput
              )}" placeholder="${this.tr("Send command or prompt (or click Composer for multi-line Bracketed Paste)...", "发送单行指令（或点击右侧「多行」打开多行 Prompt 编排器）...")}" autocomplete="off" />
              <button type="button" id="btn-open-composer" class="pm-btn pm-btn-secondary pm-btn-sm" title="${this.tr("Open Multi-Line Prompt Composer", "打开多行 Prompt 编排器")}">✎ ${this.tr("Composer", "多行")}</button>
              <button type="button" id="btn-voice-mic-desktop" class="pm-btn pm-btn-ghost pm-btn-sm ${micBtnClass}" title="${this.tr("Voice input (Whisper / Zero-Config Speech)", "语音输入 (Whisper / 免配置实时听写)")}">${svgIcon("mic", 14)}${micLabel ? ` <span>${this.escapeHtml(micLabel)}</span>` : ""}</button>
              <button type="button" data-k="ctrl+c" class="pm-btn pm-btn-ghost pm-btn-sm" style="color:var(--coral);">^C</button>
              <button type="button" id="btn-toggle-touch-pad" class="pm-btn pm-btn-ghost pm-btn-sm" title="${this.tr("Toggle virtual keypad", "切换触控虚拟键盘")}">${svgIcon("keyboard", 14)}</button>
              <button type="submit" class="pm-btn pm-btn-primary pm-btn-sm">${this.tr("Send", "发送")}</button>
            </div>
          </form>`
                : ""
          }
        </main>

        <!-- RIGHT VERTICAL THUMB RAIL -->
        <aside class="right-rail">
          <div id="rail-status-pill" role="status" title="${this.escapeHtml(connVisual.railTitle)}" aria-label="${this.escapeHtml(connVisual.railTitle)}" style="width:34px;height:58px;border-radius:999px;border:1px solid ${connVisual.railBorderCss};background:var(--bg-card);display:flex;align-items:flex-end;justify-content:center;padding-bottom:8px;">
            <span id="rail-status-dot" style="width:10px;height:10px;border-radius:99px;background:${connVisual.railDotBgCss};box-shadow:${connVisual.railDotShadowCss};"></span>
          </div>

          <div style="text-align:center;" class="coda-mono">
            <div style="font-size:12px;font-weight:700;">${nowTime}</div>
            <div style="margin-top:6px;width:42px;height:42px;border-radius:999px;border:2px solid var(--border-strong);border-top-color:var(--text-primary);display:flex;flex-direction:column;align-items:center;justify-content:center;font-size:10.5px;">
              <strong>${this.terminals.length}</strong>
              <span id="rail-transport-mode">${connVisual.railTransportBadgeHtml}</span>
            </div>
          </div>

          <div style="display:flex;flex-direction:column;gap:10px;align-items:center;">
            <button id="rail-sidebar-btn" class="circle-btn" title="${this.tr("Toggle left branch sidebar", "展开/收起左侧分支栏")}">${
              this.showSidebar ? svgIcon("chevronLeft", 14) : svgIcon("chevronRight", 14)
            }</button>
            <div class="rail-capsule">
              <button id="rail-spawn-btn" title="${this.tr("Launch Shell / Claude / Codex / Grok / AGY / Gemini", "新建 Shell / Claude / Codex / Grok / AGY / Gemini 终端")}">${svgIcon("plus", 15)}</button>
              <span style="width:16px;height:1px;background:var(--border-primary);"></span>
              <button id="rail-more-btn" title="${this.tr("System settings &amp; account", "系统设置与账号管理")}">${svgIcon("more", 15)}</button>
            </div>
          </div>

          <div class="rail-capsule">
            <input type="file" id="rail-file-input" multiple style="display:none;" />
            <button id="rail-refresh-btn" title="${this.tr("Upload file/image via hc put", "通过 hc put 上传图片/附件至远端终端")}">${svgIcon("paperclip", 14)}</button>
            <span style="width:16px;height:1px;background:var(--border-primary);"></span>
            <button id="rail-paste-btn" title="${this.tr("Paste clipboard", "粘贴剪贴板")}">${svgIcon("clipboard", 14)}</button>
          </div>
        </aside>
      </div>

      ${
        this.showSpawnModal
          ? `
        <div class="modal-mask" id="spawn-modal-bg">
          <div class="modal-card" style="width:560px;max-width:96vw;">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
              <strong style="font-size:15px;">${this.tr("Launch Remote Terminal / AI Agent", "启动远端终端 / AI Agent")}</strong>
              <button id="btn-close-spawn" type="button" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
            </div>
            <form id="spawn-agent-form" style="display:flex;flex-direction:column;gap:10px;">
              <div>
                <label style="font-size:11.5px;color:var(--text-secondary);display:block;margin-bottom:4px;">${this.tr("Agent / Runtime", "启动类型")}</label>
                <select name="agent" class="pm-input" style="margin:0;">
                  <option value="shell" ${this.spawnAgentType === "shell" ? "selected" : ""}>Shell (Interactive PTY)</option>
                  <option value="claude" ${this.spawnAgentType === "claude" ? "selected" : ""}>Claude Code</option>
                  <option value="codex" ${this.spawnAgentType === "codex" ? "selected" : ""}>Codex CLI</option>
                  <option value="grok" ${this.spawnAgentType === "grok" ? "selected" : ""}>Grok CLI</option>
                  <option value="agy" ${this.spawnAgentType === "agy" ? "selected" : ""}>Antigravity (AGY)</option>
                  <option value="gemini" ${this.spawnAgentType === "gemini" ? "selected" : ""}>Gemini CLI</option>
                </select>
              </div>
              <div>
                <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:4px;">
                  <label style="font-size:11.5px;color:var(--text-secondary);">${this.tr("Working Directory (optional)", "工作目录（可选）")}</label>
                  <div style="display:flex;gap:6px;flex-wrap:wrap;">
                    <button type="button" id="btn-spawn-browse-dirs" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 8px;font-size:11px;">
                      📁 ${this.tr("Browse Directories", "浏览远端目录")}
                    </button>
                    <button type="button" id="btn-spawn-toggle-bootstrap" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 8px;font-size:11px;">
                      ${svgIcon("gitBranch", 11)} ${this.tr("Create / Git Clone Workspace", "新建 / Git Clone 工作区")}
                    </button>
                  </div>
                </div>
                <input id="spawn-cwd-input" name="cwd" class="pm-input coda-mono" style="margin:0;" list="spawn-cwd-list" value="${this.escapeHtml(this.spawnCwd)}" placeholder="${this.tr("Leave blank for host default", "留空使用主机默认目录")}" />
                <datalist id="spawn-cwd-list">
                  ${allBranches.map((b) => `<option value="${this.escapeHtml(b.worktree_path)}"></option>`).join("")}
                  ${this.repos.map((r) => `<option value="${this.escapeHtml(r.root_path)}"></option>`).join("")}
                </datalist>
              </div>

              ${
                this.spawnDirPicker.open
                  ? (() => {
                      const currentBrowsed = joinRootAndRel(
                        this.spawnDirPicker.rootPath || "~",
                        this.spawnDirPicker.subPath
                      );
                      const canGoUp = canStepUpSpawnDirectory(
                        this.spawnDirPicker.rootPath,
                        this.spawnDirPicker.subPath
                      );
                      return `
              <div class="spawn-subbox">
                <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;">
                  <code class="coda-mono" style="font-size:11px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:300px;">${this.escapeHtml(currentBrowsed)}</code>
                  <div style="display:flex;gap:6px;">
                    ${
                      canGoUp
                        ? `<button type="button" id="btn-spawn-dir-up" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 8px;font-size:11px;">&uarr; ${this.tr("Up", "上级")}</button>`
                        : ""
                    }
                    <button type="button" id="btn-spawn-dir-use" class="pm-btn pm-btn-primary pm-btn-sm" style="height:24px;padding:0 8px;font-size:11px;">${this.tr("Use Current", "选用此目录")}</button>
                    <button type="button" id="btn-spawn-dir-close" class="pm-btn pm-btn-ghost pm-btn-sm" style="height:24px;padding:0 6px;font-size:11px;">&times;</button>
                  </div>
                </div>
                ${
                  this.spawnDirPicker.error
                    ? `<div style="font-size:11.5px;color:var(--rose);">${this.escapeHtml(this.spawnDirPicker.error)}</div>`
                    : ""
                }
                <div style="max-height:150px;overflow-y:auto;border:1px solid var(--border-primary);border-radius:7px;background:var(--bg-surface);">
                  ${
                    this.spawnDirPicker.loading
                      ? `<div style="padding:10px;font-size:11.5px;color:var(--text-tertiary);">${this.tr("Loading remote folders…", "正在读取远端目录…")}</div>`
                      : this.spawnDirPicker.entries.length === 0
                        ? `<div style="padding:10px;font-size:11.5px;color:var(--text-tertiary);">${this.tr("No subdirectories here — click 'Use Current' above.", "无子目录，可点击上方「选用此目录」。")}</div>`
                        : this.spawnDirPicker.entries
                            .map(
                              (entry) => `
                        <button type="button" class="fb-entry-row" data-spawn-dir-rel="${this.escapeHtml(entry.rel_path)}">
                          <span class="coda-mono">📁 ${this.escapeHtml(entry.name)}/</span>
                          <span class="coda-mono" style="font-size:10px;color:var(--text-tertiary);">${this.tr("Open", "进入")} &rarr;</span>
                        </button>`
                            )
                            .join("")
                  }
                </div>
              </div>`;
                    })()
                  : ""
              }

              ${
                this.spawnBootstrapOpen
                  ? `
              <div class="spawn-subbox">
                <div style="display:flex;justify-content:space-between;align-items:center;">
                  <strong style="font-size:12px;">${this.tr("Create Folder or Git Clone on Host (E2EE)", "远端新建目录或 Git Clone 仓库")}</strong>
                  <button type="button" id="btn-spawn-boot-close" class="pm-btn pm-btn-ghost pm-btn-sm" style="height:22px;padding:0 6px;">&times;</button>
                </div>
                <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;">
                  <div>
                    <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">${this.tr("Parent Directory", "父级目录")}</label>
                    <input id="spawn-boot-parent" class="pm-input coda-mono" style="margin:0;height:32px;font-size:12px;" value="${this.escapeHtml(this.spawnBootstrapParentDir)}" placeholder="~/projects" />
                  </div>
                  <div>
                    <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">${this.tr("Folder Name", "新文件夹名")}</label>
                    <input id="spawn-boot-folder" class="pm-input coda-mono" style="margin:0;height:32px;font-size:12px;" value="${this.escapeHtml(this.spawnBootstrapFolderName)}" placeholder="my-repo" />
                  </div>
                </div>
                <div>
                  <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">${this.tr("Git Clone URL (optional — leave blank to create empty folder)", "Git Clone 地址（可选，留空仅新建空目录）")}</label>
                  <input id="spawn-boot-git-url" class="pm-input coda-mono" style="margin:0;height:32px;font-size:12px;" value="${this.escapeHtml(this.spawnBootstrapGitUrl)}" placeholder="https://github.com/org/repo.git" />
                </div>
                ${
                  this.spawnBootstrapMsg
                    ? `<div style="font-size:11.5px;padding:6px 8px;border-radius:6px;border:1px solid ${
                        this.spawnBootstrapMsg.kind === "err" ? "var(--rose-bd)" : "var(--emerald-bd)"
                      };background:${
                        this.spawnBootstrapMsg.kind === "err" ? "var(--rose-bg)" : "var(--emerald-bg)"
                      };color:${this.spawnBootstrapMsg.kind === "err" ? "var(--rose)" : "var(--emerald)"};">${this.escapeHtml(
                        this.spawnBootstrapMsg.text
                      )}</div>`
                    : ""
                }
                <button type="button" id="btn-spawn-run-bootstrap" class="pm-btn pm-btn-secondary pm-btn-sm" ${this.spawnBootstrapLoading ? "disabled" : ""}>
                  ${
                    this.spawnBootstrapLoading
                      ? this.tr("Bootstrapping on Host…", "正在远端初始化工作区…")
                      : this.tr("Bootstrap & Fill Working Directory", "执行创建/克隆并填入工作目录")
                  }
                </button>
              </div>`
                  : ""
              }

              <div>
                <label style="font-size:11.5px;color:var(--text-secondary);display:block;margin-bottom:4px;">${this.tr("Initial Prompt (optional)", "初始指令 / Prompt（可选）")}</label>
                <input name="initial_prompt" class="pm-input" style="margin:0;" value="${this.escapeHtml(this.spawnInitialPrompt)}" placeholder="${this.tr("e.g. Inspect git status and run tests", "如：查看 git status 并运行测试")}" />
              </div>
              <button type="submit" class="pm-btn pm-btn-primary" style="width:100%;margin-top:4px;">
                ${svgIcon("plus", 14)} ${this.tr("Launch on Host", "在远端主机启动")}
              </button>
            </form>
          </div>
        </div>
      `
          : ""
      }

      ${
        this.pendingActivationCode
          ? `
        <div class="modal-mask" id="activate-modal-mask">
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
          <div class="modal-card" style="width:580px;max-width:96vw;">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
              <strong style="font-size:15px;">${this.tr("Workspace &amp; Account Settings", "工作区与账号设置")}</strong>
              <button id="btn-close-modal" class="circle-btn" style="width:28px;height:28px;">${svgIcon("close", 13)}</button>
            </div>
            <div class="pm-auth-tabs" style="grid-template-columns:repeat(4,minmax(0,1fr));margin-bottom:14px;">
              <button type="button" class="pm-auth-tab ${this.settingsTab === "devices" ? "active" : ""}" data-settings-tab="devices">${this.tr("Devices", "设备管理")}</button>
              <button type="button" class="pm-auth-tab ${this.settingsTab === "alerts" ? "active" : ""}" data-settings-tab="alerts">${this.tr("Alerts", "离线推送")}</button>
              <button type="button" class="pm-auth-tab ${this.settingsTab === "security" ? "active" : ""}" data-settings-tab="security">${this.tr("Security", "安全邀请")}</button>
              <button type="button" class="pm-auth-tab ${this.settingsTab === "voice" ? "active" : ""}" data-settings-tab="voice">${this.tr("Voice", "语音偏好")}</button>
            </div>
            ${feedbackBanner}
            ${
              this.settingsTab === "devices"
                ? `
              <div class="coda-mono" style="background:var(--bg-elevated);border:1px solid var(--border-primary);padding:10px;border-radius:8px;font-size:11px;margin-bottom:10px;word-break:break-all;">
                ${this.tr("Run on any Linux / macOS host and open the activation link:", "在任意 Linux / Mac 终端执行后打开返回的激活链接：")}<br/>
                <span style="color:var(--emerald);">${this.escapeHtml(installCmd)}</span>
              </div>
              <form id="quick-approve-form" style="display:flex;gap:8px;margin-bottom:12px;">
                <input name="user_code" class="pm-input" style="margin:0;flex:1;" placeholder="${this.tr("Enter device code, e.g. ABCD-2345", "输入终端设备码 如 ABCD-2345")}" />
                <button type="submit" class="pm-btn pm-btn-secondary">${this.tr("Approve", "授权接入")}</button>
              </form>
              <div style="display:flex;gap:8px;flex-wrap:wrap;margin-bottom:12px;">
                <button id="btn-gen-pair" type="button" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("+ 6-Digit Pair Token", "+ 生成 6 位配对码")}</button>
                <button id="btn-prune-stale-devices" type="button" class="pm-btn pm-btn-secondary pm-btn-sm" ${this.pruneDevicesInFlight ? "disabled" : ""}>
                  ${this.tr("Clean Offline Browser Devices", "清理离线浏览器设备")}${offlineWebClientCount > 0 ? ` (${offlineWebClientCount})` : ""}
                </button>
                <button id="btn-go-home" type="button" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("Home", "官网首页")}</button>
                <button id="btn-logout" type="button" class="pm-btn pm-btn-secondary pm-btn-sm" style="color:var(--rose);">${this.tr("Sign Out", "退出登录")}</button>
              </div>
              ${
                this.pairCommand
                  ? `<div class="coda-mono" style="background:var(--bg-elevated);border:1px dashed var(--emerald);padding:10px;border-radius:8px;font-size:11.5px;margin-bottom:12px;word-break:break-all;">
                      ${this.tr("One-line install &amp; pair on macOS / Linux:", "在 Mac / Linux 一键安装并配对：")}<br/>${this.escapeHtml(this.pairCommand)}
                    </div>`
                  : ""
              }
              <div style="font-size:12px;font-weight:600;margin-bottom:6px;color:var(--text-secondary);">
                ${this.tr(`All Devices &amp; Clients (${this.allDevices.length})`, `全部节点与客户端设备 (${this.allDevices.length})`)}
              </div>
              <div style="display:flex;flex-direction:column;gap:6px;max-height:240px;overflow-y:auto;">
                ${this.allDevices
                  .map(
                    (dev) => {
                      const hh = this.hostHealthByPeer.get(dev.id);
                      const canOta =
                        dev.is_online &&
                        (dev.role === "daemon" || dev.role === "hybrid") &&
                        (hh ? hh.ota_supported : true);
                      return `
                  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;padding:8px 10px;border-radius:8px;background:var(--bg-elevated);border:1px solid var(--border-primary);">
                    <div style="min-width:0;flex:1;">
                      <div style="font-size:12.5px;font-weight:500;display:flex;align-items:center;gap:6px;">
                        <span style="width:7px;height:7px;border-radius:99px;background:${dev.is_online ? "var(--emerald)" : "var(--text-quaternary)"};flex-shrink:0;"></span>
                        <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${this.escapeHtml(dev.name)}</span>
                      </div>
                      <div class="coda-mono" style="font-size:10.5px;color:var(--text-tertiary);margin-top:2px;">${this.escapeHtml(dev.platform)} &middot; ${this.escapeHtml(dev.role)}${dev.id === this.myClientDevice?.id ? " (this tab)" : ""}${hh ? ` &middot; ${this.escapeHtml(formatWebHostHealth(hh))}` : ""}</div>
                    </div>
                    <div style="display:flex;gap:6px;flex-shrink:0;">
                      ${
                        canOta
                          ? `<button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" data-ota-dev="${this.escapeHtml(dev.id)}">${this.tr("Upgrade", "无感升级")}</button>`
                          : ""
                      }
                      <button type="button" class="pm-btn pm-btn-ghost pm-btn-sm" data-rename-dev="${this.escapeHtml(dev.id)}" data-dev-name="${this.escapeHtml(dev.name)}">${this.tr("Rename", "重命名")}</button>
                      <button type="button" class="pm-btn pm-btn-ghost pm-btn-sm" style="color:var(--rose);" data-delete-dev="${this.escapeHtml(dev.id)}">${this.tr("Remove", "移除")}</button>
                    </div>
                  </div>
                `;
                    }
                  )
                  .join("")}
              </div>
            `
                : this.settingsTab === "alerts"
                  ? `
              ${renderWebPushSettingsCard({
                vapid: this.vapidInfo,
                vapidLoaded: this.vapidLoaded,
                webPushCount: this.webPushCount,
                thisDeviceSubscribed: this.webPushThisDeviceSubscribed,
                busy: this.webPushBusy,
                pushSupported: this.isWebPushApiSupported(),
                iosNeedsHomeScreenTip: detectBrowserIosNonStandalone(),
                tr: (en, zh) => this.tr(en, zh),
              })}
              <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;padding-bottom:12px;margin-bottom:12px;border-bottom:1px solid var(--border-primary);">
                <div style="min-width:0;">
                  <div style="font-size:12.5px;font-weight:500;display:inline-flex;align-items:center;gap:6px;">${svgIcon("bell", 13)} ${this.tr("Browser Alerts (OSC 7501)", "浏览器系统通知 (OSC 7501)")}</div>
                  <div style="font-size:11px;color:var(--text-tertiary);margin-top:2px;">${this.tr("Notify when a background agent needs approval or finishes while tab is open.", "标签页开启时，后台 Agent 需要审批或任务完成弹出系统通知。")}</div>
                </div>
                <button id="btn-toggle-browser-notify" type="button" class="pm-btn ${this.notificationsEnabled && getBrowserNotificationPermission() === "granted" ? "pm-btn-primary" : "pm-btn-secondary"} pm-btn-sm" style="flex-shrink:0;">
                  ${
                    getBrowserNotificationPermission() === "denied"
                      ? this.tr("Blocked", "已拒绝权限")
                      : this.notificationsEnabled && getBrowserNotificationPermission() === "granted"
                        ? this.tr("Enabled", "已开启")
                        : this.tr("Enable", "开启通知")
                  }
                </button>
              </div>

              <form id="webhook-settings-form" style="display:flex;flex-direction:column;gap:8px;">
                <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;">
                  <strong style="font-size:13px;">${this.tr("Offline Alert Webhooks (Zero-Knowledge)", "离线告警推送 Webhooks (零知识元数据)")}</strong>
                  <span class="pm-badge green" style="font-size:10.5px;">0 Terminal Plaintext</span>
                </div>
                <div style="font-size:11.5px;color:var(--text-secondary);line-height:1.5;">
                  ${this.tr(
                    "Receive instant mobile/IM pushes even when all browsers and apps are closed. Only host & session names are sent.",
                    "即使关闭全部浏览器与客户端，当远端 Agent 等待审批或执行完成时仍可实时推送到手机/群聊（仅发送主机与会话名，不含任何终端明文）。"
                  )}
                </div>
                <div style="display:flex;gap:14px;flex-wrap:wrap;padding:6px 0;">
                  <label style="display:inline-flex;align-items:center;gap:6px;font-size:12px;cursor:pointer;">
                    <input type="checkbox" name="notify_on_needs_you" ${this.notificationSettings.notify_on_needs_you ? "checked" : ""} />
                    <span>${this.tr("Alert on Needs-You", "需要审批时推送 (Needs-You)")}</span>
                  </label>
                  <label style="display:inline-flex;align-items:center;gap:6px;font-size:12px;cursor:pointer;">
                    <input type="checkbox" name="notify_on_finished" ${this.notificationSettings.notify_on_finished ? "checked" : ""} />
                    <span>${this.tr("Alert on Task Finished", "任务完成时推送 (Finished)")}</span>
                  </label>
                </div>

                <div>
                  <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">Bark Push URL (iOS)</label>
                  <input name="bark_url" class="pm-input coda-mono" style="margin:0;height:34px;font-size:12px;" value="${this.escapeHtml(this.notificationSettings.bark_url)}" placeholder="https://api.day.app/YOUR_BARK_KEY" />
                </div>
                <div style="display:grid;grid-template-columns:1.2fr 1fr;gap:8px;">
                  <div>
                    <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">Telegram Bot Token</label>
                    <input type="password" name="telegram_bot_token" class="pm-input coda-mono" style="margin:0;height:34px;font-size:12px;" value="${this.escapeHtml(this.notificationSettings.telegram_bot_token)}" placeholder="123456:ABC-DEF..." autocomplete="off" />
                  </div>
                  <div>
                    <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">Telegram Chat ID</label>
                    <input name="telegram_chat_id" class="pm-input coda-mono" style="margin:0;height:34px;font-size:12px;" value="${this.escapeHtml(this.notificationSettings.telegram_chat_id)}" placeholder="123456789" />
                  </div>
                </div>
                <div>
                  <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">${this.tr("Feishu / Lark Webhook URL", "飞书 / Lark 机器人 Webhook")}</label>
                  <input name="feishu_webhook_url" class="pm-input coda-mono" style="margin:0;height:34px;font-size:12px;" value="${this.escapeHtml(this.notificationSettings.feishu_webhook_url)}" placeholder="https://open.feishu.cn/open-apis/bot/v2/hook/..." />
                </div>
                <div>
                  <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">Discord Webhook URL</label>
                  <input name="discord_webhook_url" class="pm-input coda-mono" style="margin:0;height:34px;font-size:12px;" value="${this.escapeHtml(this.notificationSettings.discord_webhook_url)}" placeholder="https://discord.com/api/webhooks/..." />
                </div>
                <div>
                  <label style="font-size:11px;color:var(--text-secondary);display:block;margin-bottom:3px;">${this.tr("Generic JSON Webhook URL", "通用自定义 JSON Webhook")}</label>
                  <input name="generic_webhook_url" class="pm-input coda-mono" style="margin:0;height:34px;font-size:12px;" value="${this.escapeHtml(this.notificationSettings.generic_webhook_url)}" placeholder="https://hooks.example.com/coda-alert" />
                </div>
                <div style="display:flex;gap:8px;margin-top:4px;">
                  <button type="submit" id="btn-save-webhooks" class="pm-btn pm-btn-primary" style="flex:1;" ${this.webhookSaveInFlight ? "disabled" : ""}>
                    ${this.tr("Save Webhooks", "保存推送配置")}
                  </button>
                  <button type="button" id="btn-test-webhooks" class="pm-btn pm-btn-secondary" ${this.webhookTestInFlight ? "disabled" : ""}>
                    ${this.tr("Send Test Push", "发送测试推送")}
                  </button>
                </div>
              </form>
            `
                  : this.settingsTab === "security"
                    ? `
              <div style="margin-bottom:14px;">
                <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:6px;">
                  <strong style="font-size:12.5px;">${this.tr(`Active Sessions (${this.authSessions.length})`, `活跃登录会话 (${this.authSessions.length})`)}</strong>
                  <button type="button" id="btn-logout-all" class="pm-btn pm-btn-secondary pm-btn-sm" style="color:var(--rose);">${this.tr("Sign Out All", "全部下线")}</button>
                </div>
                <div style="display:flex;flex-direction:column;gap:6px;max-height:140px;overflow-y:auto;">
                  ${this.authSessions
                    .map(
                      (s) => `
                    <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;padding:7px 10px;border-radius:8px;background:var(--bg-elevated);border:1px solid var(--border-primary);font-size:11.5px;">
                      <div style="min-width:0;flex:1;">
                        <div style="font-weight:500;">${this.escapeHtml(s.client_label)}${s.is_current ? ` <span class="pm-badge green">${this.tr("Current", "当前")}</span>` : ""}</div>
                        <div class="coda-mono" style="font-size:10px;color:var(--text-tertiary);">${this.escapeHtml(s.scope)} &middot; ${this.escapeHtml(String(s.last_used_at || "").slice(0, 16))}</div>
                      </div>
                      ${
                        !s.is_current
                          ? `<button type="button" class="pm-btn pm-btn-ghost pm-btn-sm" style="color:var(--rose);" data-revoke-session="${this.escapeHtml(s.id)}">${this.tr("Revoke", "吊销")}</button>`
                          : ""
                      }
                    </div>
                  `
                    )
                    .join("")}
                </div>
              </div>

              ${
                this.user.is_admin
                  ? `
                <div style="border-top:1px solid var(--border-primary);padding-top:12px;margin-bottom:14px;">
                  <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                    <strong style="font-size:12.5px;">${this.tr("Admin Invite Codes", "管理员注册邀请码")}</strong>
                    <button id="btn-gen-invite" type="button" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("+ Issue Invite Code", "+ 签发邀请码")}</button>
                  </div>
                  ${
                    this.latestInviteCode
                      ? `<div class="coda-mono" style="background:var(--bg-elevated);border:1px dashed var(--signal-blue);padding:8px 10px;border-radius:8px;font-size:12px;margin-bottom:8px;display:flex;align-items:center;justify-content:space-between;gap:8px;flex-wrap:wrap;">
                          <span>${this.tr("New invite code:", "新签发邀请码：")} <b>${this.escapeHtml(this.latestInviteCode)}</b></span>
                          <button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:24px;padding:0 8px;font-size:11px;" data-copy-invite-link="${this.escapeHtml(this.latestInviteCode)}">
                            ${svgIcon("copy", 12)} ${this.tr("Copy Invite Link", "复制邀请链接")}
                          </button>
                        </div>`
                      : ""
                  }
                  <div style="display:flex;flex-direction:column;gap:5px;max-height:135px;overflow-y:auto;">
                    ${this.invitesList
                      .map(
                        (inv) => `
                      <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;padding:6px 10px;border-radius:6px;background:var(--bg-elevated);border:1px solid var(--border-primary);font-size:11.5px;">
                        <span class="coda-mono">${this.escapeHtml(inv.code)} <span style="color:var(--text-tertiary);">(${inv.used_count}/${inv.max_uses})</span></span>
                        <div style="display:flex;align-items:center;gap:6px;flex-shrink:0;">
                          <button type="button" class="pm-btn pm-btn-secondary pm-btn-sm" style="height:25px;padding:0 8px;font-size:11px;" data-copy-invite-link="${this.escapeHtml(inv.code)}">
                            ${svgIcon("copy", 12)} ${this.tr("Copy Invite Link", "复制邀请链接")}
                          </button>
                          <button type="button" class="pm-btn pm-btn-ghost pm-btn-sm" style="color:var(--rose);" data-delete-invite="${this.escapeHtml(inv.code)}">${this.tr("Revoke", "撤销")}</button>
                        </div>
                      </div>
                    `
                      )
                      .join("")}
                  </div>
                </div>
                ${renderAdminFriendsDirectorySection({
                  users: this.adminUsers,
                  busyUserId: this.adminResetBusyUserId,
                  latestResetLink: this.latestAdminResetLink,
                  tr: (en, zh) => this.tr(en, zh),
                })}
              `
                  : ""
              }

              <form id="change-pw-form" style="border-top:1px solid var(--border-primary);padding-top:12px;margin-bottom:14px;display:flex;flex-direction:column;gap:6px;">
                <strong style="font-size:12.5px;">${this.tr("Change Password", "修改登录密码")}</strong>
                <input type="password" name="current_password" class="pm-input" style="margin:0;" required placeholder="${this.tr("Current password", "当前密码")}" autocomplete="current-password" />
                <input type="password" name="new_password" class="pm-input" style="margin:0;" required minlength="8" maxlength="256" placeholder="${this.tr("New password (at least 8 chars)", "新密码（至少 8 位）")}" autocomplete="new-password" />
                <button type="submit" class="pm-btn pm-btn-secondary pm-btn-sm">${this.tr("Update Password", "确认修改密码")}</button>
              </form>

              <form id="delete-account-form" style="border-top:1px solid var(--rose-bd);padding-top:12px;display:flex;flex-direction:column;gap:6px;">
                <strong style="font-size:12.5px;color:var(--rose);">${this.tr("Danger Zone · Delete Account", "危险操作 · 注销当前账号")}</strong>
                <input type="password" name="current_password" class="pm-input" style="margin:0;" required placeholder="${this.tr("Confirm current password to permanently delete", "输入当前密码确认永久注销账号")}" />
                <button type="submit" class="pm-btn pm-btn-secondary pm-btn-sm" style="color:var(--rose);border-color:var(--rose-bd);">${this.tr("Permanently Delete Account", "永久注销并清除所有设备")}</button>
              </form>
            `
                    : `
              <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;padding-bottom:12px;margin-bottom:12px;border-bottom:1px solid var(--border-primary);">
                <span style="font-size:12px;color:var(--text-secondary);">${this.tr("Interface Language &amp; Theme", "界面语言与外观主题")}</span>
                <div style="display:flex;align-items:center;gap:8px;">
                  ${this.renderLangSwitcher()}
                  ${this.renderThemeSwitcher()}
                </div>
              </div>
              <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;padding-bottom:12px;margin-bottom:12px;border-bottom:1px solid var(--border-primary);">
                <div style="min-width:0;">
                  <div style="font-size:12.5px;font-weight:500;display:inline-flex;align-items:center;gap:6px;">${svgIcon("zap", 13)} ${this.tr("Smart-Path Auto Hot-Switch", "智能路径热切直连")}</div>
                  <div style="font-size:11px;color:var(--text-tertiary);margin-top:2px;">${
                    this.activeTransportMode === "lan_ws"
                      ? this.tr("Active: LAN Direct (zero-hop E2EE)", "当前状态：局域网直连（零跳端到端加密）")
                      : this.tr("Active: Smart Relay (auto-probing LAN in background)", "当前状态：智能中继（后台自动探测局域网无感热切）")
                  }</div>
                </div>
                <button id="btn-probe-lan-now" type="button" class="pm-btn pm-btn-secondary pm-btn-sm" style="flex-shrink:0;" ${!this.selectedDaemon?.is_online ? "disabled" : ""}>
                  ${this.tr("Probe LAN", "立即探测直连")}
                </button>
              </div>
              <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;padding-bottom:12px;margin-bottom:12px;border-bottom:1px solid var(--border-primary);">
                <div style="min-width:0;">
                  <div style="font-size:12.5px;font-weight:500;display:inline-flex;align-items:center;gap:6px;">${svgIcon("mic", 13)} ${this.tr("Zero-Config Browser Speech Fallback", "浏览器免配置实时语音识别")}</div>
                  <div style="font-size:11px;color:var(--text-tertiary);margin-top:2px;">${this.tr("Automatically transcribes live in-browser when no Whisper API key is set.", "未配置 Whisper API Key 时自动启用浏览器原生实时语音识别。")}</div>
                </div>
                <span class="pm-badge ${hasSpeechFallback ? "green" : ""}">${
                  hasSpeechFallback ? this.tr("Supported", "当前浏览器支持") : this.tr("Use Whisper", "需配置 Whisper")
                }</span>
              </div>
              <form id="voice-cfg-form">
                <div style="font-size:13px;font-weight:600;margin-bottom:8px;display:inline-flex;align-items:center;gap:6px;">${svgIcon("mic", 14)} ${this.tr("Whisper Voice Transcription Config (Optional)", "Whisper 语音转写模型配置（可选）")}</div>
                <label style="font-size:11px;color:var(--text-secondary);">Provider</label>
                <input name="provider" class="pm-input" value="${this.escapeHtml(this.voiceConfig.provider)}" placeholder="openai / groq / deepseek" />
                <label style="font-size:11px;color:var(--text-secondary);">Base URL (https://)</label>
                <input name="base_url" class="pm-input" value="${this.escapeHtml(this.voiceConfig.base_url)}" placeholder="https://api.openai.com/v1" />
                <label style="font-size:11px;color:var(--text-secondary);">API Key (${this.tr("leave blank for zero-config browser speech or existing host key", "留空可使用浏览器免配置语音或保留主机已保存密钥")})</label>
                <input type="password" name="api_key" class="pm-input" value="${this.escapeHtml(this.voiceApiKeyLocal)}" placeholder="sk-..." />
                <label style="font-size:11px;color:var(--text-secondary);">Model</label>
                <input name="model" class="pm-input" value="${this.escapeHtml(this.voiceConfig.model)}" placeholder="gpt-4o-mini-transcribe / whisper-1" />
                <button type="submit" class="pm-btn pm-btn-primary" style="width:100%;">
                  ${this.tr("Save Voice Config", "保存语音模型配置")}
                </button>
              </form>
            `
            }
          </div>
        </div>
      `
          : ""
      }
      ${this.showCmdPalette ? this.renderConsoleCommandPaletteModal() : ""}
      ${renderGitDiffModal(this.gitDiff, workspaceRoots, (en, zh) => this.tr(en, zh))}
      ${renderRemoteFileBrowserModal(
        this.fileBrowser,
        workspaceRoots,
        (en, zh) => this.tr(en, zh)
      )}
      ${renderPortPreviewModal(
        this.portPreview,
        this.selectedDaemon?.name || "Host",
        (en, zh) => this.tr(en, zh)
      )}
    `;

    this.bindCommonNavigation(root);

    const openSettings = () => {
      this.showSettingsModal = true;
      this.settingsFeedback = null;
      this.settingsDirtyFields.clear();
      this.render();
      void this.loadAccountSettingsData().then(() => {
        if (this.showSettingsModal) this.renderSettingsPreservingInputs();
      });
    };

    const performLocalSignOut = async () => {
      await this.unsubscribeWebPushForSignOut();
      this.stopVoiceCaptureResources();
      this.isRecordingVoice = false;
      this.isTranscribingVoice = false;
      this.pendingTranscribeReqId = null;
      if (this.pendingTranscribeTimer) {
        clearTimeout(this.pendingTranscribeTimer);
        this.pendingTranscribeTimer = null;
      }
      this.stopAllTransportsAndTimers();
      clearSessionToken();
      this.token = null;
      this.user = null;
      this.allDevices = [];
      this.devices = [];
      this.authSessions = [];
      this.invitesList = [];
      this.myClientDevice = null;
      this.selectedDaemon = null;
      this.terminals = [];
      this.terminalsByPeer.clear();
      this.repos = [];
      this.voiceApiKeyLocal = "";
      this.voiceConfig = {
        provider: "openai",
        base_url: "https://api.openai.com/v1",
        api_key: "",
        model: "gpt-4o-mini-transcribe",
      };
      this.notificationSettings = createDefaultNotificationSettings();
      this.smartPath.resetForNetworkChange();
      this.pendingSelectionCatchup = false;
      this.imeComposing = false;
      this.activationSuccessMsg = "";
      this.lastRenderedTermKey = "";
      this.activeTerminalAddr = null;
      this.splitMode = false;
      this.secondaryTerminalAddr = null;
      this.focusedPane = "primary";
      this.showTermSearch = false;
      this.termSearchQuery = "";
      this.termSearchIdx = 0;
      this.hostHealthByPeer.clear();
      this.fileBrowser = createInitialFileBrowserState();
      this.portPreview = createInitialPortPreviewState();
      this.gitDiff = createInitialGitDiffState();
      this.showPromptComposer = false;
      this.draftInput = "";
      this.bootFolderManuallyEdited = false;
      this.spawnBootstrapReqId = null;
      this.spawnBootstrapPeerId = null;
      this.nextFileInputTarget = "pty";
      this.pendingUploadTargets.clear();
      this.settingsDirtyFields.clear();
      this.webhookTestInFlight = false;
      this.webhookSaveInFlight = false;
      this.pruneDevicesInFlight = false;
      this.cmdPaletteSelectedIdx = 0;
      this.pairCommand = null;
      this.latestInviteCode = null;
      this.adminUsers = [];
      this.adminResetBusyUserId = null;
      this.latestAdminResetLink = null;
      const cleaned = clearAccountScopedSignOutState(
        typeof localStorage !== "undefined" ? localStorage : null
      );
      this.webPushThisDeviceSubscribed = cleaned.webPushThisDeviceSubscribed;
      this.webPushEndpoint = cleaned.webPushEndpoint;
      this.webPushCount = cleaned.webPushCount;
      this.promptSnippets = cleaned.promptSnippets;
      this.onboardingDismissed = cleaned.onboardingDismissed;
      this.onboardingCopiedInstall = cleaned.onboardingCopiedInstall;
      this.savingSnippetOpen = false;
      this.savingSnippetTitle = "";
      this.savingSnippetBusy = false;
      this.rttMs = null;
      this.consoleNotice = null;
      this.settingsFeedback = null;
      this.e2eeSessions.clear();
      this.seenRelayNonces.clear();
      this.directE2eeSessions.clear();
      this.vtTerminals.clear();
      this.vtStreamActive.clear();
      this.showSettingsModal = false;
      this.showSpawnModal = false;
      this.navigate("/login");
    };

    document.getElementById("sidebar-backdrop")?.addEventListener("click", () => {
      this.showSidebar = false;
      this.render();
    });
    document.getElementById("btn-collapse-sidebar")?.addEventListener("click", () => {
      this.showSidebar = false;
      this.render();
    });
    document.getElementById("rail-sidebar-btn")?.addEventListener("click", () => {
      this.showSidebar = !this.showSidebar;
      this.render();
    });
    document.getElementById("btn-open-settings")?.addEventListener("click", openSettings);
    document.getElementById("rail-more-btn")?.addEventListener("click", openSettings);
    document.getElementById("btn-topbar-needs-you")?.addEventListener("click", () => {
      this.jumpToNextNeedsYouTerminal();
    });
    root.querySelectorAll<HTMLElement>("[data-attention-peer]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const peerId = btn.dataset.attentionPeer || "";
        const termAddr = btn.dataset.attentionTerm || "";
        if (!peerId || !termAddr) return;
        if (isNarrowViewport) this.showSidebar = false;
        this.focusNotifiedTerminal(peerId, termAddr);
      });
    });

    const dismissPendingActivation = () => {
      this.pendingActivationCode = "";
      this.pendingDeviceInfo = null;
      this.errorMsg = "";
      window.history.replaceState({}, "", "/console");
      this.render();
    };
    document.getElementById("btn-dismiss-activate")?.addEventListener("click", dismissPendingActivation);
    document.getElementById("activate-modal-mask")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "activate-modal-mask") {
        dismissPendingActivation();
      }
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
        const approved = this.devices.find((d) => d.id === dev.id) || null;
        if (approved) {
          this.selectDaemon(approved);
        }
        this.render();
      } catch (err: any) {
        this.errorMsg = err.message;
        this.render();
      }
    });

    root.querySelectorAll<HTMLElement>("[data-settings-tab]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const tab = btn.dataset.settingsTab as "devices" | "alerts" | "security" | "voice";
        this.settingsTab = tab;
        this.settingsFeedback = null;
        this.render();
        if (tab === "security" || tab === "devices" || tab === "alerts") {
          void this.loadAccountSettingsData().then(() => {
            if (this.showSettingsModal) this.renderSettingsPreservingInputs();
          });
        }
      });
    });

    root.querySelectorAll<HTMLInputElement>("#modal-bg input[name]").forEach((inp) => {
      const markDirty = () => {
        this.settingsDirtyFields.add(`${inp.form?.id || "form"}:${inp.name}`);
      };
      inp.addEventListener("input", markDirty);
      inp.addEventListener("change", markDirty);
    });

    document.getElementById("quick-approve-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      const code = String(fd.user_code || "").trim();
      if (!code) return;
      try {
        const dev = await this.api("/api/v1/auth/device/approve", "POST", { user_code: code });
        this.settingsFeedback = {
          kind: "ok",
          text: this.tr(`Approved device ${dev.name} (${dev.platform})`, `已授权设备 ${dev.name} (${dev.platform})`),
        };
        await this.refreshDevices();
        this.render();
      } catch (err: any) {
        this.settingsFeedback = {
          kind: "err",
          text: String(err?.message || this.tr("Failed to approve device code", "设备码授权失败")),
        };
        this.render();
      }
    });

    const pruneBtn = document.getElementById("btn-prune-stale-devices") as HTMLButtonElement | null;
    pruneBtn?.addEventListener("click", async () => {
      if (this.pruneDevicesInFlight) return;
      this.pruneDevicesInFlight = true;
      pruneBtn.disabled = true;
      try {
        const res = await this.api("/api/v1/devices/prune-stale", "POST", {
          max_idle_days: 0,
          exclude_device_id: this.myClientDevice?.id ?? null,
        });
        const count = Number(res?.pruned_count ?? 0);
        await this.refreshDevices();
        this.settingsFeedback = {
          kind: "ok",
          text: this.tr(
            `Cleaned ${count} offline browser device(s).`,
            `已清理 ${count} 台离线浏览器客户端设备。`
          ),
        };
      } catch (err: any) {
        this.settingsFeedback = {
          kind: "err",
          text: String(err?.message || this.tr("Failed pruning stale browser devices", "清理离线浏览器设备失败")),
        };
      } finally {
        this.pruneDevicesInFlight = false;
        this.renderSettingsPreservingInputs();
      }
    });

    const readWebhookFormValues = (): NotificationWebhookSettings | null => {
      const form = document.getElementById("webhook-settings-form") as HTMLFormElement | null;
      if (!form) return null;
      const fd = new FormData(form);
      return {
        bark_url: String(fd.get("bark_url") || "").trim(),
        telegram_bot_token: String(fd.get("telegram_bot_token") || "").trim(),
        telegram_chat_id: String(fd.get("telegram_chat_id") || "").trim(),
        feishu_webhook_url: String(fd.get("feishu_webhook_url") || "").trim(),
        discord_webhook_url: String(fd.get("discord_webhook_url") || "").trim(),
        generic_webhook_url: String(fd.get("generic_webhook_url") || "").trim(),
        notify_on_needs_you: fd.get("notify_on_needs_you") !== null,
        notify_on_finished: fd.get("notify_on_finished") !== null,
      };
    };

    const webhookForm = document.getElementById("webhook-settings-form") as HTMLFormElement | null;
    const markWebhooksDirty = () => {
      this.settingsDirtyFields.add("webhooks");
      const cur = readWebhookFormValues();
      if (cur) {
        this.notificationSettings = cur;
      }
    };
    webhookForm?.addEventListener("input", markWebhooksDirty);
    webhookForm?.addEventListener("change", markWebhooksDirty);

    webhookForm?.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (this.webhookSaveInFlight) return;
      const nextCfg = readWebhookFormValues();
      if (!nextCfg) return;
      const vErr = validateNotificationWebhookSettings(nextCfg);
      if (vErr) {
        this.settingsFeedback = { kind: "err", text: vErr };
        this.settingsDirtyFields.delete("webhooks");
        this.notificationSettings = nextCfg;
        this.render();
        return;
      }
      this.webhookSaveInFlight = true;
      const saveBtn = document.getElementById("btn-save-webhooks") as HTMLButtonElement | null;
      if (saveBtn) saveBtn.disabled = true;
      try {
        const saved = await this.api("/api/v1/me/notifications", "PUT", nextCfg);
        this.notificationSettings = {
          bark_url: String(saved?.bark_url ?? nextCfg.bark_url),
          telegram_bot_token: String(saved?.telegram_bot_token ?? nextCfg.telegram_bot_token),
          telegram_chat_id: String(saved?.telegram_chat_id ?? nextCfg.telegram_chat_id),
          feishu_webhook_url: String(saved?.feishu_webhook_url ?? nextCfg.feishu_webhook_url),
          discord_webhook_url: String(saved?.discord_webhook_url ?? nextCfg.discord_webhook_url),
          generic_webhook_url: String(saved?.generic_webhook_url ?? nextCfg.generic_webhook_url),
          notify_on_needs_you: saved?.notify_on_needs_you !== false,
          notify_on_finished: saved?.notify_on_finished !== false,
        };
        this.settingsDirtyFields.delete("webhooks");
        this.settingsFeedback = {
          kind: "ok",
          text: this.tr("Saved offline alert webhook settings.", "已保存离线告警推送 Webhook 配置。"),
        };
      } catch (err: any) {
        this.notificationSettings = nextCfg;
        this.settingsDirtyFields.delete("webhooks");
        this.settingsFeedback = {
          kind: "err",
          text: String(err?.message || this.tr("Failed saving webhook settings", "保存离线推送配置失败")),
        };
      } finally {
        this.webhookSaveInFlight = false;
        this.render();
      }
    });

    const btnTestWebhooks = document.getElementById("btn-test-webhooks") as HTMLButtonElement | null;
    btnTestWebhooks?.addEventListener("click", async () => {
      if (this.webhookTestInFlight) return;
      const nextCfg = readWebhookFormValues();
      if (nextCfg) {
        const vErr = validateNotificationWebhookSettings(nextCfg);
        if (vErr) {
          this.notificationSettings = nextCfg;
          this.settingsDirtyFields.delete("webhooks");
          this.settingsFeedback = { kind: "err", text: vErr };
          this.render();
          return;
        }
      }
      this.webhookTestInFlight = true;
      btnTestWebhooks.disabled = true;
      try {
        if (nextCfg) {
          try {
            const saved = await this.api("/api/v1/me/notifications", "PUT", nextCfg);
            if (saved && typeof saved === "object") {
              this.notificationSettings = {
                bark_url: String(saved.bark_url ?? nextCfg.bark_url),
                telegram_bot_token: String(saved.telegram_bot_token ?? nextCfg.telegram_bot_token),
                telegram_chat_id: String(saved.telegram_chat_id ?? nextCfg.telegram_chat_id),
                feishu_webhook_url: String(saved.feishu_webhook_url ?? nextCfg.feishu_webhook_url),
                discord_webhook_url: String(saved.discord_webhook_url ?? nextCfg.discord_webhook_url),
                generic_webhook_url: String(saved.generic_webhook_url ?? nextCfg.generic_webhook_url),
                notify_on_needs_you: saved.notify_on_needs_you !== false,
                notify_on_finished: saved.notify_on_finished !== false,
              };
            }
            this.settingsDirtyFields.delete("webhooks");
          } catch (err: any) {
            this.notificationSettings = nextCfg;
            this.settingsDirtyFields.delete("webhooks");
            this.settingsFeedback = {
              kind: "err",
              text: String(err?.message || this.tr("Failed saving webhook settings before test", "测试前保存推送配置失败")),
            };
            return;
          }
        }
        const res = await this.api("/api/v1/me/notifications/test", "POST", {});
        const delivered: string[] = Array.isArray(res?.delivered_channels) ? res.delivered_channels : [];
        const errors: string[] = Array.isArray(res?.errors) ? res.errors : [];
        if (errors.length > 0) {
          this.settingsFeedback = {
            kind: "err",
            text: `${this.tr("Test push errors:", "测试推送部分失败：")} ${errors.join("; ")}`,
          };
        } else if (delivered.length > 0) {
          this.settingsFeedback = {
            kind: "ok",
            text: this.tr(
              `Test alert delivered via: ${delivered.join(", ")}`,
              `测试推送已送达通道：${delivered.join(", ")}`
            ),
          };
        } else {
          this.settingsFeedback = {
            kind: "err",
            text: this.tr(
              "No push channels configured yet — enable Lock-Screen Web Push above, or enter at least one Webhook URL / Telegram bot.",
              "尚未配置任何推送通道，请点击上方开启锁屏推送，或至少填写一个 Webhook URL / Telegram 配置。"
            ),
          };
        }
      } catch (err: any) {
        this.settingsFeedback = {
          kind: "err",
          text: String(err?.message || this.tr("Test push failed", "测试推送请求失败")),
        };
      } finally {
        this.webhookTestInFlight = false;
        this.render();
      }
    });

    root.querySelectorAll<HTMLElement>("[data-ota-dev]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const id = btn.dataset.otaDev!;
        const peer = this.devices.find((d) => d.id === id) || this.allDevices.find((d) => d.id === id);
        if (peer) this.upgradeRemoteDaemon(peer);
      });
    });

    root.querySelectorAll<HTMLElement>("[data-rename-dev]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const id = btn.dataset.renameDev!;
        const oldName = btn.dataset.devName || "";
        const nextName = window.prompt(this.tr("New device name:", "输入新的设备名称："), oldName)?.trim();
        if (!nextName || nextName === oldName) return;
        try {
          await this.api(`/api/v1/devices/${encodeURIComponent(id)}`, "PATCH", { name: nextName });
          this.settingsFeedback = {
            kind: "ok",
            text: this.tr(`Renamed device to "${nextName}"`, `已重命名为「${nextName}」`),
          };
          await this.refreshDevices();
          this.render();
        } catch (err: any) {
          this.settingsFeedback = { kind: "err", text: String(err?.message || "Rename failed") };
          this.render();
        }
      });
    });

    root.querySelectorAll<HTMLElement>("[data-delete-dev]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const id = btn.dataset.deleteDev!;
        if (!window.confirm(this.tr("Remove and revoke this device from your account?", "确认移除并吊销该设备吗？"))) {
          return;
        }
        try {
          await this.api(`/api/v1/devices/${encodeURIComponent(id)}`, "DELETE");
          this.e2eeSessions.delete(id);
          this.directE2eeSessions.delete(id);
          this.terminalsByPeer.delete(id);
          this.pruneStaleVtTerminalsForPeer(id, []);
          if (this.directWsPeerId === id) {
            this.teardownDirectWs();
          }
          this.settingsFeedback = { kind: "ok", text: this.tr("Device removed.", "设备已移除。") };
          await this.refreshDevices();
          this.render();
        } catch (err: any) {
          this.settingsFeedback = { kind: "err", text: String(err?.message || "Delete failed") };
          this.render();
        }
      });
    });

    root.querySelectorAll<HTMLElement>("[data-revoke-session]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const sid = btn.dataset.revokeSession!;
        try {
          await this.api(`/api/v1/auth/sessions/${encodeURIComponent(sid)}`, "DELETE");
          this.settingsFeedback = { kind: "ok", text: this.tr("Session revoked.", "会话已吊销。") };
          await this.loadAccountSettingsData();
          this.render();
        } catch (err: any) {
          this.settingsFeedback = { kind: "err", text: String(err?.message || "Revoke failed") };
          this.render();
        }
      });
    });

    document.getElementById("btn-logout-all")?.addEventListener("click", async () => {
      if (!window.confirm(this.tr("Sign out all browser/mobile sessions?", "确认强制退出全部登录会话吗？"))) {
        return;
      }
      await this.unsubscribeWebPushForSignOut();
      try {
        if (this.token) {
          await this.api("/api/v1/auth/logout-all", "POST", {});
        }
      } catch {
        // proceed to local teardown regardless
      }
      await performLocalSignOut();
    });

    root.querySelectorAll<HTMLElement>("[data-enable-web-push]").forEach((btn) => {
      btn.addEventListener("click", () => {
        void this.enableLockScreenWebPush();
      });
    });
    root.querySelectorAll<HTMLElement>("[data-disable-web-push]").forEach((btn) => {
      btn.addEventListener("click", () => {
        void this.disableLockScreenWebPush();
      });
    });

    document.getElementById("btn-onboarding-dismiss")?.addEventListener("click", () => {
      this.onboardingDismissed = true;
      writeLocalPref(onboardingDismissedStorageKey(this.user?.id), "1");
      this.render();
    });
    document.getElementById("btn-sidebar-show-onboarding")?.addEventListener("click", () => {
      this.onboardingDismissed = false;
      writeLocalPref(onboardingDismissedStorageKey(this.user?.id), "0");
      this.render();
    });
    document.getElementById("btn-onboarding-copy-install")?.addEventListener("click", async () => {
      const ok = await this.copyTextWithFallback(installCmd);
      this.onboardingCopiedInstall = ok;
      this.showToastNotice(
        ok ? "ok" : "err",
        ok
          ? this.tr("Copied 1-line install command to clipboard.", "已复制一行主机安装绑定命令。")
          : this.tr("Copy the install command manually from Step 1.", "请直接手动复制步骤 1 中的安装命令。")
      );
      this.render();
      if (ok) {
        setTimeout(() => {
          this.onboardingCopiedInstall = false;
          if (this.route === "console" && !this.isTypingIntoInput()) {
            this.render();
          }
        }, 2000);
      }
    });
    document.getElementById("btn-onboarding-open-devices")?.addEventListener("click", () => {
      this.settingsTab = "devices";
      openSettings();
    });
    document.getElementById("btn-onboarding-open-alerts")?.addEventListener("click", () => {
      this.settingsTab = "alerts";
      openSettings();
    });
    document.getElementById("btn-onboarding-launch-agent")?.addEventListener("click", () => {
      if (this.selectedDaemon?.is_online) {
        this.openNewAgentModal();
      } else {
        this.settingsTab = "devices";
        openSettings();
        this.settingsFeedback = {
          kind: "ok",
          text: this.tr(
            "Run the Step 1 install command on your Mac/Linux host first, then approve its device code here.",
            "请先在你的 Mac 或 Linux 主机终端执行一行安装命令，然后在此输入设备码完成绑定。"
          ),
        };
        this.render();
      }
    });

    root.querySelectorAll<HTMLElement>("[data-copy-invite-link]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const code = (btn.dataset.copyInviteLink || "").trim();
        if (!code) return;
        const inviteBase = this.siteInfo?.public_base_url || window.location.origin;
        const inviteUrl = buildInviteRegistrationUrl(inviteBase, code);
        const ok = await this.copyTextWithFallback(inviteUrl);
        const msg = ok
          ? this.tr(`Copied invite link: ${inviteUrl}`, `已复制好友注册邀请链接：${inviteUrl}`)
          : this.tr(`Invite link: ${inviteUrl}`, `好友注册邀请链接：${inviteUrl}`);
        this.settingsFeedback = { kind: "ok", text: msg };
        this.showToastNotice("ok", msg);
        this.renderSettingsPreservingInputs();
      });
    });

    document.getElementById("btn-copy-latest-reset-link")?.addEventListener("click", async () => {
      if (!this.latestAdminResetLink) return;
      const url = this.latestAdminResetLink.resetUrl;
      const ok = await this.copyTextWithFallback(url);
      const msg = ok
        ? this.tr(
            `Copied reset link for @${this.latestAdminResetLink.handle}.`,
            `已复制 @${this.latestAdminResetLink.handle} 的密码重置链接。`
          )
        : url;
      this.settingsFeedback = { kind: "ok", text: msg };
      this.showToastNotice("ok", msg);
      this.renderSettingsPreservingInputs();
    });

    root.querySelectorAll<HTMLElement>("[data-admin-reset-user]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const userId = (btn.dataset.adminResetUser || "").trim();
        const handleHint = (btn.dataset.adminResetHandle || "").trim();
        if (!userId || this.adminResetBusyUserId) return;
        this.adminResetBusyUserId = userId;
        this.renderSettingsPreservingInputs();
        try {
          const res = await this.api(
            `/api/v1/admin/users/${encodeURIComponent(userId)}/reset-link`,
            "POST",
            {}
          );
          const resetBase = this.siteInfo?.public_base_url || window.location.origin;
          const resetUrl = String(
            res?.reset_url ||
              `${resetBase.replace(/\/+$/, "")}/reset-password?token=${encodeURIComponent(
                String(res?.reset_token || "")
              )}`
          );
          const handle = String(res?.handle || handleHint || "member");
          this.latestAdminResetLink = { handle, resetUrl };
          const copied = await this.copyTextWithFallback(resetUrl);
          const msg = copied
            ? this.tr(
                `Copied one-time reset link for @${handle} (valid 24h). Send it directly to your friend!`,
                `已生成并复制 @${handle} 的一次性密码重置链接（24 小时内有效），可直接发给好友！`
              )
            : this.tr(
                `Generated reset link for @${handle}: ${resetUrl}`,
                `已生成 @${handle} 的重置链接：${resetUrl}`
              );
          this.settingsFeedback = { kind: "ok", text: msg };
          this.showToastNotice("ok", msg);
        } catch (err: any) {
          this.settingsFeedback = {
            kind: "err",
            text: String(err?.message || this.tr("Failed issuing reset link", "生成重置链接失败")),
          };
        } finally {
          this.adminResetBusyUserId = null;
          this.renderSettingsPreservingInputs();
        }
      });
    });

    root.querySelectorAll<HTMLElement>("[data-delete-invite]").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const code = btn.dataset.deleteInvite!;
        try {
          await this.api(`/api/v1/invites/${encodeURIComponent(code)}`, "DELETE");
          this.settingsFeedback = { kind: "ok", text: this.tr(`Revoked invite ${code}`, `已撤销邀请码 ${code}`) };
          await this.loadAccountSettingsData();
          this.render();
        } catch (err: any) {
          this.settingsFeedback = { kind: "err", text: String(err?.message || "Revoke invite failed") };
          this.render();
        }
      });
    });

    document.getElementById("change-pw-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      const newPw = String(fd.new_password || "");
      const pwCheck = validateNewPasswordLength(newPw, (en, zh) => this.tr(en, zh));
      if (!pwCheck.ok) {
        this.settingsFeedback = { kind: "err", text: pwCheck.error };
        this.render();
        return;
      }
      try {
        const res = await this.api("/api/v1/auth/password", "POST", {
          current_password: String(fd.current_password || ""),
          new_password: newPw,
        });
        if (res?.token) {
          this.token = res.token;
          saveSessionToken(res.token);
        }
        this.settingsFeedback = {
          kind: "ok",
          text: this.tr("Password updated. Other sessions were signed out.", "密码已更新，其余会话已自动下线。"),
        };
        await this.loadAccountSettingsData();
        this.render();
      } catch (err: any) {
        this.settingsFeedback = { kind: "err", text: String(err?.message || "Password update failed") };
        this.render();
      }
    });

    document.getElementById("delete-account-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (
        !window.confirm(
          this.tr(
            "Permanently delete your account and all enrolled devices? This cannot be undone.",
            "确定永久注销账号并清除全部设备吗？此操作不可撤销。"
          )
        )
      ) {
        return;
      }
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      try {
        // V3-09: Backend `DeleteAccountRequest` expects `{ password }`.
        await this.api("/api/v1/auth/me", "DELETE", {
          password: String(fd.current_password || ""),
        });
        await performLocalSignOut();
      } catch (err: any) {
        this.settingsFeedback = { kind: "err", text: String(err?.message || "Delete account failed") };
        this.render();
      }
    });

    document.getElementById("btn-close-modal")?.addEventListener("click", () => {
      this.showSettingsModal = false;
      this.render();
    });
    document.getElementById("modal-bg")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "modal-bg") {
        this.showSettingsModal = false;
        this.render();
      }
    });
    document.getElementById("btn-gen-pair")?.addEventListener("click", async () => {
      try {
        const res = await this.api("/api/v1/devices/pair-token", "POST", {});
        this.pairCommand = res.one_liner_command;
        this.render();
      } catch (err: any) {
        this.settingsFeedback = { kind: "err", text: String(err?.message || "Failed generating pair token") };
        this.render();
      }
    });
    document.getElementById("btn-gen-invite")?.addEventListener("click", async () => {
      try {
        const res = await this.api("/api/v1/invites", "POST", { note: "Friend", max_uses: 5, valid_days: 30 });
        this.latestInviteCode = res.code;
        await this.loadAccountSettingsData();
        this.render();
      } catch (err: any) {
        this.settingsFeedback = { kind: "err", text: String(err?.message || "Failed issuing invite code") };
        this.render();
      }
    });
    document.getElementById("btn-go-home")?.addEventListener("click", () => {
      this.showSettingsModal = false;
      this.navigate("/");
    });
    document.getElementById("btn-toggle-browser-notify")?.addEventListener("click", async () => {
      if (this.notificationsEnabled && getBrowserNotificationPermission() === "granted") {
        this.notificationsEnabled = false;
        writeLocalPref("coda_notify_enabled", "0");
        this.settingsFeedback = {
          kind: "ok",
          text: this.tr("Browser notifications disabled.", "已关闭浏览器系统通知。"),
        };
        this.render();
        return;
      }
      const perm = await requestBrowserNotificationPermission();
      if (perm === "granted") {
        this.notificationsEnabled = true;
        writeLocalPref("coda_notify_enabled", "1");
        this.enrollBackgroundDaemonsForNotifications();
        this.settingsFeedback = {
          kind: "ok",
          text: this.tr("Browser notifications enabled for OSC 7501 alerts.", "已开启 OSC 7501 系统通知。"),
        };
      } else if (perm === "unsupported") {
        this.notificationsEnabled = false;
        this.settingsFeedback = {
          kind: "err",
          text: this.tr("This browser does not support Web Notifications.", "当前浏览器不支持系统通知 API。"),
        };
      } else {
        this.notificationsEnabled = false;
        writeLocalPref("coda_notify_enabled", "0");
        this.settingsFeedback = {
          kind: "err",
          text: this.tr(
            "Notification permission was denied in browser settings.",
            "浏览器已拒绝通知权限，请在地址栏站点设置中允许通知。"
          ),
        };
      }
      this.render();
    });
    document.getElementById("btn-probe-lan-now")?.addEventListener("click", () => {
      if (!this.selectedDaemon?.is_online) return;
      this.smartPath.resetForNetworkChange(this.selectedDaemon.id);
      this.settingsFeedback = {
        kind: "ok",
        text: this.tr("Probing LAN Direct endpoints in background…", "正在后台探测局域网直连端点…"),
      };
      this.render();
      void this.tryNegotiateDirectLanWs(this.selectedDaemon, true);
    });

    if (this.termResizeObserver) {
      this.termResizeObserver.disconnect();
      this.termResizeObserver = null;
    }
    const termBox = document.getElementById("term-box");
    const secTermBox = document.getElementById("term-box-secondary");
    const setFocusedPane = (pane: "primary" | "secondary") => {
      const effective: "primary" | "secondary" =
        pane === "secondary" && this.splitMode && !isNarrowViewport && this.secondaryTerminalAddr
          ? "secondary"
          : "primary";
      if (this.focusedPane === effective) return;
      this.focusedPane = effective;
      termBox?.classList.toggle("pane-focused", effective === "primary");
      secTermBox?.classList.toggle("pane-focused", effective === "secondary");
      const nextFocusedTerm = this.terminals.find(
        (t) => t.address === (effective === "secondary" ? this.secondaryTerminalAddr : this.activeTerminalAddr)
      );
      this.syncNeedsYouBarInPlace(nextFocusedTerm);
      this.scheduleTerminalPaint();
    };
    if (termBox) {
      // V3-22: Restore scroll position across console re-renders (pin to bottom on terminal switch or if already near bottom).
      termBox.scrollTop = switchedTerminal || prevWasNearBottom ? termBox.scrollHeight : prevScrollTop;
      this.syncScrollBottomPill(termBox);
    }
    if (termBox && typeof ResizeObserver !== "undefined") {
      this.termResizeObserver = new ResizeObserver(() => {
        this.scheduleTerminalResizeSync();
      });
      this.termResizeObserver.observe(termBox);
      if (secTermBox) {
        this.termResizeObserver.observe(secTermBox);
      }
    }

    termBox?.addEventListener("focus", () => {
      setFocusedPane("primary");
    });
    termBox?.addEventListener("mousedown", () => {
      setFocusedPane("primary");
    });

    // Direct raw keystroke, drag-drop file upload, scrollback jump-to-bottom & clipboard image/text paste
    termBox?.addEventListener("keydown", (e) => {
      if (this.isAnyModalOpen()) return;
      if (isJumpNeedsYouShortcut(e)) {
        e.preventDefault();
        e.stopPropagation();
        this.jumpToNextNeedsYouTerminal();
        return;
      }
      if (isToggleSplitViewShortcut(e)) {
        e.preventDefault();
        e.stopPropagation();
        this.toggleSplitView();
        return;
      }
      if (!this.selectedDaemon || !this.activeTerminalAddr) return;
      setFocusedPane("primary");
      const keyVt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, this.activeTerminalAddr));
      const seq = encodeTerminalKeyEvent(e, {
        appCursorKeys: keyVt?.appCursorKeys ?? false,
        imeComposing: this.imeComposing,
      });
      if (seq !== null) {
        e.preventDefault();
        // V3-24: Prevent terminal keys like Ctrl+K from bubbling up to window ⌘K palette handler.
        e.stopPropagation();
        this.sendPtyInputBytesTo(this.activeTerminalAddr, seq);
      }
    });

    termBox?.addEventListener("paste", (e) => {
      if (this.isAnyModalOpen() || this.isEditablePasteTarget(e.target)) return;
      if (!this.selectedDaemon || !this.activeTerminalAddr) return;
      setFocusedPane("primary");
      if (this.handleClipboardImagePaste(e, "pty", this.activeTerminalAddr)) return;
      const text = e.clipboardData?.getData("text/plain") || e.clipboardData?.getData("text") || "";
      if (!text) return;
      e.preventDefault();
      const keyVt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, this.activeTerminalAddr));
      const curTerm = this.terminals.find((t) => t.address === this.activeTerminalAddr);
      const useBp = resolveEffectiveBracketedPaste({
        bracketedPaste: keyVt?.bracketedPaste,
        bracketedPasteExplicit: keyVt?.bracketedPasteExplicit,
        term: curTerm,
      });
      this.sendPtyInputBytesTo(this.activeTerminalAddr, wrapBracketedPaste(text, useBp));
    });

    secTermBox?.addEventListener("focus", (e) => {
      const tag = String((e.target as HTMLElement | null)?.tagName || "").toUpperCase();
      if (tag === "SELECT" || tag === "BUTTON" || tag === "INPUT") return;
      setFocusedPane("secondary");
    });
    secTermBox?.addEventListener("mousedown", (e) => {
      const tag = String((e.target as HTMLElement | null)?.tagName || "").toUpperCase();
      if (tag === "SELECT" || tag === "BUTTON" || tag === "INPUT" || tag === "OPTION") return;
      setFocusedPane("secondary");
    });
    secTermBox?.addEventListener("keydown", (e) => {
      if (this.isAnyModalOpen()) return;
      const tag = String((e.target as HTMLElement | null)?.tagName || "").toUpperCase();
      if (tag === "SELECT" || tag === "BUTTON" || tag === "INPUT") return;
      if (isJumpNeedsYouShortcut(e)) {
        e.preventDefault();
        e.stopPropagation();
        this.jumpToNextNeedsYouTerminal();
        return;
      }
      if (isToggleSplitViewShortcut(e)) {
        e.preventDefault();
        e.stopPropagation();
        this.toggleSplitView();
        return;
      }
      if (!this.selectedDaemon || !this.secondaryTerminalAddr) return;
      setFocusedPane("secondary");
      const secVt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, this.secondaryTerminalAddr));
      const seq = encodeTerminalKeyEvent(e, {
        appCursorKeys: secVt?.appCursorKeys ?? false,
        imeComposing: this.imeComposing,
      });
      if (seq !== null) {
        e.preventDefault();
        e.stopPropagation();
        this.sendPtyInputBytesTo(this.secondaryTerminalAddr, seq);
      }
    });
    secTermBox?.addEventListener("paste", (e) => {
      if (this.isAnyModalOpen() || this.isEditablePasteTarget(e.target)) return;
      if (!this.selectedDaemon || !this.secondaryTerminalAddr) return;
      setFocusedPane("secondary");
      if (this.handleClipboardImagePaste(e, "pty", this.secondaryTerminalAddr)) return;
      const text = e.clipboardData?.getData("text/plain") || e.clipboardData?.getData("text") || "";
      if (!text) return;
      e.preventDefault();
      const secVt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, this.secondaryTerminalAddr));
      const secTerm = this.terminals.find((t) => t.address === this.secondaryTerminalAddr);
      const useBp = resolveEffectiveBracketedPaste({
        bracketedPaste: secVt?.bracketedPaste,
        bracketedPasteExplicit: secVt?.bracketedPasteExplicit,
        term: secTerm,
      });
      this.sendPtyInputBytesTo(this.secondaryTerminalAddr, wrapBracketedPaste(text, useBp));
    });

    const dragHasFiles = (e: DragEvent): boolean => {
      const types = e.dataTransfer?.types;
      return Boolean(types && Array.from(types).includes("Files"));
    };
    const bindTerminalDropZone = (boxEl: HTMLElement | null, pane: "primary" | "secondary") => {
      if (!boxEl) return;
      let dropDepth = 0;
      boxEl.addEventListener("dragenter", (e) => {
        if (!dragHasFiles(e)) return;
        e.preventDefault();
        dropDepth += 1;
        boxEl.classList.add("term-drop-active");
      });
      boxEl.addEventListener("dragover", (e) => {
        if (!dragHasFiles(e)) return;
        e.preventDefault();
        if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
      });
      boxEl.addEventListener("dragleave", () => {
        if (dropDepth <= 0) return;
        dropDepth = Math.max(0, dropDepth - 1);
        if (dropDepth === 0) boxEl.classList.remove("term-drop-active");
      });
      boxEl.addEventListener("drop", (e) => {
        if (!dragHasFiles(e)) return;
        e.preventDefault();
        dropDepth = 0;
        boxEl.classList.remove("term-drop-active");
        const files = Array.from(e.dataTransfer?.files || []);
        if (files.length > 0) {
          if (pane === "secondary" && this.secondaryTerminalAddr) {
            setFocusedPane("secondary");
          } else if (pane === "primary") {
            setFocusedPane("primary");
          }
          const pinnedDaemon = this.selectedDaemon;
          const pinnedTerminal =
            pane === "secondary" ? this.secondaryTerminalAddr ?? null : this.activeTerminalAddr ?? null;
          void (async () => {
            for (const f of files) {
              await this.uploadSingleFileToHost(f, pinnedDaemon, pinnedTerminal, undefined, "pty");
            }
          })();
        }
      });
    };
    bindTerminalDropZone(termBox, "primary");
    bindTerminalDropZone(secTermBox, "secondary");

    termBox?.addEventListener("scroll", () => {
      this.syncScrollBottomPill(termBox);
    });
    document.getElementById("btn-scroll-bottom")?.addEventListener("click", () => {
      if (!termBox) return;
      termBox.scrollTop = termBox.scrollHeight;
      this.syncScrollBottomPill(termBox);
    });

    // OSC 7501 Needs-You one-tap capsule approval buttons (`Allow (y)`, `Reject (Esc)`, `1`, `2`, `3`, `Enter`)
    document.querySelectorAll<HTMLButtonElement>("[data-approve]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const targetAddr = this.getFocusedTerminalAddr();
        if (!this.selectedDaemon || !targetAddr) return;
        const action = (btn.dataset.approve || "allow") as ApprovalCapsuleAction;
        this.sendPtyInputBytesTo(targetAddr, encodeApprovalActionBytes(action));
        // V3-10: Optimistically transition focused terminal out of NeedsYou so bar & sidebar inbox update cleanly.
        const active = this.terminals.find((t) => t.address === targetAddr);
        if (active) {
          active.status = {
            ...(active.status || {}),
            state: "working",
            msg: null,
          };
          this.terminalsByPeer.set(this.selectedDaemon.id, [...this.terminals]);
        }
        this.syncNeedsYouBarInPlace(active);
        if (this.showSidebar) {
          this.render();
        }
      });
    });

    // Multi-Line Prompt Composer Drawer bindings
    const sendPromptComposerPayload = () => {
      if (this.imeComposing) return;
      const area = document.getElementById("prompt-composer-textarea") as HTMLTextAreaElement | null;
      const rawText = area ? area.value : this.draftInput;
      const targetAddr = this.getFocusedTerminalAddr();
      if (this.selectedDaemon && targetAddr) {
        if (rawText.trim()) {
          const targetVt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, targetAddr));
          const targetTerm = this.terminals.find((t) => t.address === targetAddr);
          const useBp = resolveEffectiveBracketedPaste({
            bracketedPaste: targetVt?.bracketedPaste,
            bracketedPasteExplicit: targetVt?.bracketedPasteExplicit,
            term: targetTerm,
          });
          this.sendPtyInputBytesTo(targetAddr, encodeComposerSubmit(rawText, useBp));
        } else {
          this.sendPtyInputBytesTo(targetAddr, "\r");
        }
      }
      this.draftInput = "";
      this.updateDraftPreviewInPlace();
      if (area) {
        area.value = "";
        area.focus();
      }
      if (termBox) {
        termBox.scrollTop = termBox.scrollHeight;
        this.syncScrollBottomPill(termBox);
      }
    };

    const composerTextarea = document.getElementById("prompt-composer-textarea") as HTMLTextAreaElement | null;
    composerTextarea?.addEventListener("focus", () => {
      root.querySelector(".shell-root")?.classList.add("is-composer-focused");
      this.scheduleTerminalResizeSync();
    });
    composerTextarea?.addEventListener("compositionstart", () => {
      this.imeComposing = true;
    });
    composerTextarea?.addEventListener("compositionend", () => {
      this.imeComposing = false;
      this.draftInput = composerTextarea.value;
    });
    composerTextarea?.addEventListener("blur", () => {
      this.imeComposing = false;
      root.querySelector(".shell-root")?.classList.remove("is-composer-focused");
      this.scheduleTerminalResizeSync();
    });
    composerTextarea?.addEventListener("input", () => {
      this.draftInput = composerTextarea.value;
    });
    composerTextarea?.addEventListener("keydown", (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter" && !e.isComposing && !this.imeComposing) {
        e.preventDefault();
        sendPromptComposerPayload();
      } else if (e.key === "Escape") {
        e.preventDefault();
        this.showPromptComposer = false;
        this.render();
      }
    });
    composerTextarea?.addEventListener("paste", (e) => {
      this.handleClipboardImagePaste(e, "draft");
    });

    document.getElementById("btn-open-composer")?.addEventListener("click", () => {
      this.showPromptComposer = true;
      this.render();
      setTimeout(() => {
        (document.getElementById("prompt-composer-textarea") as HTMLTextAreaElement | null)?.focus();
      }, 0);
    });
    document.getElementById("btn-chip-composer")?.addEventListener("click", () => {
      this.showPromptComposer = !this.showPromptComposer;
      this.render();
      if (this.showPromptComposer) {
        setTimeout(() => {
          (document.getElementById("prompt-composer-textarea") as HTMLTextAreaElement | null)?.focus();
        }, 0);
      }
    });
    document.getElementById("btn-chip-diff")?.addEventListener("click", () => {
      this.openRemoteGitDiff();
    });
    document.getElementById("btn-composer-close")?.addEventListener("click", () => {
      this.showPromptComposer = false;
      this.render();
    });
    document.getElementById("btn-composer-clear")?.addEventListener("click", () => {
      this.draftInput = "";
      if (composerTextarea) {
        composerTextarea.value = "";
        composerTextarea.focus();
      }
    });
    document.getElementById("btn-composer-attach")?.addEventListener("click", () => {
      const fileInp = document.getElementById("rail-file-input") as HTMLInputElement | null;
      const targetAddr = this.getFocusedTerminalAddr();
      if (!this.selectedDaemon?.is_online || !targetAddr || !fileInp) {
        this.showToastNotice(
          "err",
          this.tr(
            "Select an online host and open a terminal first.",
            "请先选择一台在线主机并打开终端会话。"
          )
        );
        return;
      }
      this.nextFileInputTarget = "draft";
      fileInp.click();
    });
    document.getElementById("btn-composer-voice")?.addEventListener("click", () => {
      void this.toggleVoiceInput();
    });
    document.getElementById("btn-composer-send")?.addEventListener("click", () => {
      sendPromptComposerPayload();
    });

    root.querySelectorAll<HTMLButtonElement>("[data-snippet-insert]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const snipId = btn.dataset.snippetInsert || "";
        const snip = this.promptSnippets.find((s) => s.id === snipId);
        if (!snip) return;
        const curDraft = composerTextarea ? composerTextarea.value : this.draftInput;
        this.draftInput = insertSnippetIntoDraft(curDraft, snip.prompt);
        this.updateDraftPreviewInPlace();
        if (composerTextarea) {
          composerTextarea.value = this.draftInput;
          composerTextarea.focus();
          composerTextarea.selectionStart = composerTextarea.value.length;
          composerTextarea.selectionEnd = composerTextarea.value.length;
        }
      });
    });

    root.querySelectorAll<HTMLButtonElement>("[data-snippet-delete]").forEach((btn) => {
      btn.addEventListener("click", async (e) => {
        e.stopPropagation();
        const snipId = btn.dataset.snippetDelete || "";
        if (!snipId || this.savingSnippetBusy) return;
        const target = this.promptSnippets.find((s) => s.id === snipId);
        if (!target) return;
        if (composerTextarea) {
          this.draftInput = composerTextarea.value;
        }
        const nextList = this.promptSnippets.filter((s) => s.id !== snipId);
        this.savingSnippetBusy = true;
        try {
          await this.savePromptSnippetsToServer(nextList);
          this.showToastNotice(
            "ok",
            this.tr(`Removed snippet "${target.title}"`, `已删除常用片段「${target.title}」`)
          );
        } catch (err: any) {
          this.showToastNotice(
            "err",
            String(err?.message || this.tr("Failed deleting snippet", "删除常用片段失败"))
          );
        } finally {
          this.savingSnippetBusy = false;
          this.render();
        }
      });
    });

    document.getElementById("btn-composer-save-snippet-toggle")?.addEventListener("click", () => {
      const currentDraft = (composerTextarea ? composerTextarea.value : this.draftInput).trim();
      if (!currentDraft) {
        this.showToastNotice(
          "err",
          this.tr(
            "Type a prompt in the composer first to save it as a snippet.",
            "请先在编排器中输入 Prompt 内容再保存为常用片段。"
          )
        );
        composerTextarea?.focus();
        return;
      }
      this.draftInput = composerTextarea ? composerTextarea.value : this.draftInput;
      this.savingSnippetOpen = !this.savingSnippetOpen;
      if (this.savingSnippetOpen) {
        this.savingSnippetTitle = deriveSnippetTitleFromPrompt(currentDraft);
      }
      this.render();
      if (this.savingSnippetOpen) {
        setTimeout(() => {
          const titleInp = document.getElementById(
            "composer-snippet-title-input"
          ) as HTMLInputElement | null;
          titleInp?.focus();
          titleInp?.select();
        }, 0);
      }
    });

    const snippetTitleInp = document.getElementById(
      "composer-snippet-title-input"
    ) as HTMLInputElement | null;
    snippetTitleInp?.addEventListener("input", () => {
      this.savingSnippetTitle = snippetTitleInp.value;
    });

    const submitSaveSnippet = async () => {
      if (this.savingSnippetBusy) return;
      const promptText = (composerTextarea ? composerTextarea.value : this.draftInput).trim();
      const rawTitle = (snippetTitleInp ? snippetTitleInp.value : this.savingSnippetTitle).trim();
      if (this.promptSnippets.length >= MAX_PROMPT_SNIPPETS) {
        this.showToastNotice(
          "err",
          this.tr(
            `Maximum ${MAX_PROMPT_SNIPPETS} saved snippets reached — delete an unused snippet first.`,
            `最多保存 ${MAX_PROMPT_SNIPPETS} 条常用片段，请先删除不常用的片段。`
          )
        );
        return;
      }
      const validated = validatePromptSnippet({
        title: rawTitle || deriveSnippetTitleFromPrompt(promptText),
        prompt: promptText,
      });
      if (!validated.ok) {
        this.showToastNotice("err", validated.error);
        return;
      }
      this.draftInput = composerTextarea ? composerTextarea.value : this.draftInput;
      this.savingSnippetBusy = true;
      this.render();
      try {
        await this.savePromptSnippetsToServer([...this.promptSnippets, validated.snippet]);
        this.savingSnippetOpen = false;
        this.savingSnippetTitle = "";
        this.showToastNotice(
          "ok",
          this.tr(
            `Saved snippet "${validated.snippet.title}" across devices.`,
            `已跨设备保存常用片段「${validated.snippet.title}」。`
          )
        );
      } catch (err: any) {
        this.showToastNotice(
          "err",
          String(err?.message || this.tr("Failed saving snippet", "保存常用片段失败"))
        );
      } finally {
        this.savingSnippetBusy = false;
        this.render();
      }
    };

    document.getElementById("btn-composer-save-snippet-confirm")?.addEventListener("click", () => {
      void submitSaveSnippet();
    });
    document.getElementById("btn-composer-save-snippet-cancel")?.addEventListener("click", () => {
      if (composerTextarea) {
        this.draftInput = composerTextarea.value;
      }
      this.savingSnippetOpen = false;
      this.savingSnippetTitle = "";
      this.render();
    });
    snippetTitleInp?.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.isComposing && !this.imeComposing) {
        e.preventDefault();
        void submitSaveSnippet();
      } else if (e.key === "Escape") {
        e.preventDefault();
        this.savingSnippetOpen = false;
        this.render();
      }
    });

    // Split View, Git Diff, ⌘F Search, Markdown Export, ⌘+Click URL, Remote File Browser & Port Preview
    document.getElementById("btn-console-split")?.addEventListener("click", () => {
      this.toggleSplitView();
    });
    document.getElementById("btn-console-diff")?.addEventListener("click", () => {
      this.openRemoteGitDiff();
    });
    document.getElementById("btn-console-files")?.addEventListener("click", () => {
      this.openRemoteFileBrowser();
    });
    document.getElementById("btn-console-port-preview")?.addEventListener("click", () => {
      this.openRemotePortPreview();
    });
    document.getElementById("btn-console-search")?.addEventListener("click", () => {
      this.showTermSearch = !this.showTermSearch;
      this.render();
      if (this.showTermSearch) {
        setTimeout(() => {
          (document.getElementById("term-search-input") as HTMLInputElement | null)?.focus();
        }, 0);
      }
    });

    // Remote Git Diff modal bindings
    document.getElementById("btn-close-git-diff")?.addEventListener("click", () => {
      this.gitDiff.open = false;
      this.render();
    });
    document.getElementById("git-diff-mask")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "git-diff-mask") {
        this.gitDiff.open = false;
        this.render();
      }
    });
    document.getElementById("btn-refresh-git-diff")?.addEventListener("click", () => {
      this.refreshRemoteGitDiff();
    });
    const gdCwdSelect = document.getElementById("gd-cwd-select") as HTMLSelectElement | null;
    gdCwdSelect?.addEventListener("change", () => {
      if (gdCwdSelect.value) this.openRemoteGitDiff(gdCwdSelect.value);
    });
    root.querySelectorAll<HTMLButtonElement>("[data-gd-file]").forEach((pill) => {
      pill.addEventListener("click", () => {
        const raw = pill.dataset.gdFile || "";
        this.gitDiff.selectedFile = raw ? raw : null;
        this.render();
      });
    });
    root.querySelectorAll<HTMLButtonElement>("[data-gd-open-file]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const rel = btn.dataset.gdOpenFile || "";
        if (rel) this.openFileFromGitDiff(rel);
      });
    });

    const termSearchInput = document.getElementById("term-search-input") as HTMLInputElement | null;
    if (termSearchInput && activeVt) {
      const screenEl = document.getElementById("term-screen-output");
      if (screenEl) this.applySearchHighlightsInPlace(screenEl, activeVt);
    }
    termSearchInput?.addEventListener("input", () => {
      this.termSearchQuery = termSearchInput.value;
      this.termSearchIdx = 0;
      if (this.selectedDaemon && this.activeTerminalAddr) {
        const vt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, this.activeTerminalAddr));
        const screenEl = document.getElementById("term-screen-output");
        if (vt && screenEl) {
          this.applySearchHighlightsInPlace(screenEl, vt);
          const matches = findWebTerminalMatches(vt.renderPlainText(), this.termSearchQuery);
          if (matches.length > 0) {
            (screenEl.children[matches[0].rowIdx] as HTMLElement | undefined)?.scrollIntoView({
              block: "center",
              behavior: "smooth",
            });
          }
        }
      }
    });
    termSearchInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        this.stepWebTerminalSearch(!e.shiftKey);
      } else if (e.key === "Escape") {
        e.preventDefault();
        this.showTermSearch = false;
        this.render();
      }
    });
    document.getElementById("term-search-prev")?.addEventListener("click", () => {
      this.stepWebTerminalSearch(false);
    });
    document.getElementById("term-search-next")?.addEventListener("click", () => {
      this.stepWebTerminalSearch(true);
    });
    document.getElementById("btn-export-session-md")?.addEventListener("click", () => {
      this.exportActiveSessionMarkdown();
    });
    document.getElementById("term-search-close")?.addEventListener("click", () => {
      this.showTermSearch = false;
      this.render();
    });

    // ⌘+Click / Ctrl+Click URL opener on primary & secondary terminal panes
    const handleTermUrlClick = (ev: MouseEvent) => {
      if (!ev.metaKey && !ev.ctrlKey) return;
      const row = (ev.target as HTMLElement | null)?.closest(".vt-row");
      if (!row) return;
      const foundUrl = firstSafeTerminalUrlInLine(row.textContent || "");
      if (foundUrl) {
        ev.preventDefault();
        window.open(foundUrl, "_blank", "noopener,noreferrer");
      }
    };
    termBox?.addEventListener("click", handleTermUrlClick);
    document.getElementById("term-box-secondary")?.addEventListener("click", handleTermUrlClick);

    const secSelect = document.getElementById("split-secondary-select") as HTMLSelectElement | null;
    secSelect?.addEventListener("change", () => {
      const next = secSelect.value;
      if (!next) return;
      if (this.secondaryTerminalAddr && this.secondaryTerminalAddr !== next && this.selectedDaemon) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "pty_detach",
          req_id: `det2-${Date.now()}`,
          terminal: this.secondaryTerminalAddr,
        });
      }
      this.secondaryTerminalAddr = next;
      this.attachAndReadTerminal(next);
      this.render();
    });
    document.getElementById("btn-swap-split")?.addEventListener("click", () => {
      if (!this.secondaryTerminalAddr || !this.activeTerminalAddr) return;
      const prevPrimary = this.activeTerminalAddr;
      this.activeTerminalAddr = this.secondaryTerminalAddr;
      this.secondaryTerminalAddr = prevPrimary;
      this.focusedPane = "primary";
      writeLocalPref("coda_last_terminal_addr", this.activeTerminalAddr);
      this.render();
    });

    // Remote File Browser modal bindings
    document.getElementById("btn-close-file-browser")?.addEventListener("click", () => {
      this.fileBrowser.open = false;
      this.render();
    });
    document.getElementById("file-browser-mask")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "file-browser-mask") {
        this.fileBrowser.open = false;
        this.render();
      }
    });
    const fbRootSelect = document.getElementById("fb-root-select") as HTMLSelectElement | null;
    fbRootSelect?.addEventListener("change", () => {
      if (fbRootSelect.value) this.openRemoteFileBrowser(fbRootSelect.value);
    });
    document.getElementById("fb-up-btn")?.addEventListener("click", () => {
      this.browseRemoteSubPath(parentSubPath(this.fileBrowser.subPath));
    });
    root.querySelectorAll<HTMLElement>("[data-fb-rel]").forEach((row) => {
      row.addEventListener("click", () => {
        const rel = row.dataset.fbRel || "";
        const isDir = row.dataset.fbDir === "1";
        if (isDir) {
          this.browseRemoteSubPath(rel);
        } else {
          this.previewRemoteFile(rel);
        }
      });
    });

    // Remote Port Preview modal bindings
    document.getElementById("btn-close-port-preview")?.addEventListener("click", () => {
      this.portPreview.open = false;
      this.render();
    });
    document.getElementById("port-preview-mask")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "port-preview-mask") {
        this.portPreview.open = false;
        this.render();
      }
    });
    const ppPortInp = document.getElementById("pp-port-input") as HTMLInputElement | null;
    const ppPathInp = document.getElementById("pp-path-input") as HTMLInputElement | null;
    ppPortInp?.addEventListener("input", () => {
      this.portPreview.portInput = ppPortInp.value;
    });
    ppPathInp?.addEventListener("input", () => {
      this.portPreview.pathInput = ppPathInp.value;
    });
    document.getElementById("port-preview-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      this.submitRemotePortPreview(
        ppPortInp ? ppPortInp.value : this.portPreview.portInput,
        ppPathInp ? ppPathInp.value : this.portPreview.pathInput
      );
    });
    root.querySelectorAll<HTMLButtonElement>("[data-pp-port]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const p = btn.dataset.ppPort || "3000";
        this.submitRemotePortPreview(p, ppPathInp ? ppPathInp.value : this.portPreview.pathInput);
      });
    });
    root.querySelectorAll<HTMLButtonElement>("[data-pp-tab]").forEach((btn) => {
      btn.addEventListener("click", () => {
        if (ppPortInp) this.portPreview.portInput = ppPortInp.value;
        if (ppPathInp) this.portPreview.pathInput = ppPathInp.value;
        this.portPreview.viewTab = btn.dataset.ppTab === "render" ? "render" : "source";
        this.render();
      });
    });
    root.querySelectorAll<HTMLButtonElement>("[data-pp-follow-path]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const nextPath = btn.dataset.ppFollowPath || "/";
        const curPort = ppPortInp ? ppPortInp.value : this.portPreview.portInput;
        this.submitRemotePortPreview(curPort, nextPath);
      });
    });

    // Mobile Quick-Command Chips
    root.querySelectorAll<HTMLButtonElement>("[data-quick-cmd]").forEach((chipBtn) => {
      chipBtn.addEventListener("click", () => {
        const cmd = chipBtn.dataset.quickCmd || "";
        const targetAddr = this.getFocusedTerminalAddr();
        if (!cmd || !this.selectedDaemon || !targetAddr) return;
        this.sendEncrypted(this.selectedDaemon, {
          op: "send_terminal_text",
          req_id: this.nextReqId("qcmd"),
          terminal: targetAddr,
          text: cmd,
        });
      });
    });

    // Console ⌘K Command Palette
    document.getElementById("btn-console-cmd-k")?.addEventListener("click", () => {
      this.showCmdPalette = true;
      this.cmdPaletteQuery = "";
      this.cmdPaletteSelectedIdx = 0;
      this.render();
      setTimeout(() => {
        (document.getElementById("console-cmd-input") as HTMLInputElement | null)?.focus();
      }, 0);
    });
    document.getElementById("console-cmd-mask")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "console-cmd-mask") {
        this.showCmdPalette = false;
        this.render();
      }
    });
    const runConsolePaletteIdx = (idx: number) => {
      const items = this.getConsolePaletteItems(this.cmdPaletteQuery);
      if (items.length === 0) return;
      const clamped = Math.max(0, Math.min(items.length - 1, idx));
      const chosen = items[clamped];
      if (!chosen) return;
      this.showCmdPalette = false;
      this.cmdPaletteQuery = "";
      this.cmdPaletteSelectedIdx = 0;
      chosen.action();
      this.render();
    };
    const syncConsolePaletteSelectionInPlace = (scrollIntoView: boolean) => {
      const listEl = document.getElementById("console-cmd-list");
      if (!listEl) return;
      const rows = listEl.querySelectorAll<HTMLButtonElement>("[data-cmd-idx]");
      if (rows.length === 0) return;
      const clamped = Math.max(0, Math.min(rows.length - 1, this.cmdPaletteSelectedIdx));
      this.cmdPaletteSelectedIdx = clamped;
      rows.forEach((row, i) => {
        const active = i === clamped;
        row.classList.toggle("active", active);
        row.setAttribute("aria-selected", active ? "true" : "false");
        if (active && scrollIntoView && typeof row.scrollIntoView === "function") {
          row.scrollIntoView({ block: "nearest" });
        }
      });
    };
    const bindConsolePaletteRows = () => {
      document.querySelectorAll<HTMLButtonElement>("[data-cmd-idx]").forEach((btn) => {
        btn.addEventListener("click", () => {
          runConsolePaletteIdx(Number(btn.dataset.cmdIdx || "0"));
        });
        btn.addEventListener("mouseenter", () => {
          this.cmdPaletteSelectedIdx = Number(btn.dataset.cmdIdx || "0");
          syncConsolePaletteSelectionInPlace(false);
        });
      });
    };
    bindConsolePaletteRows();
    const consoleCmdInput = document.getElementById("console-cmd-input") as HTMLInputElement | null;
    consoleCmdInput?.addEventListener("compositionstart", () => {
      this.imeComposing = true;
    });
    consoleCmdInput?.addEventListener("compositionend", () => {
      this.imeComposing = false;
    });
    consoleCmdInput?.addEventListener("blur", () => {
      this.imeComposing = false;
    });
    consoleCmdInput?.addEventListener("input", () => {
      this.cmdPaletteQuery = consoleCmdInput.value;
      this.cmdPaletteSelectedIdx = 0;
      const listEl = document.getElementById("console-cmd-list");
      if (!listEl) return;
      const items = this.getConsolePaletteItems(this.cmdPaletteQuery);
      listEl.innerHTML = this.renderConsolePaletteRowsHtml(items);
      bindConsolePaletteRows();
    });
    consoleCmdInput?.addEventListener("keydown", (e) => {
      if ((e.key === "ArrowDown" || e.key === "ArrowUp") && !e.isComposing && !this.imeComposing) {
        e.preventDefault();
        const items = this.getConsolePaletteItems(this.cmdPaletteQuery);
        this.cmdPaletteSelectedIdx = stepPaletteSelectionIndex(
          this.cmdPaletteSelectedIdx,
          items.length,
          e.key === "ArrowDown" ? 1 : -1
        );
        syncConsolePaletteSelectionInPlace(true);
        return;
      }
      // V3-24: Guard Enter with IME composition check so CJK candidate selection does not execute command.
      if (e.key === "Enter" && !e.isComposing && !this.imeComposing) {
        e.preventDefault();
        runConsolePaletteIdx(this.cmdPaletteSelectedIdx);
      } else if (e.key === "Escape") {
        e.preventDefault();
        this.showCmdPalette = false;
        this.render();
      }
    });
    document.getElementById("desktop-prompt-input")?.addEventListener("paste", (e) => {
      this.handleClipboardImagePaste(e as ClipboardEvent, "draft");
    });
    document.getElementById("kb-input")?.addEventListener("paste", (e) => {
      this.handleClipboardImagePaste(e as ClipboardEvent, "draft");
    });

    document.getElementById("btn-logout")?.addEventListener("click", async () => {
      await this.unsubscribeWebPushForSignOut();
      try {
        if (this.token) {
          await this.api("/api/v1/auth/logout", "POST", {});
        }
      } catch {
        // Ignore network/revoked errors during local teardown
      } finally {
        await performLocalSignOut();
      }
    });

    document.getElementById("btn-kb-toggle")?.addEventListener("click", () => {
      this.showPromptComposer = !this.showPromptComposer;
      this.showKeyboardInputBar = false;
      this.render();
      if (this.showPromptComposer) {
        setTimeout(() => {
          (document.getElementById("prompt-composer-textarea") as HTMLTextAreaElement | null)?.focus();
        }, 0);
      }
    });
    document.getElementById("btn-voice-mic")?.addEventListener("click", () => {
      void this.toggleVoiceInput();
    });
    document.getElementById("btn-voice-mic-desktop")?.addEventListener("click", () => {
      void this.toggleVoiceInput();
    });
    document.getElementById("btn-hide-touch-pad")?.addEventListener("click", () => {
      this.showMobileKeypadOnDesktop = false;
      this.render();
    });

    const kbInput = document.getElementById("kb-input") as HTMLInputElement | null;
    kbInput?.addEventListener("compositionstart", () => {
      this.imeComposing = true;
    });
    kbInput?.addEventListener("compositionend", () => {
      this.imeComposing = false;
      this.draftInput = kbInput.value;
    });
    kbInput?.addEventListener("blur", () => {
      this.imeComposing = false;
    });
    kbInput?.addEventListener("input", () => {
      this.draftInput = kbInput.value;
    });
    kbInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.isComposing || this.imeComposing)) {
        e.preventDefault();
      }
    });

    // Mobile 1-step submit: sends command directly to focused PTY!
    document.getElementById("kb-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      if (this.imeComposing) return;
      const inp = document.getElementById("kb-input") as HTMLInputElement | null;
      const text = inp ? inp.value : this.draftInput;
      const targetAddr = this.getFocusedTerminalAddr();
      if (this.selectedDaemon && targetAddr) {
        if (text.trim()) {
          this.sendEncrypted(this.selectedDaemon, {
            op: "send_terminal_text",
            req_id: this.nextReqId("tx"),
            terminal: targetAddr,
            text,
          });
        } else {
          this.sendPtyInputBytesTo(targetAddr, "\r");
        }
      }
      this.draftInput = "";
      this.showKeyboardInputBar = false;
      this.render();
    });

    const desktopInput = document.getElementById("desktop-prompt-input") as HTMLInputElement | null;
    desktopInput?.addEventListener("compositionstart", () => {
      this.imeComposing = true;
    });
    desktopInput?.addEventListener("compositionend", () => {
      this.imeComposing = false;
      this.draftInput = desktopInput.value;
    });
    desktopInput?.addEventListener("blur", () => {
      this.imeComposing = false;
    });
    desktopInput?.addEventListener("input", () => {
      this.draftInput = desktopInput.value;
    });
    desktopInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.isComposing || this.imeComposing)) {
        e.preventDefault();
      }
    });

    document.getElementById("btn-toggle-touch-pad")?.addEventListener("click", () => {
      this.showMobileKeypadOnDesktop = !this.showMobileKeypadOnDesktop;
      this.render();
    });

    document.getElementById("desktop-prompt-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      if (this.imeComposing) return;
      const inp = document.getElementById("desktop-prompt-input") as HTMLInputElement | null;
      const text = inp ? inp.value : this.draftInput;
      const targetAddr = this.getFocusedTerminalAddr();
      if (this.selectedDaemon && targetAddr) {
        if (text.trim()) {
          this.sendEncrypted(this.selectedDaemon, {
            op: "send_terminal_text",
            req_id: this.nextReqId("tx"),
            terminal: targetAddr,
            text,
          });
        } else {
          // Empty Enter sends bare CR (\r) for interactive prompts
          this.sendPtyInputBytesTo(targetAddr, "\r");
        }
      }
      this.draftInput = "";
      this.updateDraftPreviewInPlace();
      if (inp) {
        inp.value = "";
        inp.focus();
      }
      if (termBox) {
        termBox.scrollTop = termBox.scrollHeight;
        this.syncScrollBottomPill(termBox);
      }
    });

    // New Agent Modal + Remote Directory Picker + Git Clone Bootstrapper
    document.getElementById("rail-spawn-btn")?.addEventListener("click", () => {
      this.openNewAgentModal();
    });
    document.getElementById("btn-close-spawn")?.addEventListener("click", () => {
      this.showSpawnModal = false;
      this.render();
    });
    document.getElementById("spawn-modal-bg")?.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).id === "spawn-modal-bg") {
        this.showSpawnModal = false;
        this.render();
      }
    });
    document.getElementById("btn-spawn-browse-dirs")?.addEventListener("click", () => {
      if (this.spawnDirPicker.open) {
        this.captureSpawnFormDraftFromDom();
        this.spawnDirPicker.open = false;
        this.render();
      } else {
        this.openSpawnDirectoryPicker();
      }
    });
    document.getElementById("btn-spawn-dir-close")?.addEventListener("click", () => {
      this.captureSpawnFormDraftFromDom();
      this.spawnDirPicker.open = false;
      this.render();
    });
    document.getElementById("btn-spawn-dir-up")?.addEventListener("click", () => {
      this.stepUpSpawnDirectoryPicker();
    });
    document.getElementById("btn-spawn-dir-use")?.addEventListener("click", () => {
      this.captureSpawnFormDraftFromDom();
      const chosen = joinRootAndRel(
        this.spawnDirPicker.rootPath || "~",
        this.spawnDirPicker.subPath
      );
      this.spawnCwd = chosen;
      this.spawnDirPicker.open = false;
      this.render();
    });
    root.querySelectorAll<HTMLButtonElement>("[data-spawn-dir-rel]").forEach((row) => {
      row.addEventListener("click", () => {
        const rel = row.dataset.spawnDirRel || "";
        this.browseSpawnDirSubPath(rel);
      });
    });

    document.getElementById("btn-spawn-toggle-bootstrap")?.addEventListener("click", () => {
      this.captureSpawnFormDraftFromDom();
      this.spawnBootstrapOpen = !this.spawnBootstrapOpen;
      this.spawnBootstrapMsg = null;
      this.render();
    });
    document.getElementById("btn-spawn-boot-close")?.addEventListener("click", () => {
      this.captureSpawnFormDraftFromDom();
      this.spawnBootstrapOpen = false;
      this.render();
    });
    const bootFolderInp = document.getElementById("spawn-boot-folder") as HTMLInputElement | null;
    bootFolderInp?.addEventListener("input", () => {
      this.spawnBootstrapFolderName = bootFolderInp.value;
      this.bootFolderManuallyEdited = bootFolderInp.value.trim().length > 0;
    });
    const bootGitUrlInp = document.getElementById("spawn-boot-git-url") as HTMLInputElement | null;
    bootGitUrlInp?.addEventListener("input", () => {
      this.spawnBootstrapGitUrl = bootGitUrlInp.value;
      const nextFolder = computeBootstrapAutoFolder(
        bootGitUrlInp.value,
        bootFolderInp ? bootFolderInp.value : this.spawnBootstrapFolderName,
        this.bootFolderManuallyEdited
      );
      this.spawnBootstrapFolderName = nextFolder;
      if (bootFolderInp && bootFolderInp.value !== nextFolder) {
        bootFolderInp.value = nextFolder;
      }
    });
    document.getElementById("btn-spawn-run-bootstrap")?.addEventListener("click", () => {
      this.submitBootstrapWorkspace();
    });

    document.getElementById("spawn-agent-form")?.addEventListener("submit", (e) => {
      e.preventDefault();
      if (!this.selectedDaemon?.is_online) return;
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      const agent = String(fd.agent || "shell").trim() || "shell";
      const cwd = String(fd.cwd || "").trim() || null;
      const initial_prompt = String(fd.initial_prompt || "").trim() || null;
      const { cols, rows } = this.computeTerminalDimsFromDom();
      this.termCols = cols;
      this.termRows = rows;
      this.sendEncrypted(this.selectedDaemon, {
        op: "launch_agent",
        req_id: `run-${Date.now()}`,
        agent,
        name: null,
        cwd,
        initial_prompt,
        cols,
        rows,
      });
      this.showSpawnModal = false;
      this.render();
    });

    document.getElementById("rail-refresh-btn")?.addEventListener("click", () => {
      const fileInp = document.getElementById("rail-file-input") as HTMLInputElement | null;
      const targetAddr = this.getFocusedTerminalAddr();
      if (!this.selectedDaemon?.is_online || !targetAddr || !fileInp) {
        this.showToastNotice(
          "err",
          this.tr(
            "Select an online host and open a terminal first.",
            "请先选择一台在线主机并打开终端会话。"
          )
        );
        return;
      }
      this.nextFileInputTarget = "pty";
      fileInp.click();
    });

    document.getElementById("rail-file-input")?.addEventListener("change", async (ev) => {
      const inp = ev.target as HTMLInputElement;
      const files = Array.from(inp.files || []);
      const targetMode = this.nextFileInputTarget;
      this.nextFileInputTarget = "pty";
      inp.value = "";
      if (files.length === 0) return;
      const pinnedDaemon = this.selectedDaemon;
      const pinnedTerminal = this.getFocusedTerminalAddr();
      for (const file of files) {
        await this.uploadSingleFileToHost(file, pinnedDaemon, pinnedTerminal, undefined, targetMode);
      }
    });

    document.getElementById("rail-paste-btn")?.addEventListener("click", async () => {
      const canReadText = typeof navigator.clipboard?.readText === "function";
      const canReadItems = typeof navigator.clipboard?.read === "function";
      if (!canReadText && !canReadItems) {
        this.showToastNotice(
          "err",
          this.tr(
            "Clipboard API unavailable in this browser context",
            "当前浏览器上下文不支持剪贴板读取"
          )
        );
        return;
      }
      try {
        let clip = "";
        let readTextError: unknown = null;
        if (canReadText) {
          try {
            clip = await navigator.clipboard.readText();
          } catch (err) {
            readTextError = err;
          }
        }
        if (clip) {
          this.draftInput = this.draftInput ? `${this.draftInput}${clip}` : clip;
          this.updateDraftPreviewInPlace();
          const composerArea = document.getElementById(
            "prompt-composer-textarea"
          ) as HTMLTextAreaElement | null;
          const desktopInp = document.getElementById(
            "desktop-prompt-input"
          ) as HTMLInputElement | null;
          if (composerArea) {
            composerArea.value = this.draftInput;
            composerArea.focus();
          } else if (desktopInp) {
            desktopInp.value = this.draftInput;
            desktopInp.focus();
          } else {
            this.render();
          }
          return;
        }

        if (canReadItems) {
          const items = await navigator.clipboard.read();
          const imageBlobs = await extractImagesFromClipboardItems(items);
          if (imageBlobs.length > 0) {
            const pinnedDaemon = this.selectedDaemon;
            const pinnedTerminal = this.getFocusedTerminalAddr();
            const targetMode: "pty" | "draft" = this.showPromptComposer ? "draft" : "pty";
            const baseTs = Date.now();
            for (let idx = 0; idx < imageBlobs.length; idx += 1) {
              const blob = imageBlobs[idx];
              const fname = buildClipboardImageFilename(baseTs, idx, blob.type, "");
              await this.uploadSingleFileToHost(
                blob,
                pinnedDaemon,
                pinnedTerminal,
                fname,
                targetMode
              );
            }
            return;
          }
        }

        if (readTextError) {
          throw readTextError;
        }
        this.showToastNotice(
          "err",
          this.tr("Clipboard is empty.", "剪贴板当前为空。")
        );
      } catch {
        this.showToastNotice(
          "err",
          this.tr(
            "Clipboard permission denied; paste directly with Ctrl+V / ⌘V",
            "剪贴板读取未授权，请直接使用 Ctrl+V / ⌘V 粘贴"
          )
        );
      }
    });

    const voiceCfgForm = document.getElementById("voice-cfg-form") as HTMLFormElement | null;
    const markVoiceDirty = () => {
      this.settingsDirtyFields.add("voice");
    };
    voiceCfgForm?.addEventListener("input", markVoiceDirty);
    voiceCfgForm?.addEventListener("change", markVoiceDirty);
    voiceCfgForm?.addEventListener("submit", (e) => {
      e.preventDefault();
      const fd = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries()) as any;
      const nextBaseUrl = String(fd.base_url || "https://api.openai.com/v1").trim();
      if (!this.isSafeVoiceBaseUrl(nextBaseUrl)) {
        this.settingsFeedback = {
          kind: "err",
          text: this.tr("Base URL must use https:// (or localhost http://).", "Base URL 必须使用 https://（或本机 localhost）。"),
        };
        this.renderSettingsPreservingInputs();
        return;
      }
      const typedKey = String(fd.api_key || "").trim();
      const preservedHostRedacted = !typedKey && this.voiceConfig.api_key.startsWith("••");
      this.voiceApiKeyLocal = typedKey;
      const wireConfig: VoiceModelConfig = {
        provider: String(fd.provider || "openai").trim() || "openai",
        base_url: nextBaseUrl,
        api_key: typedKey,
        model: String(fd.model || "gpt-4o-mini-transcribe").trim() || "gpt-4o-mini-transcribe",
      };
      this.voiceConfig = {
        ...wireConfig,
        api_key: preservedHostRedacted ? "••••••••" : typedKey,
      };
      this.settingsDirtyFields.delete("voice");
      if (this.selectedDaemon?.is_online) {
        this.sendEncrypted(this.selectedDaemon, {
          op: "set_voice_config",
          req_id: this.nextReqId("v"),
          config: wireConfig,
        });
      }
      this.settingsFeedback = {
        kind: "ok",
        text: this.tr("Voice config saved.", "语音模型配置已保存。"),
      };
      this.render();
    });

    document.querySelectorAll("[data-kill-term]").forEach((el) => {
      el.addEventListener("click", (e) => {
        e.stopPropagation();
        const addr = (el as HTMLElement).dataset.killTerm!;
        if (!addr || !this.selectedDaemon) return;
        if (addr === this.activeTerminalAddr || addr === this.secondaryTerminalAddr) {
          this.cancelActiveVoiceCapture();
        }
        this.sendEncrypted(this.selectedDaemon, {
          op: "kill_terminal",
          req_id: this.nextReqId("kt"),
          terminal: addr,
        });
        this.terminals = this.terminals.filter((t) => t.address !== addr);
        this.terminalsByPeer.set(this.selectedDaemon.id, [...this.terminals]);
        this.vtStreamActive.delete(this.vtKey(this.selectedDaemon.id, addr));
        this.vtTerminals.delete(this.vtKey(this.selectedDaemon.id, addr));
        if (this.activeTerminalAddr === addr) {
          const nextPrimary = this.terminals[0]?.address ?? null;
          this.activeTerminalAddr = nextPrimary;
          if (nextPrimary && this.secondaryTerminalAddr === nextPrimary) {
            const nextSec = this.terminals.find((t) => t.address !== nextPrimary)?.address ?? null;
            this.secondaryTerminalAddr = nextSec;
            if (nextSec && this.splitMode) {
              this.attachAndReadTerminal(nextSec);
            }
          }
          if (this.activeTerminalAddr) {
            this.attachAndReadTerminal(this.activeTerminalAddr);
          }
        } else if (this.secondaryTerminalAddr === addr) {
          const nextSec = this.splitMode
            ? this.terminals.find((t) => t.address !== this.activeTerminalAddr)?.address ?? null
            : null;
          this.secondaryTerminalAddr = nextSec;
          if (nextSec) {
            this.attachAndReadTerminal(nextSec);
          }
        }
        if (!this.secondaryTerminalAddr) {
          this.focusedPane = "primary";
        }
        this.render();
      });
    });

    document.querySelectorAll("[data-dev]").forEach((el) => {
      el.addEventListener("click", () => {
        const devId = (el as HTMLElement).dataset.dev!;
        const peer = this.devices.find((d) => d.id === devId);
        if (!peer) return;
        this.selectDaemon(peer);
        if (isNarrowViewport) this.showSidebar = false;
        this.render();
      });
    });

    document.querySelectorAll("[data-term]").forEach((el) => {
      el.addEventListener("click", () => {
        const nextAddr = (el as HTMLElement).dataset.term!;
        if (!nextAddr) return;
        const prevPrimary = this.activeTerminalAddr;
        if (prevPrimary && prevPrimary !== nextAddr) {
          this.cancelActiveVoiceCapture();
          if (!(this.splitMode && this.secondaryTerminalAddr === nextAddr && prevPrimary)) {
            this.detachActiveTerminal();
          }
        }
        if (this.splitMode && this.secondaryTerminalAddr === nextAddr) {
          this.secondaryTerminalAddr =
            prevPrimary && prevPrimary !== nextAddr
              ? prevPrimary
              : this.terminals.find((t) => t.address !== nextAddr)?.address ?? null;
        }
        this.activeTerminalAddr = nextAddr;
        this.focusedPane = "primary";
        writeLocalPref("coda_last_terminal_addr", nextAddr);
        this.attachAndReadTerminal(nextAddr);
        if (this.splitMode && this.secondaryTerminalAddr) {
          this.attachAndReadTerminal(this.secondaryTerminalAddr);
        }
        if (isNarrowViewport) this.showSidebar = false;
        this.render();
      });
    });

    document.querySelectorAll("[data-k]").forEach((el) => {
      el.addEventListener("click", () => {
        const k = (el as HTMLElement).dataset.k!;
        const targetAddr = this.getFocusedTerminalAddr();
        if (k === "slash") {
          if (this.draftInput) {
            this.draftInput = `${this.draftInput}/`;
            this.render();
          } else {
            this.sendPtyInputBytesTo(targetAddr, "/");
          }
          return;
        }
        if (k === "backspace") {
          if (this.draftInput.length > 0) {
            this.draftInput = this.draftInput.slice(0, -1);
            this.render();
          } else {
            this.sendPtyInputBytesTo(targetAddr, "\x7f");
          }
          return;
        }
        if (k === "enter") {
          if (this.selectedDaemon && targetAddr) {
            if (this.draftInput.trim()) {
              this.sendEncrypted(this.selectedDaemon, {
                op: "send_terminal_text",
                req_id: this.nextReqId("tx"),
                terminal: targetAddr,
                text: this.draftInput,
              });
            } else {
              this.sendPtyInputBytesTo(targetAddr, "\r");
            }
          }
          this.draftInput = "";
          this.render();
          return;
        }
        if (this.selectedDaemon && targetAddr) {
          const targetVt = this.vtTerminals.get(this.vtKey(this.selectedDaemon.id, targetAddr));
          const appCursor = Boolean(targetVt?.appCursorKeys);
          const rawMap: Record<string, string> = {
            esc: "\x1b",
            tab: "\t",
            "shift+tab": "\x1b[Z",
            "ctrl+c": "\x03",
            "ctrl+d": "\x04",
            up: appCursor ? "\x1bOA" : "\x1b[A",
            down: appCursor ? "\x1bOB" : "\x1b[B",
            right: appCursor ? "\x1bOC" : "\x1b[C",
            left: appCursor ? "\x1bOD" : "\x1b[D",
          };
          if (rawMap[k]) {
            this.sendPtyInputBytesTo(targetAddr, rawMap[k]);
          } else {
            this.sendEncrypted(this.selectedDaemon, {
              op: "send_terminal_keys",
              req_id: this.nextReqId("k"),
              terminal: targetAddr,
              keys: [k],
            });
          }
        }
      });
    });
  }

  private escapeHtml(str: string): string {
    return escapeHtml(str);
  }
}

if (typeof window !== "undefined" && typeof document !== "undefined" && document.getElementById("app")) {
  void new CodaTerminalWebApp().init();
}
