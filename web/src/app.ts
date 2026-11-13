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

function codaLogoSVG(size = 24) {
  return `<svg width="${size}" height="${size}" viewBox="0 0 64 64" fill="none" aria-hidden="true">
    <defs>
      <linearGradient id="coda-nav-bg" x1="32" y1="2" x2="32" y2="62" gradientUnits="userSpaceOnUse">
        <stop offset="0%" stop-color="#171920" />
        <stop offset="100%" stop-color="#0b0c10" />
      </linearGradient>
      <linearGradient id="coda-nav-arc" x1="14" y1="14" x2="48" y2="50" gradientUnits="userSpaceOnUse">
        <stop offset="0%" stop-color="#f59e0b" />
        <stop offset="100%" stop-color="#e06c50" />
      </linearGradient>
    </defs>
    <rect x="2" y="2" width="60" height="60" rx="15" fill="url(#coda-nav-bg)" stroke="#2a2d37" stroke-width="1.5" />
    <path d="M 44.37 44.37 A 17.5 17.5 0 1 1 38.70 15.83" stroke="url(#coda-nav-arc)" stroke-width="5.5" stroke-linecap="round" />
    <path d="M 25.5 24.5 L 34 32 L 25.5 39.5" stroke="#f4f4f5" stroke-width="4.5" stroke-linecap="round" stroke-linejoin="round" />
    <circle cx="46" cy="19" r="4.2" fill="#22c55e" />
  </svg>`;
}

function codaNodeDotSVG(color = "#e06c50") {
  return `<span style="display:inline-flex;align-items:center;justify-content:center;width:16px;height:16px;border-radius:4px;border:1px solid rgba(255,255,255,0.14);background:#161820;flex-shrink:0;">
    <span style="width:7px;height:7px;border-radius:99px;background:${color};"></span>
  </span>`;
}

class CodaTerminalWebApp {
  private token: string | null = localStorage.getItem("coda_token");
  private authMode: "login" | "register" = "login";
  private previewTab: "coda" | "server" | "ios" = "coda";
  private showMobileKeypadOnDesktop = false;
  private siteInfo: SiteInfo | null = null;
  private pendingActivationCode: string = "";
  private pendingDeviceInfo: DeviceAuthPublicInfo | null = null;
  private activationSuccessMsg = "";
  private copiedToast = "";
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

  async init() {
    const params = new URLSearchParams(window.location.search);
    const codeParam = params.get("code") || "";
    if (window.location.pathname === "/activate" || codeParam) {
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
      } catch {
        this.token = null;
        localStorage.removeItem("coda_token");
      }
    }
    this.render();
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
      this.errorMsg = err.message || "授权码无效或已过期";
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

