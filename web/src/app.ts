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

function mascotSVG(color = "#e06c50") {
  return `<svg width="18" height="18" viewBox="0 0 16 16" fill="none">
    <rect x="3" y="4" width="10" height="6" fill="${color}"/>
    <rect x="2" y="6" width="1" height="3" fill="${color}"/>
    <rect x="13" y="6" width="1" height="3" fill="${color}"/>
    <rect x="4" y="5" width="2" height="2" fill="#09090b"/>
    <rect x="10" y="5" width="2" height="2" fill="#09090b"/>
    <rect x="3" y="10" width="2" height="3" fill="${color}"/>
    <rect x="6" y="10" width="1.5" height="2.5" fill="${color}"/>
    <rect x="8.5" y="10" width="1.5" height="2.5" fill="${color}"/>
    <rect x="11" y="10" width="2" height="3" fill="${color}"/>
  </svg>`;
}

class CodaTerminalWebApp {
  private token: string | null = localStorage.getItem("coda_token");
  private authMode: "login" | "register" = "login";
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
    this.devices = all.filter((d) => d.role === "daemon");
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
    if (!this.user) {
      root.innerHTML = `
        <div class="modal-mask">
          <div class="modal-card">
            <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:16px;">
              <div style="display:flex;align-items:center;gap:8px;">
                ${mascotSVG("#e06c50")}
                <strong class="coda-mono" style="font-size:15px;">CODA TERMINAL MESH</strong>
              </div>
              <button id="toggle-auth" style="background:none;border:none;color:var(--sky);font-size:12px;cursor:pointer;">
                ${this.authMode === "login" ? "注册新账号 →" : "返回登录 →"}
              </button>
            </div>
            ${this.errorMsg ? `<div style="color:var(--rose);font-size:12px;margin-bottom:10px;">${this.errorMsg}</div>` : ""}
            <form id="auth-form">
              ${
                this.authMode === "register"
                  ? `
                <label style="font-size:12px;color:var(--muted);">邮箱</label>
                <input name="email" class="inp" required placeholder="vampire@coda.dev" />
                <label style="font-size:12px;color:var(--muted);">用户名</label>
                <input name="handle" class="inp" required placeholder="vampire" />
                <label style="font-size:12px;color:var(--muted);">昵称</label>
                <input name="display_name" class="inp" required placeholder="Vampire" />
                <label style="font-size:12px;color:var(--muted);">密码 (>= 6位)</label>
                <input type="password" name="password" class="inp" required />
                <label style="font-size:12px;color:var(--muted);">好友邀请码 (首位管理员可留空)</label>
                <input name="invite_code" class="inp" placeholder="CODA-XXXXXX" />
              `
                  : `
                <label style="font-size:12px;color:var(--muted);">邮箱或用户名</label>
                <input name="email_or_handle" class="inp" required placeholder="vampire" />
                <label style="font-size:12px;color:var(--muted);">密码</label>
                <input type="password" name="password" class="inp" required />
              `
              }
              <button type="submit" style="width:100%;background:#f4f4f5;color:#000;border:none;border-radius:12px;padding:10px;font-weight:600;cursor:pointer;margin-top:4px;">
                ${this.authMode === "login" ? "连接终端算力网" : "注册并开启 E2EE 组网"}
              </button>
            </form>
          </div>
        </div>
      `;

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

    root.innerHTML = `
      <div class="shell-root">
        ${
          this.showSidebar
            ? `
          <!-- LEFT SIDEBAR: Account + Machine/Repo/Branch List -->
          <aside class="left-sidebar">
            <div style="padding:14px 16px;display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid #141417;">
              <div>
                <div style="font-size:14px;font-weight:500;">${this.user.email}</div>
                <div style="font-size:12px;color:var(--emerald);margin-top:2px;">已连接 · ${
                  this.activeTransportMode === "lan_ws" ? "⚡ 局域网直连" : "🌐 智能中继"
                }${this.rttMs !== null ? ` (${this.rttMs}ms)` : ""}</div>
              </div>
              <div style="display:flex;gap:8px;">
                <button id="btn-open-settings" class="circle-btn" title="配对机器与好友邀请码">⚙</button>
                <button id="btn-collapse-sidebar" class="circle-btn" title="收起侧边栏">◧</button>
              </div>
            </div>

            <div style="flex:1;overflow-y:auto;padding:12px;">
              ${
                this.devices.length === 0
                  ? `<div style="padding:12px;font-size:12px;color:var(--muted);line-height:1.6;">
                      当前账号下暂无在线主机。<br/>点击右上角 <b>⚙</b> 生成 6 位配对码接入你的 Mac 或 Linux 服务器。
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
                              ${mascotSVG("#e06c50")}
                              <div style="min-width:0;">
                                <div class="coda-mono" style="font-size:13px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                                  ${t.name || t.address}
                                </div>
                                <div style="font-size:12px;color:var(--emerald);margin-top:2px;">
                                  ${t.status?.state || "终端运行中"}
                                </div>
                              </div>
                            </div>
                            <div class="coda-mono" style="font-size:12px;color:var(--emerald);">+184 <span style="color:var(--rose);">-19</span></div>
                          </div>
                        `
                            )
                            .join("")
                        : `
                        <div class="branch-item ${this.selectedDaemon?.id === d.id ? "active" : ""}" data-dev="${d.id}">
                          <div style="display:flex;align-items:center;gap:10px;">
                            ${mascotSVG(d.is_online ? "#e06c50" : "#52525b")}
                            <div>
                              <div class="coda-mono" style="font-size:13px;">main</div>
                              <div style="font-size:12px;color:${d.is_online ? "var(--emerald)" : "var(--muted)"};">
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
          <div style="display:flex;align-items:flex-start;justify-content:space-between;padding-bottom:8px;border-bottom:1px solid #141417;">
            <div style="min-width:0;">
              <div style="font-size:15px;font-weight:700;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                ${activeTerm?.status?.title || activeTerm?.name || "Coda 交互式 AI 终端会话"}
              </div>
              <div class="coda-mono" style="font-size:12px;color:var(--muted);margin-top:2px;">
                ${activeTerm?.cwd || "~/personal/github/coda"} · main · ${this.selectedDaemon?.name || "Home"}
              </div>
            </div>
            <div style="display:flex;align-items:center;">
              ${(this.terminals.length > 0 ? this.terminals : [{ address: "t1" } as any])
                .map(
                  (t, i) => `
                <button data-term="${t.address}" style="margin-left:${i === 0 ? "0" : "-8px"};width:36px;height:36px;border-radius:999px;background:#141417;border:${
                    t.address === this.activeTerminalAddr ? "2px solid #f4f4f5" : "1px solid #3f3f46"
                  };display:flex;align-items:center;justify-content:center;cursor:pointer;z-index:${10 - i};">
                  ${mascotSVG(i % 2 === 0 ? "#e06c50" : "#38bdf8")}
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
  - 本机 Herdr Core (hc 0.1.20) 在线，随时可通过右侧「+」拉起 Claude Code / Grok Build / Codex

* Ready · ${this.rttMs !== null ? `RTT ${this.rttMs}ms` : "Zero-SNI Raw TCP :24443 & WSS Dual-Lane Ready"}`
          }

<div style="margin-top:14px;border-top:1px solid #27272a;padding-top:6px;border-bottom:1px solid #27272a;padding-bottom:6px;">&gt; ${this.formatCursorDraft(
            this.draftInput
          )}</div>
<div style="font-size:12px;color:#71717a;margin-top:4px;">Opus 5.5 | 5h <span style="color:var(--rose);">100%</span> | 7d 总 <span style="color:#d4d4d8;">77%</span> | <span style="color:#f59e0b;">⚠ E2EE zstd</span></div>
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

          <!-- Bottom 2-Row Tactile Keypad + Tall White Enter -->
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
          </div>
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
                  : `<span style="color:var(--muted);">5G</span>`
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
        this.showSettingsModal
          ? `
        <div class="modal-mask" id="modal-bg">
          <div class="modal-card">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:14px;">
              <strong style="font-size:15px;">机器配对、好友邀请与语音模型配置</strong>
              <button id="btn-close-modal" class="circle-btn" style="width:28px;height:28px;">✕</button>
            </div>
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
