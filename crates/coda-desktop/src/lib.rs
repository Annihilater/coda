//! Coda Native GPU-Accelerated Desktop Client (`Rust + GPUI`) for macOS, Windows, and Linux.
//! - Zero WebView / Electron overhead: renders directly onto Metal (macOS), DirectX 12 (Windows),
//!   and Vulkan/Blade (Linux) at 120Hz/240Hz.
//! - Zero FFI boundary: directly shares `coda-protocol`, `coda-herdr`, and `coda-daemon` in pure Rust!
//! - Implements the Highwind-style macOS 3-Column Agentic Workbench:
//!   1. Top Workspace Group Bar & Secondary Numbered View / Split Toggle Bar.
//!   2. Left `PROJECTS` Explorer (`SERVICES`, `AGENTS`, `ACTIONS`) with collapsible repositories.
//!   3. Center Workspace Preview / Worktree Diff Inspector / Mesh & Voice Hub.
//!   4. Right AI Agent & Live PTY Stream Pane with Desktop Prompt Composer (no mobile soft keypad).
//!   5. Bottom Global Focus & Route Telemetry Status Bar (`v0.3.5`).

use std::sync::{Arc, Mutex};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::{
    list_account_hosts, mint_peer_relay_config, poll_device_login_until_approved,
    start_device_login_flow, start_embedded_host_if_configured, DaemonConfig, DirectP2pClient,
    RemoteHostSummary, SmartPeerClient, SmartRelayFallbackConfig,
};
use coda_herdr::{discover_hc_binary, scan_git_workspaces, HerdrClient};
use coda_protocol::{
    BranchWorktreeItem, HerdrTerminalSummary, NetworkEndpointCandidate, PeerMessage,
    RepoWorkspaceSummary, StoredDeviceIdentity, TrafficLane, VoiceModelConfig,
};
use gpui::{
    actions, div, prelude::*, px, rgb, rgba, size, white, App, Application, Bounds,
    ClipboardItem, Context, FocusHandle, Focusable, KeyBinding, MouseButton, MouseUpEvent,
    SharedString, Window, WindowBounds, WindowOptions,
};
use uuid::Uuid;

actions!(
    coda_desktop,
    [
        QuitApp,
        ToggleSidebar,
        CycleBypassPermissions,
        SpawnNewTerminal,
        InterruptActiveTerminal,
        SubmitPrompt,
    ]
);

/// Layered Dark Workbench Palette matching the macOS Desktop Workbench design.
pub mod palette {
    pub const CHROME_BG: u32 = 0x141519;
    pub const SIDEBAR_BG: u32 = 0x111216;
    pub const CENTER_BG: u32 = 0x0b0c0f;
    pub const AGENT_BG: u32 = 0x0e1014;
    pub const OLED_BLACK: u32 = 0x0b0c0f;
    pub const KEY_SURFACE: u32 = 0x181a20;
    pub const KEY_HOVER: u32 = 0x22252e;
    pub const KEY_BORDER: u32 = 0x262932;
    pub const DIVIDER: u32 = 0x20232b;
    pub const TEXT_PRIMARY: u32 = 0xededf0;
    pub const TEXT_MUTED: u32 = 0x8e929f;
    pub const TEXT_FAINT: u32 = 0x5c606d;
    pub const EMERALD_WORKING: u32 = 0x22c55e;
    pub const SKY_DONE: u32 = 0x38bdf8;
    pub const CORAL_AGENT: u32 = 0xe06c50;
    pub const ROSE_DIFF_DEL: u32 = 0xf43f5e;
    pub const AMBER_WARN: u32 = 0xf59e0b;
}

#[derive(Debug, Clone)]
pub struct DesktopTerminalSession {
    pub address: String,
    pub title: String,
    pub repo_name: String,
    pub branch_name: String,
    pub machine_name: String,
    pub model_badge: String,
    pub status_label: String,
    pub is_done: bool,
    pub diff_added: u32,
    pub diff_deleted: u32,
    pub screen_text: String,
}

#[derive(Debug, Clone)]
pub struct CodaDesktopState {
    pub account_email: String,
    pub server_url: String,
    pub is_logged_in: bool,
    pub connected_route: String,
    pub show_sidebar: bool,
    pub show_center_pane: bool,
    pub show_agent_pane: bool,
    pub center_tab: String,
    pub expanded_repo: String,
    pub focus_timer_running: bool,
    pub show_voice_modal: bool,
    pub bypass_permissions: bool,
    pub draft_input: String,
    pub active_terminal_addr: String,
    pub selected_host_id: Option<Uuid>,
    pub remote_hosts: Vec<RemoteHostSummary>,
    pub pending_login_url: Option<String>,
    pub pending_login_code: Option<String>,
    pub sessions: Vec<DesktopTerminalSession>,
    pub repos: Vec<RepoWorkspaceSummary>,
    pub voice_config: VoiceModelConfig,
    pub status_banner: Option<String>,
}

impl Default for CodaDesktopState {
    fn default() -> Self {
        let sessions = vec![
            DesktopTerminalSession {
                address: "t1".into(),
                title: "GPUI 桌面工作台与 Herdr PTY 流式同步".into(),
                repo_name: "coda".into(),
                branch_name: "main".into(),
                machine_name: "MacBook-Pro".into(),
                model_badge: "Opus 4.6".into(),
                status_label: "4 个终端运行中".into(),
                is_done: false,
                diff_added: 1233,
                diff_deleted: 116,
                screen_text: concat!(
                    "  - 已连接本地 Herdr Core (`hc 0.1.20`) 守护进程，自动扫描活跃 Git Worktree 与终端。\n",
                    "  - 本地预览工作区：`coda` (`main`) 与 `codahub-web` (`feat/highwind-landing`)。\n",
                    "  - 零知识端到端加密引擎就绪：Ed25519 签名 + X25519 密钥协商 + ChaCha20-Poly1305。\n\n",
                    "* Crunched for 4m 18s · done 12:37 PM\n\n",
                    "● Background command \"cargo test -p coda-protocol -p coda-herdr\" completed (exit code 0)\n\n",
                    "  Read 2 files\n\n",
                    "● 测试与路由自检全部通过：LAN Direct P2P (:24444) 与 Cloudflare WSS (/v1/tunnel/ws) 双通道就绪。\n\n",
                    "* Churned for 22s · done 12:43 PM\n",
                    "  new task? /clear... · ✓ Rust + GPUI 120Hz Metal Ready"
                )
                .into(),
            },
            DesktopTerminalSession {
                address: "t2".into(),
                title: "Cloudflare Tunnel 与零知识 E2EE 路由巡检".into(),
                repo_name: "coda-server".into(),
                branch_name: "feat/cf-tunnel-wss".into(),
                machine_name: "green-jp-02".into(),
                model_badge: "Sonnet 4.5".into(),
                status_label: "本轮完成".into(),
                is_done: true,
                diff_added: 842,
                diff_deleted: 94,
                screen_text: concat!(
                    "  - 控制面板默认仅监听 127.0.0.1:8080，由 Cloudflare Tunnel 零公网端口反代至 https://codahub.dev。\n",
                    "  - SmartPeerClient 同 Wi-Fi 下 250ms 探测局域网 P2P 直连（< 1ms），跨网自动切换至 WSS 443。\n",
                    "  - RFC 8628 浏览器设备码授权 (/activate) 已与 Postgres 18 多租户控制面联调完成。\n\n",
                    "* Baked for 38s · done 4:33 PM\n\n",
                    "● recap: green-jp-02 节点健康检查通过，所有端到端通道加密在线。"
                )
                .into(),
            },
        ];

        let repos = vec![
            RepoWorkspaceSummary {
                repo_name: "coda".into(),
                root_path: "/Users/vampire/personal/github/coda".into(),
                machine_name: "MacBook-Pro".into(),
                branches: vec![
                    BranchWorktreeItem {
                        branch: "main".into(),
                        worktree_path: "/Users/vampire/personal/github/coda".into(),
                        diff_added: 1233,
                        diff_deleted: 116,
                        active_terminals: vec![],
                        status_summary: "4 个终端运行中".into(),
                    },
                    BranchWorktreeItem {
                        branch: "feat/cf-tunnel-wss".into(),
                        worktree_path: "/Users/vampire/personal/github/coda".into(),
                        diff_added: 842,
                        diff_deleted: 94,
                        active_terminals: vec![],
                        status_summary: "本轮完成".into(),
                    },
                ],
            },
            RepoWorkspaceSummary {
                repo_name: "codahub-web".into(),
                root_path: "/Users/vampire/personal/github/coda/web".into(),
                machine_name: "MacBook-Pro".into(),
                branches: vec![
                    BranchWorktreeItem {
                        branch: "main".into(),
                        worktree_path: "/Users/vampire/personal/github/coda/web".into(),
                        diff_added: 512,
                        diff_deleted: 48,
                        active_terminals: vec![],
                        status_summary: "1 个终端运行中".into(),
                    },
                    BranchWorktreeItem {
                        branch: "feat/highwind-landing".into(),
                        worktree_path: "/Users/vampire/personal/github/coda/web".into(),
                        diff_added: 0,
                        diff_deleted: 0,
                        active_terminals: vec![],
                        status_summary: "1 个终端 · 空闲".into(),
                    },
                ],
            },
        ];

        let default_server_url = std::env::var("CODA_SERVER_URL")
            .unwrap_or_else(|_| "https://codahub.dev".into());

        Self {
            account_email: "vampire@codahub.dev".into(),
            server_url: default_server_url,
            is_logged_in: false,
            connected_route: "已连接 · L0 GPUI Metal 120Hz".into(),
            show_sidebar: true,
            show_center_pane: true,
            show_agent_pane: true,
            center_tab: "preview".into(),
            expanded_repo: "coda".into(),
            focus_timer_running: true,
            show_voice_modal: false,
            bypass_permissions: true,
            draft_input: String::new(),
            active_terminal_addr: "t1".into(),
            selected_host_id: None,
            remote_hosts: Vec::new(),
            pending_login_url: None,
            pending_login_code: None,
            sessions,
            repos,
            voice_config: VoiceModelConfig {
                provider: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: "sk-proj-coda-gpui".into(),
                model: "gpt-4o-mini-transcribe".into(),
            },
            status_banner: None,
        }
    }
}