  render() {
    const root = document.getElementById("app")!;
    const baseUrl = this.siteInfo?.public_base_url || window.location.origin;
    const installCmd = this.siteInfo?.install_command || `curl -fsSL ${baseUrl}/install.sh | sh`;
    const ver = this.siteInfo?.version || "v0.3.5";
    const dl = this.siteInfo?.downloads;
    const isActivateView =
      window.location.pathname === "/activate" || !!this.pendingActivationCode;

    if (!this.user) {
      const openReg = this.siteInfo?.open_registration ?? true;
      const previewContent =
        this.previewTab === "server"
          ? {
              host: "green-jp-02 · Ubuntu 24.04 ARM64",
              url: `${baseUrl}/nodes/green-jp-02`,
              title: "coda-server Docker Compose · 127.0.0.1:8080 (Cloudflare Tunnel)",
              stream: `● [postgres:18] healthy · /var/lib/postgresql\n● [coda-coordinator] 127.0.0.1:8080 -> wss://codahub.dev/v1/tunnel/ws\n● Zero-SNI Raw TCP Relay :24443 · E2EE Blind Forwarding\n\n* Churned for 12s · done 16:40 PM\n✓ Host keys: Ed25519 + X25519 verified`,
            }
          : this.previewTab === "ios"
            ? {
                host: "iPhone / iPad Safari PWA · LAN Auto-Switch",
                url: `${baseUrl}/mobile`,
                title: "移动端单手 E2EE 终端与语音模型输入",
                stream: `● Detected same Wi-Fi LAN · 250ms Happy Eyeballs probe succeeded\n● Switched route: WSS 443 -> ws://192.168.1.20:24444 (0.8ms RTT)\n● ChaCha20-Poly1305 encrypted PTY attached to MacBook-Pro:t1\n\n* Ready · Touch Keypad + Whisper Voice Config Active`,
              }
            : {
                host: "MacBook-Pro · Rust + GPUI 120Hz Workbench",
                url: `${baseUrl}/workspaces/coda`,
                title: "GPUI 桌面工作台与 Herdr PTY 流式同步",
                stream: `  - 本地 Herdr Core (hc 0.1.20) 在线，工作区 Git Worktree 自动扫描完成\n  - 零知识隧道已绑定账号 @vampire：服务器永远无法解密终端明文\n\n● Background command "cargo test --workspace" completed (exit code 0)\n\n* Baked for 38s · done 16:42 PM\n✓ All tests & E2EE SmartPath P2P verified`,
              };

      root.innerHTML = `
        <div class="hw-page">
          <nav class="hw-nav">
            <div class="hw-nav-inner">
              <a class="hw-brand" href="/">
                ${codaLogoSVG(26)}
                <span>Coda</span>
                <span class="badge coda-mono" style="font-size:11px;">${this.escapeHtml(ver)}</span>
              </a>
              <div class="hw-nav-links">
                <a href="#features">核心特性</a>
                <a href="#architecture">加密与路由</a>
                <a href="#compare">能力对比</a>
                <a href="#downloads">全端下载</a>
                <a href="#portal-auth-section">接入控制台</a>
              </div>
              <div style="display:flex;gap:8px;align-items:center;">
                <button id="btn-nav-login" class="btn-ghost" style="padding:6px 12px;font-size:12px;">登录</button>
                <button id="btn-nav-register" class="btn-primary" style="padding:6px 12px;font-size:12px;">免费注册</button>
              </div>
            </div>
          </nav>

          ${
            isActivateView || this.pendingDeviceInfo
              ? `
            <div class="hw-container" style="padding-top:24px;">
              <section class="portal-panel" style="border-color:rgba(56,189,248,0.45);">
                <div style="display:flex;justify-content:space-between;align-items:center;gap:12px;flex-wrap:wrap;">
                  <div>
                    <div style="font-size:16px;font-weight:700;">🔑 浏览器设备码授权 (/activate)</div>
                    <div style="font-size:13px;color:var(--muted);margin-top:4px;">请先在下方登录或注册你的 Coda 账号，即可一键将远端电脑/服务器加入你的私有终端网络。</div>
                  </div>
                  <span class="coda-mono" style="font-size:20px;font-weight:700;color:var(--sky);letter-spacing:2px;">
                    ${this.escapeHtml(this.pendingActivationCode || "XXXX-XXXX")}
                  </span>
                </div>
                ${
                  this.pendingDeviceInfo
                    ? `<div class="coda-mono" style="margin-top:12px;padding:10px 12px;border-radius:10px;background:#0b0c0e;border:1px solid var(--line-strong);font-size:12px;color:#d4d4d8;">
                        待授权主机：<b>${this.escapeHtml(this.pendingDeviceInfo.device_name)}</b> · 平台：${this.escapeHtml(this.pendingDeviceInfo.platform)} · 角色：${this.escapeHtml(this.pendingDeviceInfo.role)} · 状态：${this.escapeHtml(this.pendingDeviceInfo.status)}
                      </div>`
                    : ""
                }
              </section>
            </div>
          `
              : ""
          }

          <section class="hw-hero">
            <div class="hw-hero-glow"></div>
            <div class="hw-container">
              <div class="hw-hero-head">
                <p class="hw-eyebrow">Zero-Knowledge E2EE · macOS GPUI / Linux Daemon / iOS &amp; Web</p>
                <h1 class="hw-h1">
                  十台算力主机常驻在线，<br />登录一次，随处掌控所有 Agent。
                </h1>
                <p class="hw-lead">
                  Coda 将你的海外/国内 Linux 服务器、主力 Mac 桌面与随身手机浏览器编织进同一张零知识加密网。
                  底层由 <code>herdr-core</code> 持久化守护终端进程，同 Wi-Fi 下自动毫秒级直连，跨网经 Cloudflare Tunnel WSS 加密转发。
                </p>

                <div class="hw-cta-row">
                  ${
                    dl
                      ? `<a class="btn-primary" href="${dl.desktop_macos_dmg}" target="_blank"> 下载 macOS 桌面端 (.dmg)</a>`
                      : `<a class="btn-primary" href="#downloads">下载桌面与服务端</a>`
                  }
                  <a class="btn-ghost" href="#portal-auth-section">登录 / 注册我的控制面板 →</a>
                </div>

                <div class="hw-cmd-pill">
                  <code>$ ${this.escapeHtml(installCmd)}</code>
                  <button id="btn-copy-install" class="btn-primary" style="padding:7px 14px;font-size:12px;flex-shrink:0;">
                    ${this.copiedToast ? this.escapeHtml(this.copiedToast) : "复制安装命令"}
                  </button>
                </div>
                <p class="hw-subhint">支持 macOS 14+ (Apple Silicon) 与 Linux x86_64 / ARM64 · 无需提前配置密钥，运行后浏览器一键确认设备码即可入网</p>
              </div>

              <!-- Interactive Highwind-Style 3-Pane Desktop App Showcase -->
              <div class="hw-window">
                <div class="hw-win-top">
                  <div style="display:flex;align-items:center;gap:12px;">
                    <div class="hw-traffic">
                      <span style="background:#ef4444;"></span>
                      <span style="background:#f59e0b;"></span>
                      <span style="background:#22c55e;"></span>
                    </div>
                    <div class="hw-win-tabs">
                      <button class="hw-win-tab ${this.previewTab === "coda" ? "active" : ""}" data-preview-tab="coda">■ coda  2/2</button>
                      <button class="hw-win-tab ${this.previewTab === "server" ? "active" : ""}" data-preview-tab="server">● green-jp-02  1/1</button>
                      <button class="hw-win-tab ${this.previewTab === "ios" ? "active" : ""}" data-preview-tab="ios">● iPhone / Web  1/1</button>
                    </div>
                  </div>
                  <div class="coda-mono" style="font-size:11px;color:var(--emerald);">● Zero-Knowledge E2EE · codahub.dev</div>
                </div>

                <div class="hw-win-grid">
                  <!-- Col 1: PROJECTS Tree -->
                  <div class="hw-win-col" style="background:#111216;">
                    <div class="hw-win-label" style="margin-top:0;">PROJECTS</div>
                    <div class="hw-win-item active"><span>▾ coda</span><span class="coda-mono" style="color:var(--emerald);">2/2</span></div>
                    <div class="hw-win-label">SERVICES</div>
                    <div class="hw-win-item"><span><span style="color:var(--emerald);">●</span> herdr-core daemon</span><span class="coda-mono" style="color:var(--faint);">live</span></div>
                    <div class="hw-win-item"><span><span style="color:var(--emerald);">●</span> codahub.dev tunnel</span><span class="coda-mono" style="color:var(--sky);">WSS</span></div>
                    <div class="hw-win-label">AGENTS</div>
                    <div class="hw-win-item active"><span><span style="color:var(--coral);">●</span> Claude Code · main</span><span class="coda-mono" style="color:var(--emerald);">+1233</span></div>
                    <div class="hw-win-item"><span><span style="color:var(--sky);">●</span> Codex · relay</span><span class="coda-mono" style="color:var(--emerald);">+842</span></div>
                    <div class="hw-win-label">ACTIONS</div>
                    <div class="hw-win-item"><span><span style="color:var(--brand-amber);">▸</span> cargo test --workspace</span></div>
                    <div class="hw-win-item"><span><span style="color:var(--brand-amber);">▸</span> coda-daemon status</span></div>
                  </div>

                  <!-- Col 2: Center Workspace Preview -->
                  <div class="hw-win-col" style="background:#0b0c0f;display:flex;flex-direction:column;justify-content:space-between;">
                    <div>
                      <div style="display:flex;align-items:center;gap:8px;padding:6px 10px;border-radius:8px;background:#14161b;border:1px solid var(--line);font-family:var(--mono);font-size:11px;margin-bottom:14px;">
                        <span style="color:var(--emerald);">dev</span>
                        <span style="color:var(--fg);">${this.escapeHtml(previewContent.url)}</span>
                      </div>
                      <div style="padding:18px;border-radius:12px;background:#111317;border:1px solid var(--line);">
                        <div class="coda-mono" style="font-size:11px;color:var(--brand-amber);margin-bottom:6px;">${this.escapeHtml(previewContent.host)}</div>
                        <div style="font-size:16px;font-weight:600;margin-bottom:8px;">${this.escapeHtml(previewContent.title)}</div>
                        <p style="font-size:13px;color:var(--muted);line-height:1.65;">
                          服务端与主控只转发 <code>ChaCha20-Poly1305</code> 密文信封。无论是 macOS 原生 GPUI 三栏工作台还是 iPhone Safari，均使用本地持久化的 <code>Ed25519 + X25519</code> 密钥直接端到端验证。
                        </p>
                      </div>
                    </div>
                    <div class="coda-mono" style="display:flex;justify-content:space-between;font-size:11px;color:var(--faint);padding-top:12px;border-top:1px solid var(--line);">
                      <span>● coda / main · MacBook-Pro</span>
                      <span style="color:var(--emerald);">+1,233 <span style="color:var(--rose);">-116</span></span>
                    </div>
                  </div>

                  <!-- Col 3: Right AI Agent PTY & Composer -->
                  <div class="hw-win-col" style="background:#0e1014;display:flex;flex-direction:column;justify-content:space-between;">
                    <div>
                      <div style="display:flex;justify-content:space-between;align-items:center;padding-bottom:8px;border-bottom:1px solid var(--line);margin-bottom:10px;">
                        <span style="font-size:12px;font-weight:600;"><span style="color:var(--emerald);">●</span> ${this.escapeHtml(previewContent.title)}</span>
                        <span class="coda-mono" style="font-size:11px;color:var(--faint);">Opus 4.6</span>
                      </div>
                      <pre class="coda-mono" style="font-size:12px;line-height:1.65;color:#d1d5db;white-space:pre-wrap;">${this.escapeHtml(previewContent.stream)}</pre>
                    </div>
                    <div style="margin-top:14px;border-radius:8px;background:#16181e;border:1px solid var(--line-strong);padding:8px 10px;">
                      <div class="coda-mono" style="font-size:12px;color:var(--fg);">
                        <span style="color:var(--brand-amber);">❯</span> 部署最新 v0.3.5 到日本主控节点并重启服务
                      </div>
                      <div class="coda-mono" style="margin-top:6px;display:flex;justify-content:space-between;font-size:11px;color:var(--faint);">
                        <span>coda · auto-approve on</span>
                        <span>⌘ Terminal</span>
                      </div>
                    </div>
                  </div>
                </div>

                <div class="hw-win-status">
                  <span>▸ 25:00 Focus &nbsp;·&nbsp; <span style="color:var(--brand-amber);">CODA ${this.escapeHtml(ver)}</span> &nbsp;·&nbsp; <span style="color:var(--emerald);">LAN P2P &lt;1ms / CF WSS Ready</span></span>
                  <span>Metal 120Hz · ChaCha20-Poly1305 + zstd</span>
                </div>
              </div>
            </div>
          </section>

          <section id="features" class="hw-section">
            <div class="hw-container">
              <p class="hw-sec-eyebrow">Why Coda</p>
              <h2 class="hw-h2">你和你的 AI Agent，在所有设备上操控同一套开发现场。</h2>
              <p class="hw-sec-sub">告别断线重连和繁琐的 SSH 端口转发，把重型服务和 Claude Code / Codex 留在常驻主机上，前端随时接入。</p>

              <div class="hw-splits">
                <!-- 01 -->
                <div class="hw-split">
                  <div>
                    <p class="hw-sec-eyebrow"><span style="color:var(--brand-amber);">01</span> · Projects &amp; Herdr</p>
                    <h3 class="hw-h3">合上笔记本、切换手机，远端 Agent 与服务继续运行。</h3>
                    <div class="hw-split-copy">
                      <p>每个终端会话都托管在远端本机的 <code>herdr-core (hc)</code> 守护进程上，断网或切端重新打开即秒级恢复上下文、命令输出和 Git Worktree 增量 Diff。</p>
                      <p>左侧项目树按 <code>SERVICES</code>、<code>AGENTS</code>、<code>ACTIONS</code> 分层组织，macOS 原生桌面端提供与 Highwind 同构的高效率三栏工作流。</p>
                      <div style="display:flex;flex-wrap:wrap;gap:10px;padding-top:4px;">
                        <span><span class="hw-kbd">⌘B</span> 切换项目栏</span>
                        <span><span class="hw-kbd">⌘N</span> 新建终端</span>
                        <span><span class="hw-kbd">Tab</span> 切换审批模式</span>
                      </div>
                    </div>
                  </div>
                  <div class="hw-card-frame">
                    <div class="hw-card-bar"><span>herdr-core · 多厂商 CLI Agent 并行编排</span><span>LIVE</span></div>
                    <div style="padding:18px;display:flex;flex-direction:column;gap:10px;font-family:var(--mono);font-size:13px;">
                      <div style="display:flex;justify-content:space-between;padding:10px 12px;border-radius:8px;background:#0b0c0e;border:1px solid var(--line);">
                        <span><span style="color:var(--coral);">●</span> Claude Code · coda/main</span>
                        <span style="color:var(--emerald);">running · +1233 -116</span>
                      </div>
                      <div style="display:flex;justify-content:space-between;padding:10px 12px;border-radius:8px;background:#0b0c0e;border:1px solid var(--line);">
                        <span><span style="color:var(--emerald);">●</span> Codex · coda-server/feat/wss</span>
                        <span style="color:var(--sky);">done · +842 -94</span>
                      </div>
                      <div style="display:flex;justify-content:space-between;padding:10px 12px;border-radius:8px;background:#0b0c0e;border:1px solid var(--line);">
                        <span><span style="color:var(--sky);">●</span> Gemini CLI · codahub-web</span>
                        <span style="color:var(--muted);">idle · +512 -48</span>
                      </div>
                    </div>
                  </div>
                </div>

                <!-- 02 -->
                <div id="architecture" class="hw-split rev">
                  <div>
                    <p class="hw-sec-eyebrow"><span style="color:var(--coral);">02</span> · Zero-Knowledge E2EE</p>
                    <h3 class="hw-h3">零知识端到端加密，主控与中继只看到乱码信封。</h3>
                    <div class="hw-split-copy">
                      <p>每台主机与客户端在本地独立生成并保管 <code>Ed25519</code> 身份私钥与 <code>X25519</code> 密钥对。所有终端输入输出均先经 <code>zstd</code> 压缩，再用 <code>ChaCha20-Poly1305</code> 加密后发出。</p>
                      <p>部署在日本服务器的控制面板只负责成员隔离与路由调度，哪怕服务器与 CDN 被完全攻破也无法窥探你的代码或对话。</p>
                    </div>
                  </div>
                  <div class="hw-card-frame">
                    <div class="hw-card-bar"><span>coda-protocol · Cryptographic Packet Envelope</span><span>JSON</span></div>
                    <pre class="coda-mono" style="padding:18px;font-size:12px;line-height:1.7;overflow-x:auto;color:#d1d5db;">{
  <span style="color:var(--brand-amber);">"frame_type"</span>: <span style="color:var(--emerald);">"route_packet"</span>,
  <span style="color:var(--brand-amber);">"cipher"</span>: <span style="color:var(--emerald);">"X25519 + HKDF-SHA256 + ChaCha20-Poly1305"</span>,
  <span style="color:var(--brand-amber);">"compression"</span>: <span style="color:var(--emerald);">"zstd_stream"</span>,
  <span style="color:var(--brand-amber);">"lan_direct"</span>: <span style="color:var(--sky);">"ws://192.168.x.x:24444 (&lt;1ms)"</span>,
  <span style="color:var(--brand-amber);">"wan_fallback"</span>: <span style="color:var(--sky);">"wss://codahub.dev/v1/tunnel/ws"</span>
}</pre>
                  </div>
                </div>

                <!-- 03 -->
                <div class="hw-split">
                  <div>
                    <p class="hw-sec-eyebrow"><span style="color:var(--emerald);">03</span> · SmartPath &amp; Device Login</p>
                    <h3 class="hw-h3">同 Wi-Fi 自动直连，一行命令浏览器确认即入网。</h3>
                    <div class="hw-split-copy">
                      <p>在服务器或 Mac 上运行 <code>curl -fsSL ${this.escapeHtml(baseUrl)}/install.sh | sh</code>，终端将弹出一次性设备授权码与 <code>/activate</code> 链接。</p>
                      <p>你只需用浏览器打开点一次同意，这台机器就实时出现在你名下所有 macOS、Web 和 iOS 设备列表里；身处同一局域网时自动绕过公网直连。</p>
                    </div>
                  </div>
                  <div class="hw-card-frame">
                    <div class="hw-card-bar"><span>coda-daemon login --server ${this.escapeHtml(baseUrl)}</span><span>RFC 8628</span></div>
                    <div class="coda-mono" style="padding:18px;font-size:12px;line-height:1.8;color:#d1d5db;">
                      <div>$ coda-daemon login --server ${this.escapeHtml(baseUrl)}</div>
                      <div style="color:var(--muted);">✓ 本地密钥已就绪 (Ed25519 + X25519)</div>
                      <div style="margin-top:8px;padding:10px 12px;border-radius:8px;background:#0b0c0e;border:1px solid var(--line-strong);">
                        请在浏览器打开链接确认设备码：<br/>
                        <span style="color:var(--sky);">${this.escapeHtml(baseUrl)}/activate?code=ABCD-2345</span>
                      </div>
                      <div style="margin-top:8px;color:var(--emerald);">● 授权成功！已注册为常驻主机节点，LAN P2P :24444 已开启</div>
                    </div>
                  </div>
                </div>
              </div>

              <dl class="hw-principles">
                <div>
                  <dt style="font-size:15px;font-weight:600;color:#fff;">原生 Rust + GPUI 桌面端</dt>
                  <dd style="margin-top:6px;font-size:14px;color:var(--muted);line-height:1.6;">Metal GPU 120Hz 原生渲染 macOS 工作台，告别套壳网页卡顿。</dd>
                </div>
                <div>
                  <dt style="font-size:15px;font-weight:600;color:#fff;">主控零公网端口暴露</dt>
                  <dd style="margin-top:6px;font-size:14px;color:var(--muted);line-height:1.6;">容器仅监听 127.0.0.1，通过 Cloudflare Tunnel 自动托管 TLS 与 WSS。</dd>
                </div>
                <div>
                  <dt style="font-size:15px;font-weight:600;color:#fff;">未登录开放全端下载</dt>
                  <dd style="margin-top:6px;font-size:14px;color:var(--muted);line-height:1.6;">官网免登录直接获取 Mac 桌面端、Linux Server/Daemon 与 iOS 测试端。</dd>
                </div>
              </dl>
            </div>
          </section>

          <section id="compare" class="hw-section">
            <div class="hw-container">
              <p class="hw-sec-eyebrow">Compare</p>
              <h2 class="hw-h2">与其他远程开发工具的能力对比。</h2>
              <p class="hw-sec-sub">专为高并发 AI Coding 工作流设计，兼顾零知识隐私、多端互通与本地级交互速度。</p>

              <div class="hw-table-wrap">
                <table class="hw-table">
                  <thead>
                    <tr>
                      <th>方案</th>
                      <th>常驻 Agent 会话</th>
                      <th>端到端零知识加密</th>
                      <th>Wi-Fi 自动 LAN 直连</th>
                      <th>CF Tunnel 零端口</th>
                      <th>浏览器设备码绑机</th>
                      <th>原生 Mac 三栏工作台</th>
                      <th>iOS 单手触控适配</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr class="is-coda">
                      <td>Coda</td>
                      <td><span class="hw-dot" title="支持"></span></td>
                      <td><span class="hw-dot" title="支持"></span></td>
                      <td><span class="hw-dot" title="支持"></span></td>
                      <td><span class="hw-dot" title="支持"></span></td>
                      <td><span class="hw-dot" title="支持"></span></td>
                      <td><span class="hw-dot" title="支持"></span></td>
                      <td><span class="hw-dot" title="支持"></span></td>
                    </tr>
                    <tr>
                      <td>SSH + tmux</td>
                      <td><span class="hw-dot"></span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                    </tr>
                    <tr>
                      <td>VS Code / Web IDE</td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dot"></span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                    </tr>
                    <tr>
                      <td>传统远程桌面</td>
                      <td><span class="hw-dot"></span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dot"></span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                      <td><span class="hw-dash">—</span></td>
                    </tr>
                  </tbody>
                </table>
              </div>
            </div>
          </section>

          <section id="downloads" class="hw-section">
            <div class="hw-container">
              <p class="hw-sec-eyebrow">Downloads · ${this.escapeHtml(ver)}</p>
              <h2 class="hw-h2">免登录下载全端客户端与服务端套件。</h2>
              <p class="hw-sec-sub">在任意设备安装后，使用同一个 Coda 账号登录或设备码授权即可自动互通。</p>

              <div class="dl-grid">
                <div class="dl-card">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                      <strong style="font-size:16px;">💻 macOS 桌面端 (Coda.app)</strong>
                      <span class="badge" style="color:var(--coral);">Hybrid 双向节点</span>
                    </div>
                    <p style="font-size:13px;color:var(--muted);line-height:1.65;">
                      原生 Rust + GPUI 三栏开发工作台，既能遥控你的云端 Linux 服务器，也能让手机随时连回这台 Mac。
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${dl ? `<a class="btn-primary" href="${dl.desktop_macos_dmg}" target="_blank">下载 Mac .dmg 安装包</a>` : ""}
                    ${dl ? `<a class="btn-ghost" href="${dl.desktop_macos_tar}" target="_blank">全套 .tar.gz</a>` : ""}
                  </div>
                </div>

                <div class="dl-card">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                      <strong style="font-size:16px;">🖥 Server / Host Daemon</strong>
                      <span class="badge">Linux &amp; macOS</span>
                    </div>
                    <p style="font-size:13px;color:var(--muted);line-height:1.65;">
                      运行在写代码主机上，桥接本机 <code>herdr-core (hc)</code> 与 Git Worktree，支持 <code>coda-daemon login</code> 浏览器授权。
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${dl ? `<a class="btn-ghost" href="${dl.daemon_linux_x64}" target="_blank">Linux x86_64</a>` : ""}
                    ${dl ? `<a class="btn-ghost" href="${dl.daemon_linux_arm64}" target="_blank">Linux ARM64</a>` : ""}
                    ${dl ? `<a class="btn-ghost" href="${dl.daemon_macos_arm64}" target="_blank">macOS Daemon</a>` : ""}
                  </div>
                </div>

                <div class="dl-card">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                      <strong style="font-size:16px;">📱 iOS 测试版 (TestFlight / PWA)</strong>
                      <span class="badge" style="color:var(--sky);">Beta 先行版</span>
                    </div>
                    <p style="font-size:13px;color:var(--muted);line-height:1.65;">
                      用 iPhone Safari 打开本页面点「分享 → 添加到主屏幕」，免上架直接拥有 E2EE 双排触控键盘与语音输入。
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${
                      dl?.ios_beta_url
                        ? `<a class="btn-primary" href="${this.escapeHtml(dl.ios_beta_url)}" target="_blank">打开 TestFlight 安装 iOS Beta</a>`
                        : `<span class="badge">TestFlight / IPA 构建已就绪（当前推荐直接添加到主屏幕使用）</span>`
                    }
                  </div>
                </div>

                <div class="dl-card">
                  <div>
                    <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                      <strong style="font-size:16px;">☁ 自建主控端部署套件</strong>
                      <span class="badge">Docker + CF Tunnel</span>
                    </div>
                    <p style="font-size:13px;color:var(--muted);line-height:1.65;">
                      1 分钟启动 PostgreSQL 18 + Coordinator + Web 官网 + 免备案裸 TCP 中继，仅绑定 <code>127.0.0.1</code> 供隧道接入。
                    </p>
                  </div>
                  <div style="display:flex;gap:8px;flex-wrap:wrap;">
                    ${dl ? `<a class="btn-ghost" href="${dl.deploy_bundle}" target="_blank">下载 deploy-bundle</a>` : ""}
                    ${dl ? `<a class="btn-ghost" href="${dl.releases_page}" target="_blank">GitHub Releases (${this.escapeHtml(ver)})</a>` : ""}
                  </div>
                </div>
              </div>
            </div>
          </section>

          <section id="portal-auth-section" class="hw-section">
            <div class="hw-container">
              <p class="hw-sec-eyebrow">Console Access</p>
              <h2 class="hw-h2">注册并进入我的私有终端网络。</h2>

              <div class="portal-auth-grid">
                <div class="portal-panel">
                  <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;">
                    <strong style="font-size:16px;">${this.authMode === "login" ? "登录我的 Coda 网络" : "注册新的 Coda 账号"}</strong>
                    <button id="toggle-auth" style="background:none;border:none;color:var(--sky);font-size:12px;cursor:pointer;">
                      ${this.authMode === "login" ? "没有账号？免费注册 →" : "已有账号？直接登录 →"}
                    </button>
                  </div>
                  ${this.errorMsg ? `<div style="color:var(--rose);font-size:12px;margin-bottom:10px;">${this.escapeHtml(this.errorMsg)}</div>` : ""}
                  <form id="auth-form">
                    ${
                      this.authMode === "register"
                        ? `
                      <label style="font-size:12px;color:var(--muted);">邮箱</label>
                      <input name="email" class="inp" required placeholder="dev@example.com" />
                      <label style="font-size:12px;color:var(--muted);">用户名 (小写英文/数字)</label>
                      <input name="handle" class="inp" required placeholder="alice" />
                      <label style="font-size:12px;color:var(--muted);">显示昵称</label>
                      <input name="display_name" class="inp" required placeholder="Alice" />
                      <label style="font-size:12px;color:var(--muted);">密码 (>= 6位)</label>
                      <input type="password" name="password" class="inp" required />
                      ${
                        openReg
                          ? `<div style="font-size:11px;color:var(--emerald);margin-bottom:10px;">当前站点已开放自由注册，无需填写邀请码。</div>`
                          : `<label style="font-size:12px;color:var(--muted);">邀请码 (首位管理员留空)</label><input name="invite_code" class="inp" placeholder="CODA-XXXXXX" />`
                      }
                    `
                        : `
                      <label style="font-size:12px;color:var(--muted);">邮箱或用户名</label>
                      <input name="email_or_handle" class="inp" required placeholder="alice" />
                      <label style="font-size:12px;color:var(--muted);">密码</label>
                      <input type="password" name="password" class="inp" required />
                    `
                    }
                    <button type="submit" class="btn-primary" style="width:100%;margin-top:4px;padding:11px;">
                      ${this.authMode === "login" ? "登录并进入控制台" : "立即注册并开启 E2EE 组网"}
                    </button>
                  </form>
                </div>

                <div class="portal-panel" style="display:flex;flex-direction:column;justify-content:space-between;">
                  <div>
                    <strong style="font-size:16px;">3 步接入任何写代码设备</strong>
                    <ol style="margin-top:12px;padding-left:18px;font-size:13px;color:var(--muted);line-height:1.9;">
                      <li>在官网直接注册并登录（或先在服务器上装好再登录）。</li>
                      <li>在任何服务器/Mac 上执行上方 <code>curl | sh</code> 或运行 <code>coda-daemon login --server ${this.escapeHtml(baseUrl)}</code>。</li>
                      <li>打开终端输出的 <code>/activate?code=XXXX-XXXX</code> 授权链接点击批准，这台机器立刻出现在你的控制面板与 Mac 客户端中。</li>
                    </ol>
                  </div>
                  <div style="margin-top:16px;padding:12px;border-radius:10px;background:#0b0c0e;border:1px solid var(--line);font-size:12px;color:var(--muted);">
                    端到端密钥始终保存在设备本地（<code>Ed25519 + X25519</code> + <code>ChaCha20-Poly1305</code>），主控节点仅做加密包盲转发。
                  </div>
                </div>
              </div>

              <footer class="hw-footer">
                <div style="display:flex;align-items:center;gap:8px;">
                  ${codaLogoSVG(18)}
                  <span>Coda Mesh · codahub.dev</span>
                </div>
                <div style="display:flex;gap:18px;flex-wrap:wrap;">
                  <a href="#downloads" style="text-decoration:none;">下载中心</a>
                  ${dl ? `<a href="${dl.releases_page}" target="_blank" style="text-decoration:none;">GitHub Releases</a>` : ""}
                </div>
              </footer>
            </div>
          </section>
        </div>
      `;

      document.querySelectorAll("[data-preview-tab]").forEach((el) => {
        el.addEventListener("click", () => {
          this.previewTab = (el as HTMLElement).dataset.previewTab as "coda" | "server" | "ios";
          this.render();
        });
      });

      document.getElementById("btn-copy-install")?.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText(installCmd);
          this.copiedToast = "✓ 已复制";
          this.render();
          setTimeout(() => {
            this.copiedToast = "";
            if (!this.user) this.render();
          }, 1800);
        } catch {
          this.copiedToast = "请手动复制上方命令";
          this.render();
        }
      });

      document.getElementById("btn-nav-login")?.addEventListener("click", () => {
        this.authMode = "login";
        this.render();
        document.getElementById("portal-auth-section")?.scrollIntoView({ behavior: "smooth" });
      });

      document.getElementById("btn-nav-register")?.addEventListener("click", () => {
        this.authMode = "register";
        this.render();
        document.getElementById("portal-auth-section")?.scrollIntoView({ behavior: "smooth" });
      });

      document.getElementById("toggle-auth")?.addEventListener("click", () => {
        this.authMode = this.authMode === "login" ? "register" : "login";
        this.errorMsg = "";
        this.render();
      });

      document.getElementById("auth-form")?.addEventListener("submit", async (e) => {
        e.preventDefault();
        const payload = Object.fromEntries(new FormData(e.target as HTMLFormElement).entries());
        try {
          const res =
            this.authMode === "login"
              ? await this.api("/api/v1/auth/login", "POST", payload)
              : await this.api("/api/v1/auth/register", "POST", payload);
          this.token = res.token;
          this.user = res.user;
          this.errorMsg = "";
          localStorage.setItem("coda_token", res.token);
          await this.bootstrapMesh();
          this.render();
        } catch (err: any) {
          this.errorMsg = err.message;
          this.render();
        }
      });
      return;
    }

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
            <div style="padding:14px 16px;display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid var(--line);">
              <div style="display:flex;align-items:center;gap:10px;min-width:0;">
                ${codaLogoSVG(24)}
                <div style="min-width:0;">
                  <div style="font-size:13px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${this.user.email}</div>
                  <div style="font-size:11px;color:var(--emerald);margin-top:2px;">已连接 · ${
                    this.activeTransportMode === "lan_ws" ? "⚡ 局域网直连" : "🌐 智能中继"
                  }${this.rttMs !== null ? ` (${this.rttMs}ms)` : ""}</div>
                </div>
              </div>
              <div style="display:flex;gap:6px;">
                <button id="btn-open-settings" class="circle-btn" title="配对机器与好友邀请码">⚙</button>
                <button id="btn-collapse-sidebar" class="circle-btn" title="收起侧边栏">◧</button>
              </div>
            </div>

            <div style="flex:1;overflow-y:auto;padding:12px;">
              ${
                this.devices.length === 0
                  ? `<div style="padding:12px;font-size:12px;color:var(--muted);line-height:1.65;">
                      当前账号下暂无在线主机。<br/><br/>
                      在任意 Linux / Mac 运行：<br/>
                      <code class="coda-mono" style="color:var(--emerald);word-break:break-all;">${this.escapeHtml(installCmd)}</code><br/><br/>
                      打开终端输出的激活链接即可接入。或点右上角 <b>⚙</b>。
                    </div>`
                  : this.devices
                      .map(
                        (d) => `
                  <div style="margin-bottom:18px;">
                    <div style="display:flex;align-items:center;gap:6px;padding:4px 6px;font-size:12px;color:#a1a1aa;">
                      <span style="width:7px;height:7px;border-radius:99px;background:${d.is_online ? "var(--emerald)" : "#52525b"};"></span>
                      <span style="color:#e4e4e7;font-weight:500;">coda</span>
                      <span>· ${d.name}</span>
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
                                  ${t.status?.state || "终端运行中"}
                                </div>
                              </div>
                            </div>
                            <div class="coda-mono" style="font-size:11px;color:var(--emerald);">+184 <span style="color:var(--rose);">-19</span></div>
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
                              <div style="font-size:11px;color:${d.is_online ? "var(--emerald)" : "var(--muted)"};">
                                ${d.is_online ? "点击加载 Herdr 终端" : "主机离线"}
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
          <div style="display:flex;align-items:flex-start;justify-content:space-between;padding-bottom:8px;border-bottom:1px solid var(--line);">
            <div style="min-width:0;">
              <div style="font-size:15px;font-weight:700;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                ${activeTerm?.status?.title || activeTerm?.name || "Coda 交互式 AI 终端会话"}
              </div>
              <div class="coda-mono" style="font-size:12px;color:var(--muted);margin-top:2px;">
                ${activeTerm?.cwd || "~/personal/github/coda"} · main · ${this.selectedDaemon?.name || "MacBook-Pro"}
              </div>
            </div>
            <div style="display:flex;align-items:center;gap:8px;">
              ${(this.terminals.length > 0 ? this.terminals : [{ address: "t1" } as any])
                .map(
                  (t, i) => `
                <button data-term="${t.address}" style="margin-left:${i === 0 ? "0" : "-8px"};width:34px;height:34px;border-radius:999px;background:#14161b;border:${
                    t.address === this.activeTerminalAddr ? "2px solid #f4f4f5" : "1px solid #3f3f46"
                  };display:flex;align-items:center;justify-content:center;cursor:pointer;z-index:${10 - i};">
                  ${codaNodeDotSVG(i % 2 === 0 ? "#e06c50" : "#38bdf8")}
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
              : `  - 已连接 PostgreSQL 18 多租户控制面与端到端加密隧道 (X25519 + ChaCha20-Poly1305)
  - 本机 Herdr Core (hc 0.1.20) 在线，随时可通过右侧「+」拉起 Claude Code / Codex / Gemini CLI

* Ready · ${this.rttMs !== null ? `RTT ${this.rttMs}ms` : "CF Tunnel WSS :443 & Direct LAN P2P Ready"}`
          }