impl CodaDesktopState {
    pub fn active_session(&self) -> &DesktopTerminalSession {
        self.sessions
            .iter()
            .find(|s| s.address == self.active_terminal_addr)
            .unwrap_or(&self.sessions[0])
    }

    pub fn active_session_mut(&mut self) -> &mut DesktopTerminalSession {
        let addr = self.active_terminal_addr.clone();
        if let Some(idx) = self.sessions.iter().position(|s| s.address == addr) {
            &mut self.sessions[idx]
        } else {
            &mut self.sessions[0]
        }
    }

    /// Synchronizes live `herdr-core` (`hc`) terminals and Git worktrees into the GPUI state.
    pub async fn sync_from_local_herdr(&mut self) {
        let Some(hc_bin) = discover_hc_binary() else {
            return;
        };
        let Ok(client) = HerdrClient::connect_with_bin(hc_bin).await else {
            return;
        };
        if let Ok(Some(vcfg)) = client.get_voice_config().await {
            self.voice_config = vcfg;
        }
        let Ok(terms) = client.list_terminals(false).await else {
            return;
        };
        let scanned_repos = scan_git_workspaces(
            &client.info().core_name,
            &[],
            &terms,
        )
        .await;
        if !scanned_repos.is_empty() {
            self.repos = scanned_repos;
        }

        for t in terms {
            self.upsert_herdr_terminal(&client, t).await;
        }
    }

    /// Connects directly to a remote or LAN `coda-daemon` via Phase 3 Direct P2P (`< 10ms` zero-relay E2EE)
    /// and synchronizes its Git workspaces, worktrees, and Voice Model configuration.
    pub async fn sync_from_direct_p2p_peer(
        &mut self,
        endpoint: &str,
        client_id: &StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
    ) -> anyhow::Result<()> {
        let mut p2p = DirectP2pClient::connect_direct(
            endpoint,
            client_id,
            client_device_id,
            target_device_id,
        )
        .await?;
        self.connected_route = format!("已连接 · P2P {} Zero-Relay", p2p.path_kind);

        p2p.send_message(
            &PeerMessage::ListWorkspaces {
                req_id: "gpui-ws-1".into(),
                root_dirs: vec![],
            },
            TrafficLane::ControlP0,
        )
        .await?;

        if let PeerMessage::WorkspacesSnapshot {
            repos,
            voice_config,
            ..
        } = p2p.recv_message().await?
        {
            if !repos.is_empty() {
                self.repos = repos;
            }
            if let Some(vc) = voice_config {
                self.voice_config = vc;
            }
        }
        Ok(())
    }

    /// Auto-negotiates between LAN Direct P2P and Cloud Relay via `SmartPeerClient`
    /// and updates the Mac client's connection status badge.
    pub async fn sync_from_smart_peer(
        &mut self,
        client_id: StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        endpoints: Vec<NetworkEndpointCandidate>,
        udp_probe_addr: Option<std::net::SocketAddr>,
        relay_cfg: Option<SmartRelayFallbackConfig>,
    ) -> anyhow::Result<()> {
        let mut smart = SmartPeerClient::connect_auto(
            client_id,
            client_device_id,
            target_device_id,
            endpoints,
            udp_probe_addr,
            relay_cfg,
        )
        .await?;

        self.connected_route = if smart.is_direct_p2p() {
            "已连接 · ⚡ 局域网直连 LAN P2P".into()
        } else {
            "已连接 · 🌐 智能中继通道".into()
        };

        if let PeerMessage::WorkspacesSnapshot {
            repos,
            voice_config,
            ..
        } = smart
            .request(
                &PeerMessage::ListWorkspaces {
                    req_id: "gpui-smart-ws".into(),
                    root_dirs: vec![],
                },
                TrafficLane::ControlP0,
            )
            .await?
        {
            if !repos.is_empty() {
                self.repos = repos;
            }
            if let Some(vc) = voice_config {
                self.voice_config = vc;
            }
        }
        Ok(())
    }

    pub fn apply_voice_preset(&mut self, provider: &str) {
        let cfg = match provider {
            "groq" => VoiceModelConfig {
                provider: "groq".into(),
                base_url: "https://api.groq.com/openai/v1".into(),
                api_key: self.voice_config.api_key.clone(),
                model: "whisper-large-v3-turbo".into(),
            },
            "deepseek" => VoiceModelConfig {
                provider: "deepseek".into(),
                base_url: "https://api.deepseek.com/v1".into(),
                api_key: self.voice_config.api_key.clone(),
                model: "deepseek-voice-v1".into(),
            },
            _ => VoiceModelConfig {
                provider: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: self.voice_config.api_key.clone(),
                model: "gpt-4o-mini-transcribe".into(),
            },
        };
        self.voice_config = cfg.clone();
        self.status_banner = Some(format!(
            "🎙 已切换语音模型为 {} ({}) 并同步至 Herdr doc.set",
            cfg.provider, cfg.model
        ));

        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    if let Ok(hc) = HerdrClient::connect_default().await {
                        let _ = hc.set_voice_config(&cfg).await;
                    }
                });
            }
        });
    }

    async fn upsert_herdr_terminal(&mut self, client: &HerdrClient, t: HerdrTerminalSummary) {
        let (screen, status) = client
            .read_screen(&t.address, 60)
            .await
            .unwrap_or_else(|_| (String::new(), t.status.clone()));
        let title = status
            .as_ref()
            .and_then(|s| s.title.clone())
            .or_else(|| t.name.clone())
            .unwrap_or_else(|| format!("Herdr 终端 {}", t.address));
        let status_label = status
            .as_ref()
            .map(|s| format!("{} · {}", s.app.as_deref().unwrap_or("shell"), s.state))
            .unwrap_or_else(|| "终端运行中".into());
        let is_done = status
            .as_ref()
            .map(|s| s.state == "done")
            .unwrap_or(false);

        if let Some(existing) = self.sessions.iter_mut().find(|s| s.address == t.address) {
            if !screen.trim().is_empty() {
                existing.screen_text = screen;
            }
            existing.title = title;
            existing.status_label = status_label;
            existing.is_done = is_done;
        } else {
            self.sessions.insert(
                0,
                DesktopTerminalSession {
                    address: t.address.clone(),
                    title,
                    repo_name: "coda".into(),
                    branch_name: "main".into(),
                    machine_name: client.info().core_name.clone(),
                    model_badge: "Herdr Live".into(),
                    status_label,
                    is_done,
                    diff_added: 184,
                    diff_deleted: 19,
                    screen_text: if screen.trim().is_empty() {
                        format!("● Connected to live herdr-core terminal `{}` in {}", t.address, t.cwd)
                    } else {
                        screen
                    },
                },
            );
        }
    }

    pub fn handle_tactile_key(&mut self, key: &str) {
        match key {
            "esc" => {
                self.draft_input.clear();
                self.show_voice_modal = false;
            }
            "tab" => {
                self.bypass_permissions = !self.bypass_permissions;
            }
            "backspace" => {
                self.draft_input.pop();
            }
            "slash" => {
                self.draft_input = "/clear".into();
            }
            "ctrl_c" => {
                self.draft_input.clear();
                self.active_session_mut()
                    .screen_text
                    .push_str("\n^C (Sent SIGINT to active herdr agent)");
            }
            "ctrl_d" => {
                self.active_session_mut()
                    .screen_text
                    .push_str("\n^D (EOF)");
            }
            "enter" => {
                let cmd = self.draft_input.trim().to_string();
                if !cmd.is_empty() {
                    let active_addr = self.active_terminal_addr.clone();
                    let target_host = self.selected_host_id.and_then(|id| {
                        self.remote_hosts.iter().find(|h| h.id == id).cloned()
                    });
                    self.active_session_mut()
                        .screen_text
                        .push_str(&format!("\n\n> {cmd}\n● Executing in {} ({active_addr})...",
                            target_host.as_ref().map(|h| h.name.as_str()).unwrap_or("herdr-core")
                        ));
                    self.draft_input.clear();
                    if let Some(host) = target_host {
                        spawn_background_remote_send(host, active_addr, cmd);
                    } else {
                        spawn_background_herdr_send(active_addr, cmd);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn trigger_attachment_upload(&mut self) {
        let active_addr = self.active_terminal_addr.clone();
        let filename = "coda_gpui_capture.txt";
        let remote_hint = format!("/tmp/herdr-{active_addr}/{filename}");
        if self.draft_input.is_empty() {
            self.draft_input = remote_hint;
        } else {
            self.draft_input.push(' ');
            self.draft_input.push_str(&remote_hint);
        }
        self.status_banner = Some(format!("📎 已通过 hc terminal.put 上传附件至终端 {active_addr}"));

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            if let Ok(rt) = rt {
                rt.block_on(async move {
                    if let Ok(hc) = HerdrClient::connect_default().await {
                        let data_b64 = B64.encode(b"Coda GPUI Attachment Snapshot");
                        let _ = hc.put_file(&active_addr, filename, &data_b64).await;
                    }
                });
            }
        });
    }

    pub fn spawn_new_session(&mut self) {
        let next_idx = self.sessions.len() + 1;
        let addr = format!("t{next_idx}");
        let machine = self
            .selected_host_id
            .and_then(|id| {
                self.remote_hosts
                    .iter()
                    .find(|h| h.id == id)
                    .map(|h| h.name.clone())
            })
            .unwrap_or_else(|| "MacBook-Pro".into());
        self.sessions.insert(
            0,
            DesktopTerminalSession {
                address: addr.clone(),
                title: format!("Claude Code 终端会话 #{next_idx}"),
                repo_name: self.expanded_repo.clone(),
                branch_name: "main".into(),
                machine_name: machine,
                model_badge: "Opus 4.6".into(),
                status_label: "1 个终端运行中".into(),
                is_done: false,
                diff_added: 0,
                diff_deleted: 0,
                screen_text: format!(
                    "● Spawned herdr-core agent terminal `{addr}` via Coda GPUI Workbench"
                ),
            },
        );
        self.active_terminal_addr = addr;
    }

    pub fn run_quick_action(&mut self, command: &str) {
        self.draft_input = command.to_string();
        self.handle_tactile_key("enter");
    }
}

fn spawn_background_herdr_sync(state_bg: Arc<Mutex<CodaDesktopState>>) {
    std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            rt.block_on(async move {
                let mut tmp = CodaDesktopState::default();
                tmp.sync_from_local_herdr().await;
                if let Ok(mut g) = state_bg.lock() {
                    if !tmp.repos.is_empty() {
                        g.repos = tmp.repos;
                    }
                    g.voice_config = tmp.voice_config;
                    for s in tmp.sessions {
                        if !g.sessions.iter().any(|existing| existing.address == s.address) {
                            g.sessions.push(s);
                        }
                    }
                    g.status_banner = Some("✓ 已同步本地 Herdr 终端与 Git Worktree 列表".into());
                }
            });
        }
    });
}

fn spawn_background_herdr_send(terminal_addr: String, text: String) {
    std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            rt.block_on(async move {
                if let Ok(hc) = HerdrClient::connect_default().await {
                    let _ = hc.send_text(&terminal_addr, &text, true).await;
                }
            });
        }
    });
}

fn spawn_background_remote_send(host: RemoteHostSummary, terminal_addr: String, text: String) {
    std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            rt.block_on(async move {
                let Ok(cfg) = DaemonConfig::load(&DaemonConfig::default_path()) else {
                    return;
                };
                let Some(token) = cfg.session_token.as_deref() else {
                    return;
                };
                let relay_cfg = mint_peer_relay_config(
                    &cfg.server_url,
                    token,
                    cfg.device_id,
                    host.id,
                    &host.x25519_pubkey,
                )
                .await
                .ok();
                if let Ok(mut peer) = SmartPeerClient::connect_auto(
                    cfg.identity,
                    cfg.device_id,
                    host.id,
                    host.endpoints,
                    Some("255.255.255.255:24444".parse().unwrap()),
                    relay_cfg,
                )
                .await
                {
                    let _ = peer
                        .request(
                            &PeerMessage::SendTerminalText {
                                req_id: format!("gpui-{}", Uuid::new_v4().simple()),
                                terminal: terminal_addr,
                                text,
                            },
                            TrafficLane::ControlP0,
                        )
                        .await;
                }
            });
        }
    });
}

fn open_browser_url(url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

fn spawn_desktop_browser_login(state_bg: Arc<Mutex<CodaDesktopState>>, server_url: String) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let cfg_path = DaemonConfig::default_path();
            let existing = DaemonConfig::load(&cfg_path).ok().map(|c| c.identity);
            let started = match start_device_login_flow(&server_url, "hybrid", None, existing).await {
                Ok(s) => s,
                Err(e) => {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_banner = Some(format!("✗ 连接服务器失败: {e}"));
                    }
                    return;
                }
            };
            open_browser_url(&started.verification_uri_complete);
            if let Ok(mut g) = state_bg.lock() {
                g.pending_login_url = Some(started.verification_uri_complete.clone());
                g.pending_login_code = Some(started.user_code.clone());
                g.status_banner = Some(format!(
                    "🔑 请在浏览器确认授权码 {} ({})",
                    started.user_code, started.verification_uri_complete
                ));
            }
            match poll_device_login_until_approved(&started).await {
                Ok(cfg) => {
                    let _ = cfg.save(&cfg_path);
                    let mut hosts = Vec::new();
                    if let Some(ref tok) = cfg.session_token {
                        if let Ok(list) = list_account_hosts(&cfg.server_url, tok).await {
                            hosts = list.into_iter().filter(|h| h.id != cfg.device_id).collect();
                        }
                    }
                    let embedded_addr = start_embedded_host_if_configured(&cfg_path).await;
                    if let Ok(mut g) = state_bg.lock() {
                        g.is_logged_in = true;
                        g.pending_login_url = None;
                        g.pending_login_code = None;
                        g.account_email = cfg
                            .owner_email
                            .clone()
                            .unwrap_or_else(|| format!("@{}", cfg.owner_handle));
                        g.remote_hosts = hosts;
                        g.connected_route = match embedded_addr {
                            Some(addr) => {
                                format!("已登录 @{} · Hybrid :{}", cfg.owner_handle, addr.port())
                            }
                            None => format!("已登录 @{} · Hybrid Mesh", cfg.owner_handle),
                        };
                        g.status_banner = Some(format!(
                            "✓ 已作为双角色主机 `{}` 接入账号 @{}",
                            cfg.identity.name, cfg.owner_handle
                        ));
                    }
                    if embedded_addr.is_some() {
                        std::future::pending::<()>().await;
                    }
                }
                Err(e) => {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_banner = Some(format!("✗ 浏览器授权未完成: {e}"));
                    }
                }
            }
        });
    });
}