<div style="margin-top:14px;border-top:1px solid var(--line);padding-top:6px;border-bottom:1px solid var(--line);padding-bottom:6px;">&gt; ${this.formatCursorDraft(
            this.draftInput
          )}</div>
<div style="font-size:12px;color:#71717a;margin-top:4px;">Opus 4.6 | 5h <span style="color:var(--rose);">100%</span> | 7d 总 <span style="color:#d4d4d8;">77%</span> | <span style="color:#f59e0b;">⚠ E2EE zstd</span></div>
<div style="font-size:12px;color:var(--coral);">▶▶ bypass permissions on (shift+tab to cycle) <span style="color:#71717a;">· ← ${
            this.terminals.length || 1
          } agents · PR</span></div></div>

          ${
            this.showKeyboardInputBar
              ? `
            <form id="kb-form" style="display:flex;gap:8px;margin-bottom:8px;">
              <input id="kb-input" class="inp" style="margin:0;flex:1;" value="${this.escapeHtml(
                this.draftInput
              )}" placeholder="输入命令或中文 Prompt 发送至当前 Herdr 终端..." autofocus />
              <button type="submit" style="background:#f4f4f5;color:#000;border:none;border-radius:10px;padding:0 14px;font-size:12px;font-weight:600;cursor:pointer;">确定</button>
            </form>
          `
              : ""
          }

          ${
            renderTouchKeypad
              ? `
          <!-- Mobile 2-Row Tactile Keypad + Tall White Enter -->
          <div class="keypad-grid">
            <div class="keypad-left">
              <div class="keypad-row">
                <button data-k="esc" class="k-btn" style="grid-column:span 2;">esc</button>
                <button data-k="shift+tab" class="k-btn" style="grid-column:span 2;">tab</button>
                <button id="btn-kb-toggle" class="k-btn" style="grid-column:span 3;display:flex;justify-content:space-around;padding:0 10px;">
                  <span>⌨</span>
                  <span style="width:1px;height:14px;background:#3f3f46;"></span>
                  <span>🎙</span>
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
              <span class="coda-mono" style="color:var(--brand-amber);font-weight:700;">❯</span>
              <input id="desktop-prompt-input" class="desktop-composer-inp" value="${this.escapeHtml(
                this.draftInput
              )}" placeholder="向当前 Herdr / Agent 终端发送指令或 Prompt... (Enter 发送)" autocomplete="off" />
              <button type="button" data-k="ctrl+c" class="btn-ghost" style="padding:5px 9px;font-size:11px;color:var(--coral);">^C</button>
              <button type="button" id="btn-toggle-touch-pad" class="btn-ghost" style="padding:5px 9px;font-size:11px;" title="切换触控虚拟键盘">⌨</button>
              <button type="submit" class="btn-primary" style="padding:6px 12px;font-size:12px;">↑ 发送</button>
            </div>
          </form>`
          }
        </main>

        <!-- RIGHT VERTICAL THUMB RAIL -->
        <aside class="right-rail">
          <div style="width:34px;height:58px;border-radius:999px;border:1px solid #3f1d24;background:#0c0a0b;display:flex;align-items:flex-end;justify-content:center;padding-bottom:8px;">
            <span style="width:11px;height:11px;border-radius:99px;background:#ef4444;box-shadow:0 0 8px #ef4444;"></span>
          </div>

          <div style="text-align:center;" class="coda-mono">
            <div style="font-size:12px;font-weight:700;">${nowTime}</div>
            <div style="margin-top:6px;width:42px;height:42px;border-radius:999px;border:2px solid #3f3f46;border-top-color:#f4f4f5;display:flex;flex-direction:column;align-items:center;justify-content:center;font-size:11px;">
              <strong>68</strong>
              ${
                this.activeTransportMode === "lan_ws"
                  ? `<span style="color:var(--emerald);">LAN</span>`
                  : `<span style="color:var(--muted);">WSS</span>`
              }
            </div>
          </div>

          <div style="display:flex;flex-direction:column;gap:10px;align-items:center;">
            <button id="rail-sidebar-btn" class="circle-btn" title="展开/收起左侧分支栏">${this.showSidebar ? "‹" : "›"}</button>
            <div class="rail-capsule">
              <button id="rail-spawn-btn" title="新建 Herdr 终端">+</button>
              <span style="width:16px;height:1px;background:#27272a;"></span>
              <button id="rail-more-btn" title="系统设置与配对">•••</button>
            </div>
          </div>

          <div class="rail-capsule">
            <input type="file" id="rail-file-input" style="display:none;" />
            <button id="rail-refresh-btn" title="通过 hc put 上传图片/附件至远端终端">📎</button>
            <span style="width:16px;height:1px;background:#27272a;"></span>
            <button id="rail-paste-btn" title="粘贴剪贴板">📋</button>
          </div>
        </aside>
      </div>

      ${
        this.pendingActivationCode || this.showSettingsModal
          ? ""
          : ""
      }
      ${
        this.pendingActivationCode
          ? `
        <div class="modal-mask">
          <div class="modal-card">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
              <strong style="font-size:15px;">🔑 确认授权设备接入账号 @${this.escapeHtml(this.user.handle)}</strong>
              <button id="btn-dismiss-activate" class="circle-btn" style="width:28px;height:28px;">✕</button>
            </div>
            ${this.errorMsg ? `<div style="color:var(--rose);font-size:12px;margin-bottom:8px;">${this.escapeHtml(this.errorMsg)}</div>` : ""}
            <div class="coda-mono" style="font-size:20px;font-weight:700;color:var(--sky);letter-spacing:2px;margin-bottom:10px;">
              ${this.escapeHtml(this.pendingActivationCode)}
            </div>
            ${
              this.pendingDeviceInfo
                ? `<div style="background:#070709;border:1px solid #27272a;border-radius:12px;padding:12px;font-size:12px;line-height:1.7;margin-bottom:14px;">
                    设备名称：<b>${this.escapeHtml(this.pendingDeviceInfo.device_name)}</b><br/>
                    平台 / 角色：${this.escapeHtml(this.pendingDeviceInfo.platform)} / ${this.escapeHtml(this.pendingDeviceInfo.role)}<br/>
                    当前状态：${this.escapeHtml(this.pendingDeviceInfo.status)}
                  </div>`
                : `<div style="font-size:12px;color:var(--muted);margin-bottom:12px;">点击下方确认，将把该授权码对应的设备绑定到当前账号。</div>`
            }
            <button id="btn-approve-device" class="btn-primary" style="width:100%;padding:11px;">
              ✓ 批准此设备接入我的私有网络
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
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:14px;">
              <strong style="font-size:15px;">接入新设备、授权码与语音模型配置</strong>
              <button id="btn-close-modal" class="circle-btn" style="width:28px;height:28px;">✕</button>
            </div>
            ${
              this.activationSuccessMsg
                ? `<div style="background:rgba(34,197,94,0.12);border:1px solid rgba(34,197,94,0.35);color:var(--emerald);padding:9px 11px;border-radius:10px;font-size:12px;margin-bottom:10px;">${this.escapeHtml(this.activationSuccessMsg)}</div>`
                : ""
            }
            <div class="coda-mono" style="background:#070709;border:1px solid #27272a;padding:10px;border-radius:10px;font-size:11px;margin-bottom:10px;word-break:break-all;">
              在任意 Linux / Mac 终端执行后打开返回的激活链接：<br/>
              <span style="color:var(--emerald);">${this.escapeHtml(installCmd)}</span>
            </div>
            <form id="quick-approve-form" style="display:flex;gap:8px;margin-bottom:12px;">
              <input name="user_code" class="inp" style="margin:0;flex:1;" placeholder="输入终端设备码 如 ABCD-2345" />
              <button type="submit" class="k-btn" style="padding:8px 12px;font-size:12px;white-space:nowrap;">授权接入</button>
            </form>
            <div style="display:flex;gap:8px;flex-wrap:wrap;margin-bottom:12px;">
              <button id="btn-gen-pair" class="k-btn" style="padding:8px 12px;font-size:12px;">+ 生成 6 位机器配对码</button>
              ${
                this.user.is_admin
                  ? `<button id="btn-gen-invite" class="k-btn" style="padding:8px 12px;font-size:12px;">+ 签发好友邀请码</button>`
                  : ""
              }
              <button id="btn-logout" class="k-btn" style="padding:8px 12px;font-size:12px;color:var(--rose);">退出登录</button>
            </div>
            ${
              this.pairCommand
                ? `<div class="coda-mono" style="background:#070709;border:1px dashed var(--emerald);padding:10px;border-radius:10px;font-size:12px;margin-bottom:10px;word-break:break-all;">
                    在 Mac / Linux 一键安装并配对：<br/>${this.pairCommand}
                  </div>`
                : ""
            }
            ${
              this.latestInviteCode
                ? `<div class="coda-mono" style="background:#070709;border:1px dashed var(--sky);padding:10px;border-radius:10px;font-size:12px;margin-bottom:10px;">
                    好友注册邀请码：<b>${this.latestInviteCode}</b>
                  </div>`
                : ""
            }
            <form id="voice-cfg-form" style="border-top:1px solid #1f1f24;padding-top:12px;margin-top:8px;">
              <div style="font-size:13px;font-weight:600;margin-bottom:8px;">🎙 语音模型 provider + baseurl + key 配置 (持久化至 Herdr doc.set)</div>
              <label style="font-size:11px;color:var(--muted);">Provider</label>
              <input name="provider" class="inp" value="${this.escapeHtml(this.voiceConfig.provider)}" placeholder="openai / groq / deepseek" />
              <label style="font-size:11px;color:var(--muted);">Base URL</label>
              <input name="base_url" class="inp" value="${this.escapeHtml(this.voiceConfig.base_url)}" placeholder="https://api.openai.com/v1" />
              <label style="font-size:11px;color:var(--muted);">API Key</label>
              <input type="password" name="api_key" class="inp" value="${this.escapeHtml(this.voiceConfig.api_key)}" placeholder="sk-..." />
              <label style="font-size:11px;color:var(--muted);">Model</label>
              <input name="model" class="inp" value="${this.escapeHtml(this.voiceConfig.model)}" placeholder="gpt-4o-mini-transcribe" />
              <button type="submit" style="width:100%;background:#f4f4f5;color:#000;border:none;border-radius:10px;padding:9px;font-size:12px;font-weight:600;cursor:pointer;">
                保存语音模型配置
              </button>
            </form>
          </div>
        </div>
      `
          : ""
      }
    `;

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
      window.history.replaceState({}, "", "/");
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
        this.activationSuccessMsg = `✓ 已授权设备 ${dev.name} (${dev.platform})`;
        window.history.replaceState({}, "", "/");
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
        this.activationSuccessMsg = `✓ 已授权设备 ${dev.name} (${dev.platform})`;
        await this.refreshDevices();
        this.render();
      } catch (err: any) {
        this.activationSuccessMsg = `✗ ${err.message}`;
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
    document.getElementById("btn-logout")?.addEventListener("click", () => {
      localStorage.removeItem("coda_token");
      this.token = null;
      this.user = null;
      this.showSettingsModal = false;
      this.directWs?.close();
      this.ws?.close();
      this.render();
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