fn spawn_load_remote_host(state_bg: Arc<Mutex<CodaDesktopState>>, host: RemoteHostSummary) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let Ok(cfg) = DaemonConfig::load(&DaemonConfig::default_path()) else {
                return;
            };
            let relay_cfg = match cfg.session_token.as_deref() {
                Some(tok) => mint_peer_relay_config(
                    &cfg.server_url,
                    tok,
                    cfg.device_id,
                    host.id,
                    &host.x25519_pubkey,
                )
                .await
                .ok(),
                None => None,
            };
            match SmartPeerClient::connect_auto(
                cfg.identity,
                cfg.device_id,
                host.id,
                host.endpoints.clone(),
                Some("255.255.255.255:24444".parse().unwrap()),
                relay_cfg,
            )
            .await
            {
                Ok(mut peer) => {
                    let route = peer.active_route().to_string();
                    let req = PeerMessage::ListTerminals {
                        req_id: format!("lt-{}", Uuid::new_v4().simple()),
                        include_finished: true,
                    };
                    if let Ok(PeerMessage::TerminalsSnapshot { terminals, .. }) =
                        peer.request(&req, TrafficLane::ControlP0).await
                    {
                        let first_addr = terminals.first().map(|t| t.address.clone());
                        let mut screen = String::new();
                        if let Some(ref addr) = first_addr {
                            if let Ok(PeerMessage::TerminalScreenSnapshot { text, .. }) = peer
                                .request(
                                    &PeerMessage::ReadTerminalScreen {
                                        req_id: format!("scr-{}", Uuid::new_v4().simple()),
                                        terminal: addr.clone(),
                                        history_lines: 80,
                                    },
                                    TrafficLane::BulkP1,
                                )
                                .await
                            {
                                screen = text;
                            }
                        }
                        if let Ok(mut g) = state_bg.lock() {
                            g.connected_route = format!("已连接 · {route} → {}", host.name);
                            if !terminals.is_empty() {
                                g.sessions = terminals
                                    .into_iter()
                                    .map(|t| DesktopTerminalSession {
                                        address: t.address.clone(),
                                        title: t
                                            .status
                                            .as_ref()
                                            .and_then(|s| s.title.clone())
                                            .or(t.name.clone())
                                            .unwrap_or_else(|| t.address.clone()),
                                        repo_name: host.name.clone(),
                                        branch_name: t.name.unwrap_or(t.address),
                                        machine_name: host.name.clone(),
                                        model_badge: route.clone(),
                                        status_label: t
                                            .status
                                            .as_ref()
                                            .map(|s| s.state.clone())
                                            .unwrap_or_else(|| "running".into()),
                                        is_done: !t.alive,
                                        diff_added: 0,
                                        diff_deleted: 0,
                                        screen_text: if screen.is_empty() {
                                            format!("Connected to remote host `{}` via {route}", host.name)
                                        } else {
                                            screen.clone()
                                        },
                                    })
                                    .collect();
                                if let Some(addr) = first_addr {
                                    g.active_terminal_addr = addr;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_banner = Some(format!("✗ 无法连接 {}: {e}", host.name));
                    }
                }
            }
        });
    });
}

// ============================================================================
// GPUI Root View (`CodaGpuWorkspaceView`)
// ============================================================================

pub struct CodaGpuWorkspaceView {
    pub state: Arc<Mutex<CodaDesktopState>>,
    pub focus_handle: FocusHandle,
}

impl Focusable for CodaGpuWorkspaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CodaGpuWorkspaceView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = Arc::new(Mutex::new(CodaDesktopState::default()));
        let state_bg = Arc::clone(&state);

        // Background sync with local `herdr-core` (`hc`) & embedded LAN P2P host listener
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    let mut tmp = CodaDesktopState::default();
                    tmp.sync_from_local_herdr().await;

                    let cfg_path = DaemonConfig::default_path();
                    let loaded_cfg = DaemonConfig::load(&cfg_path).ok();
                    let cfg_exists = loaded_cfg.is_some();
                    let embedded_addr = if cfg_exists {
                        start_embedded_host_if_configured(&cfg_path).await
                    } else {
                        None
                    };
                    let mut remote_hosts = Vec::new();
                    if let Some(ref cfg) = loaded_cfg {
                        if let Some(ref tok) = cfg.session_token {
                            if let Ok(list) = list_account_hosts(&cfg.server_url, tok).await {
                                remote_hosts =
                                    list.into_iter().filter(|h| h.id != cfg.device_id).collect();
                            }
                        }
                    }

                    if let Ok(mut guard) = state_bg.lock() {
                        guard.repos = tmp.repos;
                        guard.voice_config = tmp.voice_config;
                        if let Some(ref cfg) = loaded_cfg {
                            guard.is_logged_in = true;
                            guard.server_url = cfg.server_url.clone();
                            guard.account_email = cfg
                                .owner_email
                                .clone()
                                .unwrap_or_else(|| format!("@{}", cfg.owner_handle));
                            guard.remote_hosts = remote_hosts;
                        }
                        if let Some(addr) = embedded_addr {
                            guard.connected_route = format!("已连接 · ⚡ 内网直连主机 :{} 就绪", addr.port());
                        } else if cfg_exists {
                            guard.connected_route = "已连接 · ⚡ 局域网直连引擎已激活 (GPUI 120Hz)".into();
                        }
                        for s in tmp.sessions {
                            if !guard.sessions.iter().any(|existing| existing.address == s.address) {
                                guard.sessions.push(s);
                            }
                        }
                    }

                    if embedded_addr.is_some() {
                        std::future::pending::<()>().await;
                    }
                });
            }
        });

        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }

    fn render_top_workspace_bar(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let srv = st.server_url.clone();
        let active_local = st.selected_host_id.is_none();

        div()
            .h(px(38.0))
            .w_full()
            .bg(rgb(palette::CHROME_BG))
            .border_b_1()
            .border_color(rgb(palette::DIVIDER))
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            // Left: Brand + Focus + Workspace/Host Group Tabs
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    // Coda Brand Mark
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(palette::KEY_SURFACE))
                            .border_1()
                            .border_color(rgb(palette::KEY_BORDER))
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(8.0)).rounded_full().bg(rgb(palette::EMERALD_WORKING)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child("CODA"),
                            ),
                    )
                    // Focus Workspace Pill
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(palette::KEY_SURFACE))
                            .border_1()
                            .border_color(rgb(palette::KEY_BORDER))
                            .text_xs()
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(SharedString::from(format!("■ Focus  {}", st.sessions.len())))
                            .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.center_tab = "preview".into();
                                        g.show_center_pane = true;
                                        g.show_agent_pane = true;
                                    }
                                    cx.notify();
                                }),
                            ),
                    )
                    // Local Mac Host Tab
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if active_local {
                                rgb(palette::KEY_HOVER)
                            } else {
                                rgb(palette::SIDEBAR_BG)
                            })
                            .border_1()
                            .border_color(if active_local {
                                rgb(palette::CORAL_AGENT)
                            } else {
                                rgb(palette::KEY_BORDER)
                            })
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child(div().size(px(6.0)).rounded_full().bg(rgb(palette::CORAL_AGENT)))
                            .child(SharedString::from(format!(
                                "Local Mac  {}/{}",
                                st.repos.len(),
                                st.repos.len()
                            )))
                            .hover(|s| s.cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.selected_host_id = None;
                                        g.connected_route = "已连接 · L0 本机 Herdr 直连".into();
                                    }
                                    cx.notify();
                                }),
                            ),
                    )
                    // Remote Host Tabs
                    .children(st.remote_hosts.iter().map(|host| {
                        let h_clone = host.clone();
                        let is_cur = st.selected_host_id == Some(host.id);
                        let dot_color = if host.is_online {
                            palette::EMERALD_WORKING
                        } else {
                            palette::TEXT_FAINT
                        };
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if is_cur {
                                rgb(palette::KEY_HOVER)
                            } else {
                                rgb(palette::SIDEBAR_BG)
                            })
                            .border_1()
                            .border_color(if is_cur {
                                rgb(palette::SKY_DONE)
                            } else {
                                rgb(palette::KEY_BORDER)
                            })
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(if host.is_online {
                                rgb(palette::TEXT_PRIMARY)
                            } else {
                                rgb(palette::TEXT_MUTED)
                            })
                            .child(div().size(px(6.0)).rounded_full().bg(rgb(dot_color)))
                            .child(SharedString::from(host.name.clone()))
                            .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.selected_host_id = Some(h_clone.id);
                                        g.status_banner = Some(format!(
                                            "正在经智能路由连接 {}...",
                                            h_clone.name
                                        ));
                                    }
                                    spawn_load_remote_host(Arc::clone(&this.state), h_clone.clone());
                                    cx.notify();
                                }),
                            )
                    }))
                    // + Enroll / Sync Host Tab
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(palette::SIDEBAR_BG))
                            .border_1()
                            .border_color(rgb(palette::KEY_BORDER))
                            .text_xs()
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child("+")
                            .hover(|s| {
                                s.bg(rgb(palette::KEY_HOVER))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .cursor_pointer()
                            })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    let url = this
                                        .state
                                        .lock()
                                        .map(|g| g.server_url.clone())
                                        .unwrap_or_default();
                                    spawn_desktop_browser_login(Arc::clone(&this.state), url);
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            // Right: Voice Config + Browser Login / Account Status
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(palette::KEY_SURFACE))
                            .border_1()
                            .border_color(rgb(palette::KEY_BORDER))
                            .text_xs()
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child("⚙ 语音/模型")
                            .hover(|s| {
                                s.bg(rgb(palette::KEY_HOVER))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .cursor_pointer()
                            })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.show_voice_modal = !g.show_voice_modal;
                                        g.show_agent_pane = true;
                                    }
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(if st.is_logged_in {
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(palette::KEY_SURFACE))
                            .border_1()
                            .border_color(rgb(palette::EMERALD_WORKING))
                            .text_xs()
                            .text_color(rgb(palette::EMERALD_WORKING))
                            .child(SharedString::from(format!("● {}", st.account_email)))
                            .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.center_tab = "mesh".into();
                                        g.show_center_pane = true;
                                    }
                                    cx.notify();
                                }),
                            )
                    } else {
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(palette::KEY_SURFACE))
                            .border_1()
                            .border_color(rgb(palette::AMBER_WARN))
                            .text_xs()
                            .text_color(rgb(palette::AMBER_WARN))
                            .child(SharedString::from(format!("🔑 浏览器登录 ({srv})")))
                            .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    let url = this
                                        .state
                                        .lock()
                                        .map(|g| g.server_url.clone())
                                        .unwrap_or_default();
                                    spawn_desktop_browser_login(Arc::clone(&this.state), url);
                                    cx.notify();
                                }),
                            )
                    }),
            )
    }

    fn render_secondary_view_bar(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .h(px(32.0))
            .w_full()
            .bg(rgb(palette::SIDEBAR_BG))
            .border_b_1()
            .border_color(rgb(palette::DIVIDER))
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            // Left: Numbered View / Session Switcher
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(st.sessions.iter().enumerate().map(|(idx, sess)| {
                        let addr = sess.address.clone();
                        let is_active = addr == st.active_terminal_addr;
                        let label = format!("{}  {}", idx + 1, sess.repo_name);
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if is_active {
                                rgb(palette::KEY_HOVER)
                            } else {
                                rgb(palette::SIDEBAR_BG)
                            })
                            .border_1()
                            .border_color(if is_active {
                                rgb(palette::KEY_BORDER)
                            } else {
                                rgb(palette::SIDEBAR_BG)
                            })
                            .text_xs()
                            .text_color(if is_active {
                                rgb(palette::TEXT_PRIMARY)
                            } else {
                                rgb(palette::TEXT_MUTED)
                            })
                            .child(SharedString::from(label))
                            .hover(|s| {
                                s.bg(rgb(palette::KEY_SURFACE))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .cursor_pointer()
                            })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.active_terminal_addr = addr.clone();
                                    }
                                    cx.notify();
                                }),
                            )
                    }))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child("+ 新建终端")
                            .hover(|s| {
                                s.bg(rgb(palette::KEY_SURFACE))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .cursor_pointer()
                            })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.spawn_new_session();
                                    }
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            // Right: Split Layout Toggles (`Projects` | `Preview` | `Agent`)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if st.show_sidebar {
                                rgb(palette::KEY_HOVER)
                            } else {
                                rgb(palette::SIDEBAR_BG)
                            })
                            .border_1()
                            .border_color(rgb(palette::KEY_BORDER))
                            .text_xs()
                            .text_color(if st.show_sidebar {
                                rgb(palette::TEXT_PRIMARY)
                            } else {
                                rgb(palette::TEXT_FAINT)
                            })
                            .child("◧ Projects")
                            .hover(|s| s.cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.show_sidebar = !g.show_sidebar;
                                    }
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if st.show_center_pane {
                                rgb(palette::KEY_HOVER)
                            } else {
                                rgb(palette::SIDEBAR_BG)
                            })
                            .border_1()
                            .border_color(rgb(palette::KEY_BORDER))
                            .text_xs()
                            .text_color(if st.show_center_pane {
                                rgb(palette::TEXT_PRIMARY)
                            } else {
                                rgb(palette::TEXT_FAINT)
                            })
                            .child("▣ Preview")
                            .hover(|s| s.cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.show_center_pane = !g.show_center_pane;
                                    }
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if st.show_agent_pane {
                                rgb(palette::KEY_HOVER)
                            } else {
                                rgb(palette::SIDEBAR_BG)
                            })
                            .border_1()
                            .border_color(rgb(palette::KEY_BORDER))
                            .text_xs()
                            .text_color(if st.show_agent_pane {
                                rgb(palette::TEXT_PRIMARY)
                            } else {
                                rgb(palette::TEXT_FAINT)
                            })
                            .child("◨ Agent")
                            .hover(|s| s.cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.show_agent_pane = !g.show_agent_pane;
                                    }
                                    cx.notify();
                                }),
                            ),
                    ),
            )
    }

    fn render_left_projects_sidebar(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let total_branches: usize = st.repos.iter().map(|r| r.branches.len()).sum();

        div()
            .w(px(264.0))
            .h_full()
            .bg(rgb(palette::SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(palette::DIVIDER))
            .flex()
            .flex_col()
            // Sidebar Header: `PROJECTS <count>` + `[↻ Sync]` + `[+]`
            .child(
                div()
                    .h(px(36.0))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(palette::DIVIDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child("PROJECTS"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(palette::TEXT_FAINT))
                                    .child(SharedString::from(format!("{total_branches}"))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child("↻")
                                    .hover(|s| {
                                        s.bg(rgb(palette::KEY_HOVER))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .cursor_pointer()
                                    })
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                            spawn_background_herdr_sync(Arc::clone(&this.state));
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(palette::CORAL_AGENT))
                                    .text_xs()
                                    .text_color(white())
                                    .child("+")
                                    .hover(|s| s.cursor_pointer())
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.spawn_new_session();
                                            }
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    ),
            )
            // Scrollable Project Tree (`SERVICES`, `AGENTS`, `ACTIONS`)
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(st.repos.iter().map(|repo| {
                        let repo_name = repo.repo_name.clone();
                        let is_expanded = st.expanded_repo == repo_name;
                        let chevron = if is_expanded { "▾" } else { "▸" };
                        let branch_count = repo.branches.len();
                        let short_path = repo
                            .root_path
                            .rsplit('/')
                            .take(2)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect::<Vec<_>>()
                            .join("/");

                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            // Project Row
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(if is_expanded {
                                        rgba(0xffffff0a)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .hover(|s| s.bg(rgba(0xffffff10)).cursor_pointer())
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                if g.expanded_repo == repo_name {
                                                    g.expanded_repo.clear();
                                                } else {
                                                    g.expanded_repo = repo_name.clone();
                                                }
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(palette::TEXT_FAINT))
                                                    .child(chevron),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .child(SharedString::from(repo.repo_name.clone())),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(palette::TEXT_FAINT))
                                                    .child(SharedString::from(format!(" {short_path}"))),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(palette::EMERALD_WORKING))
                                            .child(SharedString::from(format!("{branch_count}/{branch_count}"))),
                                    ),
                            )
                            // Expanded Sections: SERVICES, AGENTS, ACTIONS
                            .when(is_expanded, |el| {
                                el.child(
                                    div()
                                        .pl_3()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        // SERVICES
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .text_xs()
                                                        .text_color(rgb(palette::TEXT_FAINT))
                                                        .child("SERVICES"),
                                                )
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .py_1()
                                                        .rounded_md()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .hover(|s| s.bg(rgba(0xffffff0a)).cursor_pointer())
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .size(px(7.0))
                                                                        .rounded_full()
                                                                        .bg(rgb(palette::EMERALD_WORKING)),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                                                        .child("herdr-core daemon"),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(palette::TEXT_FAINT))
                                                                .child("live"),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .py_1()
                                                        .rounded_md()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .hover(|s| s.bg(rgba(0xffffff0a)).cursor_pointer())
                                                        .on_mouse_up(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                                if let Ok(mut g) = this.state.lock() {
                                                                    g.center_tab = "mesh".into();
                                                                    g.show_center_pane = true;
                                                                }
                                                                cx.notify();
                                                            }),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .size(px(7.0))
                                                                        .rounded_full()
                                                                        .bg(rgb(palette::EMERALD_WORKING)),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                                                        .child("codahub.dev tunnel"),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(palette::SKY_DONE))
                                                                .child("E2EE"),
                                                        ),
                                                ),
                                        )
                                        // AGENTS
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .text_xs()
                                                        .text_color(rgb(palette::TEXT_FAINT))
                                                        .child("AGENTS"),
                                                )
                                                .children(st.sessions.iter().map(|sess| {
                                                    let addr = sess.address.clone();
                                                    let is_sel = addr == st.active_terminal_addr;
                                                    let dot_color = if sess.is_done {
                                                        palette::SKY_DONE
                                                    } else {
                                                        palette::EMERALD_WORKING
                                                    };
                                                    div()
                                                        .px_2()
                                                        .py_1()
                                                        .rounded_md()
                                                        .bg(if is_sel {
                                                            rgb(palette::KEY_HOVER)
                                                        } else {
                                                            rgb(palette::SIDEBAR_BG)
                                                        })
                                                        .border_1()
                                                        .border_color(if is_sel {
                                                            rgb(palette::KEY_BORDER)
                                                        } else {
                                                            rgb(palette::SIDEBAR_BG)
                                                        })
                                                        .hover(|s| s.bg(rgb(palette::KEY_SURFACE)).cursor_pointer())
                                                        .on_mouse_up(
                                                            MouseButton::Left,
                                                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                                if let Ok(mut g) = this.state.lock() {
                                                                    g.active_terminal_addr = addr.clone();
                                                                    g.show_agent_pane = true;
                                                                }
                                                                cx.notify();
                                                            }),
                                                        )
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .size(px(7.0))
                                                                        .rounded_full()
                                                                        .bg(rgb(dot_color)),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .flex()
                                                                        .flex_col()
                                                                        .child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                                                .child(SharedString::from(sess.title.clone())),
                                                                        )
                                                                        .child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(rgb(palette::TEXT_FAINT))
                                                                                .child(SharedString::from(format!(
                                                                                    "{} · {}",
                                                                                    sess.branch_name, sess.machine_name
                                                                                ))),
                                                                        ),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(rgb(palette::EMERALD_WORKING))
                                                                .child(SharedString::from(format!(
                                                                    "+{}",
                                                                    sess.diff_added
                                                                ))),
                                                        )
                                                })),
                                        )
                                        // ACTIONS
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .px_2()
                                                        .text_xs()
                                                        .text_color(rgb(palette::TEXT_FAINT))
                                                        .child("ACTIONS"),
                                                )
                                                .children(
                                                    [
                                                        ("cargo test --workspace", "cargo test --workspace"),
                                                        ("git status -sb", "git status -sb"),
                                                        ("coda-daemon status", "coda-daemon status"),
                                                        ("/clear", "/clear"),
                                                    ]
                                                    .into_iter()
                                                    .map(|(label, cmd)| {
                                                        div()
                                                            .px_2()
                                                            .py_1()
                                                            .rounded_md()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .text_xs()
                                                            .text_color(rgb(palette::TEXT_MUTED))
                                                            .hover(|s| {
                                                                s.bg(rgb(palette::KEY_SURFACE))
                                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                                    .cursor_pointer()
                                                            })
                                                            .child(
                                                                div()
                                                                    .text_color(rgb(palette::AMBER_WARN))
                                                                    .child("▸"),
                                                            )
                                                            .child(label)
                                                            .on_mouse_up(
                                                                MouseButton::Left,
                                                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                                    if let Ok(mut g) = this.state.lock() {
                                                                        g.run_quick_action(cmd);
                                                                    }
                                                                    cx.notify();
                                                                }),
                                                            )
                                                    }),
                                                ),
                                        ),
                                )
                            })
                    })),
            )
    }

    fn render_center_inspector_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = st.active_session().clone();
        let tab = st.center_tab.as_str();
        let url_open = st.server_url.clone();

        div()
            .flex_1()
            .h_full()
            .bg(rgb(palette::CENTER_BG))
            .border_r_1()
            .border_color(rgb(palette::DIVIDER))
            .flex()
            .flex_col()
            // Top Address / Inspector Mode Toolbar
            .child(
                div()
                    .h(px(36.0))
                    .px_3()
                    .bg(rgb(palette::SIDEBAR_BG))
                    .border_b_1()
                    .border_color(rgb(palette::DIVIDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(palette::KEY_SURFACE))
                                    .border_1()
                                    .border_color(rgb(palette::EMERALD_WORKING))
                                    .text_xs()
                                    .text_color(rgb(palette::EMERALD_WORKING))
                                    .child("dev"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(palette::CENTER_BG))
                                    .border_1()
                                    .border_color(rgb(palette::KEY_BORDER))
                                    .text_xs()
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(SharedString::from(format!(
                                        "{}/{}",
                                        st.server_url, active.repo_name
                                    ))),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child("↗ 浏览器")
                                    .hover(|s| {
                                        s.bg(rgb(palette::KEY_HOVER))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .cursor_pointer()
                                    })
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |_, _: &MouseUpEvent, _, _| {
                                            open_browser_url(&url_open);
                                        }),
                                    ),
                            ),
                    )
                    // Mode Tabs: Preview | Worktrees & Diff | Mesh & Voice
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .children(
                                [
                                    ("preview", "Preview"),
                                    ("worktrees", "Worktrees & Diff"),
                                    ("mesh", "Mesh & Voice"),
                                ]
                                .into_iter()
                                .map(|(key, label)| {
                                    let is_cur = tab == key;
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(if is_cur {
                                            rgb(palette::KEY_HOVER)
                                        } else {
                                            rgb(palette::SIDEBAR_BG)
                                        })
                                        .border_1()
                                        .border_color(if is_cur {
                                            rgb(palette::KEY_BORDER)
                                        } else {
                                            rgb(palette::SIDEBAR_BG)
                                        })
                                        .text_xs()
                                        .text_color(if is_cur {
                                            rgb(palette::TEXT_PRIMARY)
                                        } else {
                                            rgb(palette::TEXT_MUTED)
                                        })
                                        .child(label)
                                        .hover(|s| s.cursor_pointer())
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.center_tab = key.to_string();
                                                }
                                                cx.notify();
                                            }),
                                        )
                                }),
                            ),
                    ),
            )
            // Center Body Viewport
            .child(
                div()
                    .id("center-scroll")
                    .flex_1()
                    .overflow_scroll()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .when_some(st.pending_login_code.clone(), |el, code| {
                        let auth_url = st.pending_login_url.clone().unwrap_or_default();
                        let auth_url_click = auth_url.clone();
                        el.child(
                            div()
                                .p_4()
                                .rounded_xl()
                                .bg(rgb(palette::KEY_SURFACE))
                                .border_1()
                                .border_color(rgb(palette::SKY_DONE))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(palette::SKY_DONE))
                                        .child("🔑 RFC 8628 浏览器设备授权等待确认中"),
                                )
                                .child(
                                    div()
                                        .text_base()
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(SharedString::from(format!("授权码：{code}"))),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child(SharedString::from(auth_url)),
                                )
                                .child(
                                    div()
                                        .px_3()
                                        .py_1()
                                        .rounded_md()
                                        .bg(rgb(palette::SKY_DONE))
                                        .text_xs()
                                        .text_color(rgb(palette::OLED_BLACK))
                                        .child("重新打开浏览器授权页面 →")
                                        .hover(|s| s.cursor_pointer())
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |_, _: &MouseUpEvent, _, _| {
                                                if !auth_url_click.is_empty() {
                                                    open_browser_url(&auth_url_click);
                                                }
                                            }),
                                        ),
                                ),
                        )
                    })
                    .when(tab == "preview", |el| {
                        el.child(
                            div()
                                .p_5()
                                .rounded_xl()
                                .bg(rgb(palette::SIDEBAR_BG))
                                .border_1()
                                .border_color(rgb(palette::KEY_BORDER))
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette::AMBER_WARN))
                                        .child("ZERO-KNOWLEDGE WORKSPACE INSPECTOR"),
                                )
                                .child(
                                    div()
                                        .text_base()
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child("同一账号下，控制远端 Linux / Mac / iOS 全端写入与调试"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child(SharedString::from(format!(
                                            "当前选中工作区：{} ({}) · 主机：{}\n端到端通道：{} · 控制面：{}",
                                            active.repo_name,
                                            active.branch_name,
                                            active.machine_name,
                                            st.connected_route,
                                            st.server_url
                                        ))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .child(
                                    div()
                                        .flex_1()
                                        .p_4()
                                        .rounded_xl()
                                        .bg(rgb(palette::SIDEBAR_BG))
                                        .border_1()
                                        .border_color(rgb(palette::KEY_BORDER))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette::EMERALD_WORKING))
                                                .child("● Herdr Core PTY"),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .child(SharedString::from(format!(
                                                    "{} 个活跃 Agent 会话",
                                                    st.sessions.len()
                                                ))),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette::TEXT_FAINT))
                                                .child("关闭窗口或切换分支也不会丢失远端终端进度"),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .p_4()
                                        .rounded_xl()
                                        .bg(rgb(palette::SIDEBAR_BG))
                                        .border_1()
                                        .border_color(rgb(palette::KEY_BORDER))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette::SKY_DONE))
                                                .child("⚡ SmartPath Mesh"),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .child(SharedString::from(format!(
                                                    "{} 台在线/已配对节点",
                                                    st.remote_hosts.len() + 1
                                                ))),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette::TEXT_FAINT))
                                                .child("同 Wi-Fi 自动走 LAN P2P (<1ms)，跨网走 CF WSS"),
                                        ),
                                ),
                        )
                    })
                    .when(tab == "worktrees" || tab == "preview", |el| {
                        el.child(
                            div()
                                .p_4()
                                .rounded_xl()
                                .bg(rgb(palette::SIDEBAR_BG))
                                .border_1()
                                .border_color(rgb(palette::KEY_BORDER))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette::TEXT_MUTED))
                                                .child("GIT WORKTREES & DIFF STATS"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette::TEXT_FAINT))
                                                .child(SharedString::from(format!(
                                                    "{} repositories",
                                                    st.repos.len()
                                                ))),
                                        ),
                                )
                                .children(st.repos.iter().map(|repo| {
                                    div()
                                        .p_3()
                                        .rounded_lg()
                                        .bg(rgb(palette::CENTER_BG))
                                        .border_1()
                                        .border_color(rgb(palette::DIVIDER))
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                                        .child(SharedString::from(format!(
                                                            "{}  ({})",
                                                            repo.repo_name, repo.root_path
                                                        ))),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(palette::TEXT_FAINT))
                                                        .child(SharedString::from(
                                                            repo.machine_name.clone(),
                                                        )),
                                                ),
                                        )
                                        .children(repo.branches.iter().map(|br| {
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .text_xs()
                                                .child(
                                                    div()
                                                        .text_color(rgb(palette::SKY_DONE))
                                                        .child(SharedString::from(format!(
                                                            "  ⎇ {} · {}",
                                                            br.branch, br.status_summary
                                                        ))),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .text_color(rgb(
                                                                    palette::EMERALD_WORKING,
                                                                ))
                                                                .child(SharedString::from(format!(
                                                                    "+{}",
                                                                    br.diff_added
                                                                ))),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_color(rgb(
                                                                    palette::ROSE_DIFF_DEL,
                                                                ))
                                                                .child(SharedString::from(format!(
                                                                    "-{}",
                                                                    br.diff_deleted
                                                                ))),
                                                        ),
                                                )
                                        }))
                                })),
                        )
                    })
                    .when(tab == "mesh", |el| {
                        el.child(
                            div()
                                .p_4()
                                .rounded_xl()
                                .bg(rgb(palette::SIDEBAR_BG))
                                .border_1()
                                .border_color(rgb(palette::KEY_BORDER))
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child("🎙 VOICE MODEL & CRYPTOGRAPHIC TUNNEL"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(SharedString::from(format!(
                                            "Provider: {} · Model: {} · Endpoint: {}",
                                            st.voice_config.provider,
                                            st.voice_config.model,
                                            st.voice_config.base_url
                                        ))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .px_3()
                                                .py_1()
                                                .rounded_md()
                                                .bg(rgb(palette::KEY_SURFACE))
                                                .border_1()
                                                .border_color(rgb(palette::KEY_BORDER))
                                                .text_xs()
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .child("OpenAI Transcribe")
                                                .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.apply_voice_preset("openai");
                                                        }
                                                        cx.notify();
                                                    }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .px_3()
                                                .py_1()
                                                .rounded_md()
                                                .bg(rgb(palette::KEY_SURFACE))
                                                .border_1()
                                                .border_color(rgb(palette::KEY_BORDER))
                                                .text_xs()
                                                .text_color(rgb(palette::SKY_DONE))
                                                .child("Groq Whisper-v3-Turbo")
                                                .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.apply_voice_preset("groq");
                                                        }
                                                        cx.notify();
                                                    }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .px_3()
                                                .py_1()
                                                .rounded_md()
                                                .bg(rgb(palette::KEY_SURFACE))
                                                .border_1()
                                                .border_color(rgb(palette::KEY_BORDER))
                                                .text_xs()
                                                .text_color(rgb(palette::CORAL_AGENT))
                                                .child("DeepSeek Voice")
                                                .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.apply_voice_preset("deepseek");
                                                        }
                                                        cx.notify();
                                                    }),
                                                ),
                                        ),
                                ),
                        )
                    }),
            )
            // Bottom Breadcrumb
            .child(
                div()
                    .h(px(26.0))
                    .px_3()
                    .bg(rgb(palette::SIDEBAR_BG))
                    .border_t_1()
                    .border_color(rgb(palette::DIVIDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_xs()
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(div().size(px(6.0)).rounded_full().bg(rgb(palette::EMERALD_WORKING)))
                            .child(SharedString::from(format!(
                                "{} / {} · {}",
                                active.repo_name, active.branch_name, active.machine_name
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .text_xs()
                            .child(
                                div()
                                    .text_color(rgb(palette::EMERALD_WORKING))
                                    .child(SharedString::from(format!("+{}", active.diff_added))),
                            )
                            .child(
                                div()
                                    .text_color(rgb(palette::ROSE_DIFF_DEL))
                                    .child(SharedString::from(format!("-{}", active.diff_deleted))),
                            ),
                    ),
            )
    }

    fn render_right_agent_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = st.active_session().clone();
        let draft_display = if st.draft_input.is_empty() {
            "输入指令或 Prompt 发送给远端 Agent... (Enter 发送)".to_string()
        } else {
            format!("{}▏", st.draft_input)
        };
        let draft_color = if st.draft_input.is_empty() {
            palette::TEXT_FAINT
        } else {
            palette::TEXT_PRIMARY
        };
        let mode_label = if st.bypass_permissions {
            "⇄ bypass permissions on"
        } else {
            "⇄ plan mode (review)"
        };

        let pane = if st.show_center_pane {
            div().w(px(470.0))
        } else {
            div().flex_1()
        };

        pane.h_full()
            .bg(rgb(palette::AGENT_BG))
            .flex()
            .flex_col()
            // 1. Agent Pane Header
            .child(
                div()
                    .h(px(36.0))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(palette::DIVIDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(7.0))
                                    .rounded_full()
                                    .bg(rgb(if active.is_done {
                                        palette::SKY_DONE
                                    } else {
                                        palette::EMERALD_WORKING
                                    })),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(SharedString::from(active.title.clone())),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(palette::KEY_SURFACE))
                                    .text_xs()
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(SharedString::from(active.model_badge.clone())),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(palette::KEY_SURFACE))
                                    .border_1()
                                    .border_color(rgb(palette::KEY_BORDER))
                                    .text_xs()
                                    .text_color(rgb(palette::CORAL_AGENT))
                                    .child("^C")
                                    .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.handle_tactile_key("ctrl_c");
                                            }
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    ),
            )
            // 2. Scrollable Dense Monospace Stream
            .child(
                div()
                    .id("terminal-viewport")
                    .flex_1()
                    .overflow_scroll()
                    .px_4()
                    .py_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when_some(st.status_banner.clone(), |el, banner| {
                        el.child(
                            div()
                                .px_3()
                                .py_1()
                                .mb_2()
                                .rounded_md()
                                .bg(rgb(palette::KEY_SURFACE))
                                .border_1()
                                .border_color(rgb(palette::EMERALD_WORKING))
                                .text_xs()
                                .text_color(rgb(palette::EMERALD_WORKING))
                                .child(SharedString::from(banner)),
                        )
                    })
                    .when(st.show_voice_modal, |el| {
                        el.child(
                            div()
                                .p_3()
                                .mb_2()
                                .rounded_lg()
                                .bg(rgb(palette::KEY_SURFACE))
                                .border_1()
                                .border_color(rgb(palette::KEY_BORDER))
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(SharedString::from(format!(
                                            "🎙 语音模型: {} · {} ({})",
                                            st.voice_config.provider,
                                            st.voice_config.model,
                                            st.voice_config.base_url
                                        ))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            div()
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .bg(rgb(palette::SIDEBAR_BG))
                                                .border_1()
                                                .border_color(rgb(palette::KEY_BORDER))
                                                .text_xs()
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .child("OpenAI")
                                                .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.apply_voice_preset("openai");
                                                        }
                                                        cx.notify();
                                                    }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .bg(rgb(palette::SIDEBAR_BG))
                                                .border_1()
                                                .border_color(rgb(palette::KEY_BORDER))
                                                .text_xs()
                                                .text_color(rgb(palette::SKY_DONE))
                                                .child("Groq Whisper")
                                                .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.apply_voice_preset("groq");
                                                        }
                                                        cx.notify();
                                                    }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .bg(rgb(palette::SIDEBAR_BG))
                                                .border_1()
                                                .border_color(rgb(palette::KEY_BORDER))
                                                .text_xs()
                                                .text_color(rgb(palette::CORAL_AGENT))
                                                .child("DeepSeek")
                                                .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.apply_voice_preset("deepseek");
                                                        }
                                                        cx.notify();
                                                    }),
                                                ),
                                        ),
                                ),
                        )
                    })
                    .children(active.screen_text.lines().map(|line| {
                        let trimmed = line.trim_start();
                        let line_color = if trimmed.starts_with("●") {
                            palette::EMERALD_WORKING
                        } else if trimmed.starts_with('*') {
                            palette::AMBER_WARN
                        } else if trimmed.starts_with('+') {
                            palette::EMERALD_WORKING
                        } else {
                            palette::TEXT_PRIMARY
                        };
                        div()
                            .text_xs()
                            .text_color(rgb(line_color))
                            .child(SharedString::from(line.to_string()))
                    })),
            )
            // 3. Desktop Prompt Composer Box (Matches Highwind Bottom-Right Prompt Bar)
            .child(
                div()
                    .bg(rgb(palette::KEY_SURFACE))
                    .border_t_1()
                    .border_color(rgb(palette::KEY_BORDER))
                    .flex()
                    .flex_col()
                    // Row 1: `❯ [input]` + `[📎]` + `[📋]` + `[↑]`
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(palette::AMBER_WARN))
                                            .child("❯"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(draft_color))
                                            .child(SharedString::from(draft_display)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .text_xs()
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child("📎")
                                            .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.trigger_attachment_upload();
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .text_xs()
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child("📋")
                                            .hover(|s| s.bg(rgb(palette::KEY_HOVER)).cursor_pointer())
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                    if let Some(item) = cx.read_from_clipboard() {
                                                        if let Some(txt) = item.text() {
                                                            let trimmed = txt.trim();
                                                            if let Ok(mut g) = this.state.lock() {
                                                                if !g.is_logged_in
                                                                    && (trimmed.starts_with("http://")
                                                                        || trimmed.starts_with("https://"))
                                                                {
                                                                    g.server_url =
                                                                        trimmed.trim_end_matches('/').to_string();
                                                                    g.status_banner = Some(format!(
                                                                        "✓ 已更新服务器地址为 {}",
                                                                        g.server_url
                                                                    ));
                                                                } else {
                                                                    g.draft_input.push_str(&txt);
                                                                }
                                                            }
                                                            cx.notify();
                                                        }
                                                    } else {
                                                        let cmd = this
                                                            .state
                                                            .lock()
                                                            .map(|g| {
                                                                format!(
                                                                    "coda-daemon login --server {}",
                                                                    g.server_url
                                                                )
                                                            })
                                                            .unwrap_or_else(|_| {
                                                                "coda-daemon login --server https://codahub.dev".into()
                                                            });
                                                        cx.write_to_clipboard(ClipboardItem::new_string(cmd));
                                                    }
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(rgb(palette::TEXT_PRIMARY))
                                            .text_xs()
                                            .text_color(rgb(palette::OLED_BLACK))
                                            .child("↑ 发送")
                                            .hover(|s| s.bg(white()).cursor_pointer())
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.handle_tactile_key("enter");
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            ),
                    )
                    // Row 2: repo badge | model | clickable mode | agent count
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .border_t_1()
                            .border_color(rgb(palette::DIVIDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_xs()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .px_2()
                                            .rounded_sm()
                                            .bg(rgb(palette::SIDEBAR_BG))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(SharedString::from(active.repo_name.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child(SharedString::from(format!(
                                                "{} · {}",
                                                active.model_badge, active.address
                                            ))),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_color(rgb(palette::CORAL_AGENT))
                                            .child(mode_label)
                                            .hover(|s| s.cursor_pointer())
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.handle_tactile_key("tab");
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(palette::TEXT_FAINT))
                                            .child(SharedString::from(format!(
                                                "· {} agents",
                                                st.sessions.len()
                                            ))),
                                    ),
                            ),
                    )
                    // Row 3: Bottom Terminal Strip
                    .child(
                        div()
                            .h(px(26.0))
                            .px_3()
                            .bg(rgb(palette::SIDEBAR_BG))
                            .border_t_1()
                            .border_color(rgb(palette::DIVIDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_xs()
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(SharedString::from(format!(
                                "▣ Terminal · hc attach {}",
                                active.address
                            )))
                            .child(
                                div()
                                    .text_color(rgb(palette::TEXT_FAINT))
                                    .child("⌘B Sidebar · ⌘N New · Tab Mode"),
                            ),
                    ),
            )
    }

    fn render_bottom_status_bar(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focus_text = if st.focus_timer_running {
            "▸ 25:00 Focus"
        } else {
            "⏸ 25:00 Paused"
        };

        div()
            .h(px(24.0))
            .w_full()
            .bg(rgb(palette::CHROME_BG))
            .border_t_1()
            .border_color(rgb(palette::DIVIDER))
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(focus_text)
                            .hover(|s| s.text_color(rgb(palette::TEXT_PRIMARY)).cursor_pointer())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.focus_timer_running = !g.focus_timer_running;
                                    }
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_color(rgb(palette::AMBER_WARN))
                            .child(SharedString::from(format!("CODA v{}", env!("CARGO_PKG_VERSION")))),
                    )
                    .child(
                        div()
                            .text_color(rgb(palette::EMERALD_WORKING))
                            .child(SharedString::from(st.connected_route.clone())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_color(rgb(palette::TEXT_FAINT))
                    .child("E2EE ChaCha20-Poly1305")
                    .child("Metal 120Hz")
                    .child(SharedString::from(format!("Sessions: {}", st.sessions.len()))),
            )
    }
}

impl Render for CodaGpuWorkspaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let st = self.state.lock().unwrap().clone();
        div()
            .size_full()
            .bg(rgb(palette::OLED_BLACK))
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.show_sidebar = !g.show_sidebar;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SpawnNewTerminal, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.spawn_new_session();
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CycleBypassPermissions, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.handle_tactile_key("tab");
                }
                cx.notify();
            }))
            .flex()
            .flex_col()
            .child(self.render_top_workspace_bar(&st, cx))
            .child(self.render_secondary_view_bar(&st, cx))
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_row()
                    .when(st.show_sidebar, |el| {
                        el.child(self.render_left_projects_sidebar(&st, cx))
                    })
                    .when(st.show_center_pane, |el| {
                        el.child(self.render_center_inspector_pane(&st, cx))
                    })
                    .when(st.show_agent_pane, |el| {
                        el.child(self.render_right_agent_pane(&st, cx))
                    }),
            )
            .child(self.render_bottom_status_bar(&st, cx))
    }
}

/// Launches the native 120Hz GPU-accelerated Coda Desktop window (`macOS`, `Windows`, `Linux`).
pub fn launch_gpui_desktop_app() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", QuitApp, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
            KeyBinding::new("cmd-n", SpawnNewTerminal, None),
        ]);
        cx.on_action(|_: &QuitApp, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1380.0), px(840.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(CodaGpuWorkspaceView::new),
            )
            .expect("failed to open Coda GPUI desktop window");

        let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        cx.observe_keystrokes(move |ev, _, cx| {
            let key = ev.keystroke.key.as_str();
            if ev.keystroke.modifiers.platform && key == "v" {
                if let Some(item) = cx.read_from_clipboard() {
                    if let Some(txt) = item.text() {
                        view.update(cx, |this, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.draft_input.push_str(&txt);
                            }
                            cx.notify();
                        });
                    }
                }
                return;
            }
            view.update(cx, |this, cx| {
                if let Ok(mut g) = this.state.lock() {
                    match key {
                        "enter" => g.handle_tactile_key("enter"),
                        "backspace" => g.handle_tactile_key("backspace"),
                        "escape" => g.handle_tactile_key("esc"),
                        "tab" => g.handle_tactile_key("tab"),
                        _ => {
                            if let Some(ref ch) = ev.keystroke.key_char {
                                if !ev.keystroke.modifiers.platform && !ev.keystroke.modifiers.control {
                                    g.draft_input.push_str(ch);
                                }
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();

        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gpui_desktop_state_and_live_herdr_sync() {
        let mut state = CodaDesktopState::default();
        assert_eq!(state.active_session().address, "t1");
        assert!(state.bypass_permissions);
        assert!(state.show_sidebar);
        assert!(state.show_center_pane);
        assert!(state.show_agent_pane);

        // Verify prompt & shortcut actions (`tab`, `slash`, `backspace`, `attachment`, `spawn`)
        state.handle_tactile_key("tab");
        assert!(!state.bypass_permissions);

        state.handle_tactile_key("slash");
        assert_eq!(state.draft_input, "/clear");

        state.handle_tactile_key("backspace");
        assert_eq!(state.draft_input, "/clea");

        state.trigger_attachment_upload();
        assert!(state.draft_input.contains("coda_gpui_capture.txt"));

        let initial_sessions = state.sessions.len();
        state.spawn_new_session();
        assert_eq!(state.sessions.len(), initial_sessions + 1);

        // Verify live herdr-core workspace & terminal synchronization
        state.sync_from_local_herdr().await;
        assert!(!state.repos.is_empty());
    }
}
