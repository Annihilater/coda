//! Coda Native GPU-Accelerated Workbench (`Rust + GPUI`)
//!
//! Designed around a focused developer & agent workstation:
//! - Frameless transparent macOS titlebar merged with unified workspace chrome.
//! - Real local Unix `openpty` shells & decoupled `herdr-core` (`hc`) persistent sessions.
//! - TrueColor 24-bit ANSI VT100 rendering in `Menlo` with side-by-side split terminals.
//! - Live `OSC 7501` agent inbox & 4-mode workbench (`Workbench`, `Terminals`, `Code`, `Board`).
//! - Hot-reloadable `~/.config/coda/layout.json` & `~/.config/coda/AGENTS.md`.
//! - Real Git working-tree diff/commits, workspace file browser, and E2EE Cloud Mesh (`coda-daemon`).

pub mod config;
pub mod git_inspector;
pub mod pty;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use coda_daemon::{
    list_account_hosts, login_with_password_and_enroll, mint_peer_relay_config,
    poll_device_login_until_approved, start_device_login_flow, start_embedded_host_if_configured,
    DaemonConfig, RemoteHostSummary, SmartPeerClient,
};
use coda_herdr::{discover_hc_binary, HerdrClient};
use coda_protocol::{PeerMessage, TrafficLane};
use gpui::{
    actions, div, point, prelude::*, px, rgb, rgba, size, App, Application, Bounds, ClipboardItem,
    Context, FocusHandle, Focusable, FontWeight, HighlightStyle, Hsla, KeyBinding, MouseButton,
    MouseUpEvent, ScrollDelta, ScrollWheelEvent, SharedString, StyledText, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};
use uuid::Uuid;

use crate::config::{
    detect_local_machine_name, discover_local_git_repositories, display_compact_path,
    ensure_agents_md_manual, ensure_macos_gui_path, pick_folder_via_native_dialog,
    reveal_in_finder, space_avatar_color, tr, DesktopPreferences, LayoutMode,
    MalleableLayoutConfig, UiLocale,
};
use crate::git_inspector::{
    compute_repo_snapshot, list_workspace_dir, read_workspace_file_preview, RepoInspectorSnapshot,
    WorkspaceFileEntry,
};
use crate::pty::{AgentLifecycleState, LocalPtyManager, Osc7501Status, StyledTerminalLine};

actions!(
    coda_desktop,
    [
        QuitApp,
        ToggleSidebar,
        NewTerminal,
        ToggleSplit,
        ToggleCommandPalette,
        SwitchLayoutWorkbench,
        SwitchLayoutTerminals,
        SwitchLayoutCode,
        SwitchLayoutBoard,
        InterruptActiveTerminal,
    ]
);

/// Herdr-inspired layered dark obsidian color tokens.
pub mod palette {
    pub const CANVAS_BG: u32 = 0x0a0b0e;
    pub const SIDEBAR_BG: u32 = 0x0d0e11;
    pub const CHROME_BG: u32 = 0x111317;
    pub const SURFACE_BG: u32 = 0x14161b;
    pub const HOVER_BG: u32 = 0x1b1e25;
    pub const BORDER: u32 = 0x1e222b;
    pub const BORDER_STRONG: u32 = 0x2b313e;
    pub const TEXT_PRIMARY: u32 = 0xe6edf3;
    pub const TEXT_MUTED: u32 = 0x8b949e;
    pub const TEXT_DIM: u32 = 0x525866;
    pub const NEEDS_YOU_AMBER: u32 = 0xf59e0b;
    pub const WORKING_SKY: u32 = 0x38bdf8;
    pub const DONE_GREEN: u32 = 0x22c55e;
    pub const ERROR_RED: u32 = 0xf87171;
}

/// Active keyboard input routing target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveInputTarget {
    #[default]
    TerminalDirect,
    CommandPalette,
    AuthServerUrl,
    AuthAccount,
    AuthPassword,
}

/// Primary or Secondary split terminal pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitPane {
    #[default]
    Primary,
    Secondary,
}

/// Optional slide-out inspector drawer on the right edge (closed by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightDrawerTab {
    Changes,
    Files,
    Mesh,
}

/// Workspace entry shown in the `Spaces` section of the sidebar.
#[derive(Debug, Clone)]
pub struct SpaceEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub machine_id: String,
    pub machine_label: String,
    pub is_remote: bool,
    pub branch: String,
    pub dirty_files: usize,
    pub ahead: usize,
    pub behind: usize,
    pub avatar_color: u32,
}

/// Live terminal or agent session shown in tabs, the `Agents` inbox, and the Kanban board.
#[derive(Debug, Clone)]
pub struct TerminalSessionEntry {
    pub id: String,
    pub title: String,
    pub space_id: String,
    pub cwd: String,
    pub started_at: Instant,
    pub last_output_at: Instant,
    pub last_byte_sig: usize,
    pub lines: Vec<String>,
    pub styled_lines: Vec<StyledTerminalLine>,
    pub scroll_offset: usize,
    pub alive: bool,
    pub attached_hc: bool,
    pub hc_addr: Option<String>,
    pub remote_host_id: Option<String>,
    pub osc_status: Option<Osc7501Status>,
}

impl TerminalSessionEntry {
    pub fn effective_state(&self) -> AgentLifecycleState {
        if let Some(ref osc) = self.osc_status {
            return osc.state;
        }
        if !self.alive {
            return AgentLifecycleState::Done;
        }
        if self.started_at.elapsed() > Duration::from_secs(1)
            && self.last_output_at.elapsed() < Duration::from_secs(3)
        {
            return AgentLifecycleState::Working;
        }
        AgentLifecycleState::Idle
    }

    pub fn elapsed_label(&self) -> String {
        let secs = self.started_at.elapsed().as_secs();
        if secs < 60 {
            "<1m".to_string()
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h", secs / 3600)
        }
    }

    pub fn preview_snippet(&self) -> String {
        if let Some(ref osc) = self.osc_status {
            if let Some(ref t) = osc.task {
                return t.clone();
            }
        }
        self.lines
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().chars().take(72).collect())
            .unwrap_or_else(|| self.cwd.clone())
    }
}

/// Single command palette entry.
#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub kind_tag: &'static str,
    pub label: String,
    pub detail: String,
    pub action: PaletteAction,
}

#[derive(Debug, Clone)]
pub enum PaletteAction {
    FocusSpace(String),
    FocusSession(String),
    SetLayout(LayoutMode),
    NewTerminal,
    LaunchAgent(&'static str),
    ToggleSplit,
    ToggleSidebar,
    AddSpaceFolder,
    OpenDrawer(RightDrawerTab),
    ToggleLocale,
}

/// Root serializable-free desktop state backing the Coda GPUI workbench.
#[derive(Debug, Clone)]
pub struct CodaDesktopState {
    pub locale: UiLocale,
    pub local_machine_name: String,
    pub layout: MalleableLayoutConfig,
    pub layout_mtime: Option<SystemTime>,

    pub spaces: Vec<SpaceEntry>,
    pub active_space_id: Option<String>,

    pub sessions: Vec<TerminalSessionEntry>,
    pub active_session_id: Option<String>,
    pub split_session_id: Option<String>,
    pub focused_pane: SplitPane,

    pub input_target: ActiveInputTarget,
    pub right_drawer: Option<RightDrawerTab>,
    pub palette_open: bool,
    pub palette_query: String,
    pub palette_selected: usize,

    // Real Git / Files inspector for active space
    pub repo_snapshot: RepoInspectorSnapshot,
    pub dir_entries: Vec<WorkspaceFileEntry>,
    pub selected_file_rel: Option<String>,
    pub selected_file_content: String,
    pub code_show_diff: bool,
    pub code_bottom_terminal: bool,

    // `herdr-core` runtime state
    pub hc_available: bool,
    pub hc_version: Option<String>,
    pub hc_path: Option<PathBuf>,

    // Cloud mesh (`coda-daemon`) auth & remote hosts
    pub server_url: String,
    pub auth_account_input: String,
    pub auth_password_input: String,
    pub is_logged_in: bool,
    pub account_email: String,
    pub remote_hosts: Vec<RemoteHostSummary>,
    pub selected_remote_host_id: Option<String>,
    pub remote_probe_status: Option<String>,
    pub device_login_code: Option<String>,
    pub device_login_uri: Option<String>,
    pub status_notice: Option<String>,
}

impl Default for CodaDesktopState {
    fn default() -> Self {
        Self {
            locale: UiLocale::En,
            local_machine_name: detect_local_machine_name(),
            layout: MalleableLayoutConfig::default(),
            layout_mtime: None,
            spaces: Vec::new(),
            active_space_id: None,
            sessions: Vec::new(),
            active_session_id: None,
            split_session_id: None,
            focused_pane: SplitPane::Primary,
            input_target: ActiveInputTarget::TerminalDirect,
            right_drawer: None,
            palette_open: false,
            palette_query: String::new(),
            palette_selected: 0,
            repo_snapshot: RepoInspectorSnapshot::default(),
            dir_entries: Vec::new(),
            selected_file_rel: None,
            selected_file_content: String::new(),
            code_show_diff: false,
            code_bottom_terminal: true,
            hc_available: false,
            hc_version: None,
            hc_path: None,
            server_url: DesktopPreferences::default().server_url,
            auth_account_input: String::new(),
            auth_password_input: String::new(),
            is_logged_in: false,
            account_email: String::new(),
            remote_hosts: Vec::new(),
            selected_remote_host_id: None,
            remote_probe_status: None,
            device_login_code: None,
            device_login_uri: None,
            status_notice: None,
        }
    }
}

impl CodaDesktopState {
    pub fn active_space(&self) -> Option<&SpaceEntry> {
        let id = self.active_space_id.as_deref()?;
        self.spaces.iter().find(|s| s.id == id)
    }

    pub fn focused_session_id(&self) -> Option<&str> {
        if self.layout.split_terminals && self.focused_pane == SplitPane::Secondary {
            self.split_session_id
                .as_deref()
                .or(self.active_session_id.as_deref())
        } else {
            self.active_session_id.as_deref()
        }
    }

    pub fn session_by_id(&self, id: &str) -> Option<&TerminalSessionEntry> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn palette_items(&self) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        for space in &self.spaces {
            items.push(PaletteItem {
                kind_tag: "SPACE",
                label: space.name.clone(),
                detail: format!("{} · {}", space.branch, display_compact_path(&space.path)),
                action: PaletteAction::FocusSpace(space.id.clone()),
            });
        }
        for sess in &self.sessions {
            let space_name = self
                .spaces
                .iter()
                .find(|s| s.id == sess.space_id)
                .map(|s| s.name.as_str())
                .unwrap_or("local");
            items.push(PaletteItem {
                kind_tag: "TERM",
                label: sess.title.clone(),
                detail: format!("{} · {}", space_name, sess.preview_snippet()),
                action: PaletteAction::FocusSession(sess.id.clone()),
            });
        }
        for mode in [
            LayoutMode::Workbench,
            LayoutMode::Terminals,
            LayoutMode::Code,
            LayoutMode::Board,
        ] {
            items.push(PaletteItem {
                kind_tag: "LAYOUT",
                label: mode.label(self.locale).to_string(),
                detail: format!("Switch layout to {}", mode.as_slug()),
                action: PaletteAction::SetLayout(mode),
            });
        }
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "New Terminal", "新建终端").to_string(),
            detail: "Cmd+N".to_string(),
            action: PaletteAction::NewTerminal,
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Toggle Split Terminals", "切换分屏终端").to_string(),
            detail: "Cmd+D".to_string(),
            action: PaletteAction::ToggleSplit,
        });
        items.push(PaletteItem {
            kind_tag: "AGENT",
            label: tr(self.locale, "Launch Claude Code Agent", "启动 Claude Code Agent").to_string(),
            detail: "agent · claude".to_string(),
            action: PaletteAction::LaunchAgent("claude"),
        });
        items.push(PaletteItem {
            kind_tag: "AGENT",
            label: tr(self.locale, "Launch Codex Agent", "启动 Codex Agent").to_string(),
            detail: "agent · codex".to_string(),
            action: PaletteAction::LaunchAgent("codex"),
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Add Workspace Folder...", "添加本地工作区...").to_string(),
            detail: "Finder picker".to_string(),
            action: PaletteAction::AddSpaceFolder,
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Toggle Sidebar", "显示/隐藏侧边栏").to_string(),
            detail: "Cmd+B".to_string(),
            action: PaletteAction::ToggleSidebar,
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Connect Remote Machine (Mesh)", "连接远端节点 (Mesh)").to_string(),
            detail: "E2EE P2P / WSS".to_string(),
            action: PaletteAction::OpenDrawer(RightDrawerTab::Mesh),
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Switch Language (EN / 中文)", "切换界面语言 (EN / 中文)").to_string(),
            detail: self.locale.label().to_string(),
            action: PaletteAction::ToggleLocale,
        });

        let q = self.palette_query.trim().to_ascii_lowercase();
        if q.is_empty() {
            items
        } else {
            items
                .into_iter()
                .filter(|item| {
                    item.label.to_ascii_lowercase().contains(&q)
                        || item.detail.to_ascii_lowercase().contains(&q)
                        || item.kind_tag.to_ascii_lowercase().contains(&q)
                })
                .collect()
        }
    }
}

/// Primary GPUI workbench view.
pub struct CodaGpuWorkspaceView {
    pub state: Arc<Mutex<CodaDesktopState>>,
    pub pty_mgr: Arc<Mutex<LocalPtyManager>>,
    pub ui_dirty: Arc<AtomicBool>,
    pub focus_handle: FocusHandle,
}

impl Focusable for CodaGpuWorkspaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CodaGpuWorkspaceView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        ensure_macos_gui_path();
        MalleableLayoutConfig::save_default_if_missing();
        ensure_agents_md_manual();

        let prefs = DesktopPreferences::load_or_default();
        let (layout_cfg, layout_mtime) = MalleableLayoutConfig::load_with_mtime();

        let mut init_state = CodaDesktopState::default();
        init_state.locale = prefs.locale;
        init_state.server_url = prefs.server_url.clone();
        init_state.layout = layout_cfg;
        init_state.layout_mtime = layout_mtime;
        init_state.hc_path = discover_hc_binary();
        init_state.hc_available = init_state.hc_path.is_some();

        // Hydrate daemon login identity if configured on disk
        if let Ok(dcfg) = DaemonConfig::load(&DaemonConfig::default_path()) {
            init_state.server_url = dcfg.server_url.clone();
            if dcfg.session_token.is_some() {
                init_state.is_logged_in = true;
                init_state.account_email = dcfg
                    .owner_email
                    .clone()
                    .unwrap_or_else(|| dcfg.owner_handle.clone());
            }
        }

        // Discover real local repositories + saved user folders, falling back to `$HOME` if none exist.
        let mut candidates: Vec<PathBuf> = discover_local_git_repositories(&prefs.workspace_roots)
            .into_iter()
            .map(PathBuf::from)
            .collect();
        if candidates.is_empty() {
            if let Ok(home) = std::env::var("HOME") {
                candidates.push(PathBuf::from(home));
            } else if let Ok(cwd) = std::env::current_dir() {
                candidates.push(cwd);
            }
        }

        let machine_label = init_state.local_machine_name.clone();
        for path in candidates {
            let snap = compute_repo_snapshot(&path, None);
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("workspace")
                .to_string();
            let path_str = path.to_string_lossy().to_string();
            init_state.spaces.push(SpaceEntry {
                id: format!("local:{}", path_str),
                avatar_color: space_avatar_color(&name),
                name,
                path: path_str,
                machine_id: "local".to_string(),
                machine_label: machine_label.clone(),
                is_remote: false,
                branch: snap.branch,
                dirty_files: snap.changed_files.len(),
                ahead: 0,
                behind: 0,
            });
        }
        init_state.active_space_id = init_state.spaces.first().map(|s| s.id.clone());

        let state = Arc::new(Mutex::new(init_state));
        let ui_dirty = Arc::new(AtomicBool::new(false));
        let pty_mgr = Arc::new(Mutex::new(LocalPtyManager::new(Arc::clone(&ui_dirty))));

        let view = Self {
            state: Arc::clone(&state),
            pty_mgr: Arc::clone(&pty_mgr),
            ui_dirty: Arc::clone(&ui_dirty),
            focus_handle: cx.focus_handle(),
        };

        // Immediately spawn a real interactive shell in the active workspace + inspect git/files.
        view.refresh_active_space_inspector();
        view.spawn_terminal_in_active_space(None);

        // Background probes: `herdr-core` persistent sessions & `coda-daemon` mesh hosts.
        spawn_herdr_discovery(Arc::clone(&state), Arc::clone(&pty_mgr), Arc::clone(&ui_dirty));
        spawn_refresh_hosts(Arc::clone(&state), Arc::clone(&ui_dirty));

        // Embedded P2P host listener when device credentials are present.
        std::thread::spawn(|| {
            if let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async {
                    let _ = start_embedded_host_if_configured(&DaemonConfig::default_path()).await;
                });
            }
        });

        // 50Hz UI pump + 1s hot-reload watcher for `~/.config/coda/layout.json`.
        let ui_dirty_pump = Arc::clone(&ui_dirty);
        cx.spawn(async move |this, cx| {
            let mut ticks: u32 = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                ticks = ticks.wrapping_add(1);
                let dirty = ui_dirty_pump.swap(false, Ordering::AcqRel);
                let check_layout = ticks % 50 == 0;
                if dirty || check_layout {
                    let updated = this.update(cx, |view, cx| {
                        let mut changed = false;
                        if dirty {
                            view.sync_pty_snapshots_into_state();
                            changed = true;
                        }
                        if check_layout && view.poll_layout_json_hot_reload() {
                            changed = true;
                        }
                        if changed {
                            cx.notify();
                        }
                    });
                    if updated.is_err() {
                        break;
                    }
                }
            }
        })
        .detach();

        view
    }

    fn poll_layout_json_hot_reload(&self) -> bool {
        let Some(path) = MalleableLayoutConfig::path() else {
            return false;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return false;
        };
        let Ok(mtime) = meta.modified() else {
            return false;
        };
        let Ok(mut g) = self.state.lock() else {
            return false;
        };
        if g.layout_mtime == Some(mtime) {
            return false;
        }
        let (cfg, _) = MalleableLayoutConfig::load_with_mtime();
        g.layout = cfg;
        g.layout_mtime = Some(mtime);
        true
    }

    fn persist_layout_config(&self) {
        if let Ok(mut g) = self.state.lock() {
            let _ = g.layout.save();
            g.layout_mtime = MalleableLayoutConfig::load_with_mtime().1;
        }
    }

    fn sync_pty_snapshots_into_state(&self) {
        let snaps = match self.pty_mgr.lock() {
            Ok(mgr) => mgr.snapshot_all(250),
            Err(_) => return,
        };
        let Ok(mut g) = self.state.lock() else {
            return;
        };
        for sess in &mut g.sessions {
            if let Some(snap) = snaps.get(&sess.id) {
                let sig = snap
                    .lines
                    .iter()
                    .map(|l| l.len())
                    .sum::<usize>()
                    .wrapping_add(snap.lines.len() * 131);
                if sig != sess.last_byte_sig {
                    sess.last_output_at = Instant::now();
                    sess.last_byte_sig = sig;
                }
                sess.lines = snap.lines.clone();
                sess.styled_lines = snap.styled_lines.clone();
                sess.alive = snap.alive;
                sess.attached_hc = snap.attached_hc;
                if let Some(ref osc) = snap.osc_status {
                    sess.osc_status = Some(osc.clone());
                }
            }
        }
    }

    fn refresh_active_space_inspector(&self) {
        let (space_id, space_path) = {
            let Ok(g) = self.state.lock() else {
                return;
            };
            let Some(space) = g.active_space() else {
                return;
            };
            if space.is_remote {
                return;
            }
            (space.id.clone(), PathBuf::from(&space.path))
        };

        let snap = compute_repo_snapshot(&space_path, None);
        let entries = list_workspace_dir(&space_path, &space_path);

        if let Ok(mut g) = self.state.lock() {
            if let Some(sp) = g.spaces.iter_mut().find(|s| s.id == space_id) {
                sp.branch = snap.branch.clone();
                sp.dirty_files = snap.changed_files.len();
                sp.ahead = 0;
                sp.behind = 0;
            }
            g.repo_snapshot = snap;
            g.dir_entries = entries;
            if let Some(ref rel) = g.selected_file_rel {
                let full = space_path.join(rel);
                if full.is_file() {
                    g.selected_file_content = read_workspace_file_preview(&full, 120_000);
                }
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn spawn_terminal_in_active_space(&self, command: Option<&str>) {
        let (space_id, space_name, cwd) = {
            let Ok(g) = self.state.lock() else {
                return;
            };
            let Some(sp) = g.active_space() else {
                return;
            };
            (sp.id.clone(), sp.name.clone(), PathBuf::from(&sp.path))
        };

        let shell_bin = std::env::var("SHELL")
            .ok()
            .and_then(|s| Path::new(&s).file_name().map(|f| f.to_string_lossy().to_string()))
            .unwrap_or_else(|| "zsh".to_string());
        let id = format!("pty-{}", &Uuid::new_v4().simple().to_string()[..8]);

        let spawned = match self.pty_mgr.lock() {
            Ok(mut mgr) => mgr.spawn_session(&id, &cwd, command, 120, 36),
            Err(e) => Err(e.to_string()),
        };

        if let Err(err) = spawned {
            if let Ok(mut g) = self.state.lock() {
                g.status_notice = Some(err);
            }
            return;
        }

        if let Ok(mut g) = self.state.lock() {
            let count = g.sessions.iter().filter(|s| s.space_id == space_id).count() + 1;
            let title = if count == 1 {
                format!("{space_name} · {shell_bin}")
            } else {
                format!("{shell_bin} {count}")
            };
            g.sessions.push(TerminalSessionEntry {
                id: id.clone(),
                title,
                space_id,
                cwd: cwd.to_string_lossy().to_string(),
                started_at: Instant::now(),
                last_output_at: Instant::now(),
                last_byte_sig: 0,
                lines: Vec::new(),
                styled_lines: Vec::new(),
                scroll_offset: 0,
                alive: true,
                attached_hc: false,
                hc_addr: None,
                remote_host_id: None,
                osc_status: None,
            });
            if g.layout.split_terminals && g.focused_pane == SplitPane::Secondary {
                g.split_session_id = Some(id);
            } else {
                g.active_session_id = Some(id);
            }
            g.input_target = ActiveInputTarget::TerminalDirect;
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn launch_agent_session(&self, agent_cmd: &str) {
        let (space_id, space_name, cwd, hc_path) = {
            let Ok(g) = self.state.lock() else {
                return;
            };
            let Some(sp) = g.active_space() else {
                return;
            };
            (
                sp.id.clone(),
                sp.name.clone(),
                PathBuf::from(&sp.path),
                g.hc_path.clone(),
            )
        };

        if let Some(hc_bin) = hc_path {
            let state_bg = Arc::clone(&self.state);
            let pty_mgr_bg = Arc::clone(&self.pty_mgr);
            let ui_dirty = Arc::clone(&self.ui_dirty);
            let agent = agent_cmd.to_string();
            let cwd_str = cwd.to_string_lossy().to_string();
            std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                rt.block_on(async move {
                    if let Ok(hc) = HerdrClient::connect_with_bin(hc_bin.clone()).await {
                        if let Ok(term) = hc
                            .launch_agent(&agent, Some(&agent), Some(&cwd_str), None, 120, 36)
                            .await
                        {
                            let id = format!("hc-{}", term.address);
                            let attached = pty_mgr_bg
                                .lock()
                                .map(|mut mgr| {
                                    mgr.spawn_herdr_attach(
                                        &id,
                                        &cwd,
                                        &hc_bin,
                                        &term.address,
                                        120,
                                        36,
                                    )
                                    .is_ok()
                                })
                                .unwrap_or(false);
                            if attached {
                                if let Ok(mut g) = state_bg.lock() {
                                    g.sessions.push(TerminalSessionEntry {
                                        id: id.clone(),
                                        title: format!("{space_name} · {agent}"),
                                        space_id,
                                        cwd: cwd_str,
                                        started_at: Instant::now(),
                                        last_output_at: Instant::now(),
                                        last_byte_sig: 0,
                                        lines: Vec::new(),
                                        styled_lines: Vec::new(),
                                        scroll_offset: 0,
                                        alive: true,
                                        attached_hc: true,
                                        hc_addr: Some(term.address),
                                        remote_host_id: None,
                                        osc_status: None,
                                    });
                                    g.active_session_id = Some(id);
                                    g.input_target = ActiveInputTarget::TerminalDirect;
                                }
                                ui_dirty.store(true, Ordering::Release);
                                return;
                            }
                        }
                    }
                });
            });
            return;
        }

        // Local PTY fallback: spawn interactive login shell and invoke agent CLI directly
        let before_id = self
            .state
            .lock()
            .ok()
            .and_then(|g| g.active_session_id.clone());
        self.spawn_terminal_in_active_space(Some(agent_cmd));
        let mut renamed_id = None;
        if let Ok(mut g) = self.state.lock() {
            if g.active_session_id != before_id {
                if let Some(id) = g.active_session_id.clone() {
                    if let Some(sess) = g.sessions.iter_mut().find(|s| s.id == id) {
                        sess.title = format!("{space_name} · {agent_cmd}");
                    }
                    renamed_id = Some(id);
                }
            }
        }
        let _ = renamed_id;
    }

    fn select_session(&self, session_id: &str) {
        let Ok(mut g) = self.state.lock() else {
            return;
        };
        let Some(space_id) = g
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.space_id.clone())
        else {
            return;
        };
        g.active_space_id = Some(space_id);
        if g.layout.split_terminals && g.focused_pane == SplitPane::Secondary {
            g.split_session_id = Some(session_id.to_string());
        } else {
            g.active_session_id = Some(session_id.to_string());
        }
        if g.layout.layout == LayoutMode::Board {
            g.layout.layout = LayoutMode::Workbench;
        }
        g.input_target = ActiveInputTarget::TerminalDirect;
        drop(g);
        self.refresh_active_space_inspector();
    }

    fn select_space(&self, space_id: &str) {
        let mut need_spawn = false;
        if let Ok(mut g) = self.state.lock() {
            g.active_space_id = Some(space_id.to_string());
            g.selected_file_rel = None;
            g.selected_file_content.clear();
            let first_sess = g
                .sessions
                .iter()
                .find(|s| s.space_id == space_id)
                .map(|s| s.id.clone());
            if let Some(sid) = first_sess {
                g.active_session_id = Some(sid);
            } else if let Some(sp) = g.spaces.iter().find(|s| s.id == space_id) {
                need_spawn = !sp.is_remote;
            }
            g.input_target = ActiveInputTarget::TerminalDirect;
        }
        self.refresh_active_space_inspector();
        if need_spawn {
            self.spawn_terminal_in_active_space(None);
        }
    }

    fn toggle_split_terminals(&self) {
        let mut need_second = false;
        if let Ok(mut g) = self.state.lock() {
            g.layout.split_terminals = !g.layout.split_terminals;
            if g.layout.split_terminals && g.split_session_id.is_none() {
                let primary_id = g.active_session_id.clone();
                let other = g
                    .sessions
                    .iter()
                    .find(|s| Some(&s.id) != primary_id.as_ref())
                    .map(|s| s.id.clone());
                if other.is_some() {
                    g.split_session_id = other;
                } else {
                    g.focused_pane = SplitPane::Secondary;
                    need_second = true;
                }
            } else if !g.layout.split_terminals {
                g.focused_pane = SplitPane::Primary;
            }
        }
        if need_second {
            self.spawn_terminal_in_active_space(None);
        }
        self.persist_layout_config();
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn close_session(&self, session_id: &str) {
        if let Ok(mut mgr) = self.pty_mgr.lock() {
            mgr.kill_session(session_id);
        }
        if let Ok(mut g) = self.state.lock() {
            g.sessions.retain(|s| s.id != session_id);
            if g.active_session_id.as_deref() == Some(session_id) {
                g.active_session_id = g.sessions.first().map(|s| s.id.clone());
            }
            if g.split_session_id.as_deref() == Some(session_id) {
                g.split_session_id = g
                    .sessions
                    .iter()
                    .find(|s| Some(&s.id) != g.active_session_id.as_ref())
                    .map(|s| s.id.clone());
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn add_workspace_folder(&self) {
        let Some(path_str) = pick_folder_via_native_dialog() else {
            return;
        };
        let folder = PathBuf::from(&path_str);
        let snap = compute_repo_snapshot(&folder, None);
        let name = folder
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace")
            .to_string();
        let space_id = format!("local:{path_str}");

        if let Ok(mut g) = self.state.lock() {
            let machine_label = g.local_machine_name.clone();
            if !g.spaces.iter().any(|s| s.id == space_id) {
                g.spaces.push(SpaceEntry {
                    id: space_id.clone(),
                    avatar_color: space_avatar_color(&name),
                    name,
                    path: path_str.clone(),
                    machine_id: "local".to_string(),
                    machine_label,
                    is_remote: false,
                    branch: snap.branch,
                    dirty_files: snap.changed_files.len(),
                    ahead: 0,
                    behind: 0,
                });
            }
            let mut prefs = DesktopPreferences::load_or_default();
            if !prefs.workspace_roots.contains(&path_str) {
                prefs.workspace_roots.push(path_str);
                let _ = prefs.save();
            }
        }
        self.select_space(&space_id);
    }

    fn execute_palette_action(&self, action: PaletteAction) {
        match action {
            PaletteAction::FocusSpace(id) => self.select_space(&id),
            PaletteAction::FocusSession(id) => self.select_session(&id),
            PaletteAction::SetLayout(mode) => {
                if let Ok(mut g) = self.state.lock() {
                    g.layout.layout = mode;
                    if mode == LayoutMode::Terminals {
                        g.layout.sidebar_visible = false;
                    }
                }
                self.persist_layout_config();
            }
            PaletteAction::NewTerminal => self.spawn_terminal_in_active_space(None),
            PaletteAction::LaunchAgent(agent_cmd) => self.launch_agent_session(agent_cmd),
            PaletteAction::ToggleSplit => self.toggle_split_terminals(),
            PaletteAction::ToggleSidebar => {
                if let Ok(mut g) = self.state.lock() {
                    g.layout.sidebar_visible = !g.layout.sidebar_visible;
                }
                self.persist_layout_config();
            }
            PaletteAction::AddSpaceFolder => self.add_workspace_folder(),
            PaletteAction::OpenDrawer(tab) => {
                if let Ok(mut g) = self.state.lock() {
                    g.right_drawer = Some(tab);
                }
            }
            PaletteAction::ToggleLocale => self.toggle_locale(),
        }
        if let Ok(mut g) = self.state.lock() {
            g.palette_open = false;
            g.palette_query.clear();
            g.palette_selected = 0;
            g.input_target = ActiveInputTarget::TerminalDirect;
        }
    }

    fn toggle_locale(&self) {
        if let Ok(mut g) = self.state.lock() {
            g.locale = g.locale.toggle();
            let mut prefs = DesktopPreferences::load_or_default();
            prefs.locale = g.locale;
            let _ = prefs.save();
        }
        self.ui_dirty.store(true, Ordering::Release);
    }
}

// ============================================================================
// GPUI Render Implementation
// ============================================================================

impl Render for CodaGpuWorkspaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let st = match self.state.lock() {
            Ok(g) => g.clone(),
            Err(_) => CodaDesktopState::default(),
        };

        let main_area = match st.layout.layout {
            LayoutMode::Workbench => self.render_workbench_panes(&st, cx).into_any_element(),
            LayoutMode::Terminals => self.render_terminals_grid(&st, cx).into_any_element(),
            LayoutMode::Code => self.render_code_workspace(&st, cx).into_any_element(),
            LayoutMode::Board => self.render_board_kanban(&st, cx).into_any_element(),
        };

        div()
            .id("coda-root")
            .key_context("CodaWorkbench")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.layout.sidebar_visible = !g.layout.sidebar_visible;
                }
                this.persist_layout_config();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, _, cx| {
                this.spawn_terminal_in_active_space(None);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleSplit, _, cx| {
                this.toggle_split_terminals();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.palette_open = !g.palette_open;
                    if g.palette_open {
                        g.palette_query.clear();
                        g.palette_selected = 0;
                        g.input_target = ActiveInputTarget::CommandPalette;
                    } else {
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SwitchLayoutWorkbench, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.layout.layout = LayoutMode::Workbench;
                }
                this.persist_layout_config();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SwitchLayoutTerminals, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.layout.layout = LayoutMode::Terminals;
                }
                this.persist_layout_config();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SwitchLayoutCode, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.layout.layout = LayoutMode::Code;
                }
                this.persist_layout_config();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SwitchLayoutBoard, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.layout.layout = LayoutMode::Board;
                }
                this.persist_layout_config();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &InterruptActiveTerminal, _, cx| {
                let target_id = this
                    .state
                    .lock()
                    .ok()
                    .and_then(|g| g.focused_session_id().map(|s| s.to_string()));
                if let Some(id) = target_id {
                    if let Ok(mgr) = this.pty_mgr.lock() {
                        let _ = mgr.send_sigint(&id);
                    }
                }
                cx.notify();
            }))
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .text_color(rgb(palette::TEXT_PRIMARY))
            .flex()
            .flex_row()
            .overflow_hidden()
            .relative()
            .when(st.layout.sidebar_visible, |el| {
                el.child(self.render_left_sidebar(&st, cx))
            })
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(self.render_unified_topbar(&st, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_row()
                            .overflow_hidden()
                            .child(div().flex_1().h_full().min_w_0().child(main_area))
                            .when_some(st.right_drawer, |el, drawer| {
                                el.child(self.render_right_drawer(&st, drawer, cx))
                            }),
                    ),
            )
            .when(st.palette_open, |el| {
                el.child(self.render_command_palette_overlay(&st, cx))
            })
    }
}

impl CodaGpuWorkspaceView {
    fn render_left_sidebar(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let width = st.layout.sidebar_width_px.clamp(200.0, 320.0);
        let accent = st.layout.accent_color_u32();

        // Group Spaces by machine_label preserving discovery order
        let mut machine_groups: Vec<(String, bool, Vec<&SpaceEntry>)> = Vec::new();
        for sp in &st.spaces {
            if let Some(grp) = machine_groups.iter_mut().find(|g| g.0 == sp.machine_label) {
                grp.2.push(sp);
            } else {
                machine_groups.push((sp.machine_label.clone(), sp.is_remote, vec![sp]));
            }
        }

        div()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(palette::SIDEBAR_BG))
            .border_r_1()
            .border_color(rgb(palette::BORDER))
            .flex()
            .flex_col()
            .child(
                // Integrated macOS traffic-light header strip (38px)
                div()
                    .h(px(38.0))
                    .flex_shrink_0()
                    .px(px(10.0))
                    .when(cfg!(target_os = "macos"), |el| el.pl(px(78.0)))
                    .border_b_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().w(px(7.0)).h(px(7.0)).rounded(px(99.0)).bg(rgb(accent)))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child("Coda"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id("btn-open-palette")
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .cursor_pointer()
                                    .child("⌘P")
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.palette_open = true;
                                                g.palette_query.clear();
                                                g.palette_selected = 0;
                                                g.input_target = ActiveInputTarget::CommandPalette;
                                            }
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("btn-collapse-sidebar")
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .cursor_pointer()
                                    .child("◧")
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.layout.sidebar_visible = false;
                                            }
                                            this.persist_layout_config();
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(
                // Scrollable Spaces + Agents sidebar body
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(8.0))
                    .py(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(
                        // SPACES SECTION
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .px(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(tr(loc, "SPACES", "工作区")),
                                    )
                                    .child(
                                        div()
                                            .id("btn-add-space")
                                            .px(px(5.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .text_size(px(12.0))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .cursor_pointer()
                                            .child("+")
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                    this.add_workspace_folder();
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            )
                            .children(machine_groups.into_iter().map(|(machine, is_remote, list)| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .px(px(6.0))
                                            .pt(px(4.0))
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .truncate()
                                            .child(if is_remote {
                                                machine
                                            } else {
                                                format!("{} · {machine}", tr(loc, "This Mac", "本机"))
                                            }),
                                    )
                                    .children(list.into_iter().map(|sp| {
                                        let is_active =
                                            st.active_space_id.as_deref() == Some(&sp.id);
                                        let sp_id = sp.id.clone();
                                        let initial = sp
                                            .name
                                            .chars()
                                            .next()
                                            .unwrap_or('C')
                                            .to_ascii_uppercase()
                                            .to_string();
                                        let has_dirty = sp.dirty_files > 0;
                                        div()
                                            .id(SharedString::from(format!("space-{}", sp.id)))
                                            .px(px(6.0))
                                            .py(px(5.0))
                                            .rounded(px(6.0))
                                            .bg(rgb(if is_active {
                                                palette::HOVER_BG
                                            } else {
                                                palette::SIDEBAR_BG
                                            }))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(7.0))
                                                    .child(
                                                        div()
                                                            .w(px(18.0))
                                                            .h(px(18.0))
                                                            .rounded(px(5.0))
                                                            .bg(rgba((sp.avatar_color << 8) | 0x26))
                                                            .border_1()
                                                            .border_color(rgba(
                                                                (sp.avatar_color << 8) | 0x66,
                                                            ))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_size(px(10.0))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(sp.avatar_color))
                                                            .child(initial),
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .truncate()
                                                            .text_size(px(12.5))
                                                            .text_color(rgb(if is_active {
                                                                palette::TEXT_PRIMARY
                                                            } else {
                                                                palette::TEXT_MUTED
                                                            }))
                                                            .child(sp.name.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(5.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(10.5))
                                                            .text_color(rgb(palette::TEXT_DIM))
                                                            .child(sp.branch.clone()),
                                                    )
                                                    .when(has_dirty, |el| {
                                                        el.child(
                                                            div()
                                                                .w(px(5.0))
                                                                .h(px(5.0))
                                                                .rounded(px(99.0))
                                                                .bg(rgb(palette::NEEDS_YOU_AMBER)),
                                                        )
                                                    }),
                                            )
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                    this.select_space(&sp_id);
                                                    cx.notify();
                                                }),
                                            )
                                    }))
                            })),
                    )
                    .child(
                        // AGENTS INBOX SECTION
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .px(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(tr(loc, "AGENTS", "会话收件箱")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(format!("{}", st.sessions.len())),
                                    ),
                            )
                            .children(st.sessions.iter().map(|sess| {
                                let is_active =
                                    st.active_session_id.as_deref() == Some(&sess.id)
                                        || st.split_session_id.as_deref() == Some(&sess.id);
                                let state_color = sess.effective_state().color_u32();
                                let sid = sess.id.clone();
                                let sp_name = st
                                    .spaces
                                    .iter()
                                    .find(|s| s.id == sess.space_id)
                                    .map(|s| s.name.as_str())
                                    .unwrap_or("local")
                                    .to_string();

                                div()
                                    .id(SharedString::from(format!("agent-row-{}", sess.id)))
                                    .px(px(7.0))
                                    .py(px(6.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(if is_active {
                                        palette::SURFACE_BG
                                    } else {
                                        palette::SIDEBAR_BG
                                    }))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .child(
                                                        div()
                                                            .w(px(6.0))
                                                            .h(px(6.0))
                                                            .rounded(px(99.0))
                                                            .bg(rgb(state_color)),
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .truncate()
                                                            .text_size(px(12.0))
                                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                                            .child(sess.title.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(palette::TEXT_DIM))
                                                    .child(sess.elapsed_label()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .pl(px(12.0))
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .truncate()
                                            .child(format!(
                                                "{sp_name} · {}",
                                                sess.preview_snippet()
                                            )),
                                    )
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                            this.select_session(&sid);
                                            cx.notify();
                                        }),
                                    )
                            })),
                    ),
            )
            .child(
                // Sidebar footer: + Add machine + EN / 中文
                div()
                    .h(px(34.0))
                    .flex_shrink_0()
                    .px(px(8.0))
                    .border_t_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("btn-add-machine")
                            .px(px(6.0))
                            .py(px(3.0))
                            .rounded(px(5.0))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_size(px(11.5))
                            .text_color(rgb(if st.is_logged_in {
                                palette::DONE_GREEN
                            } else {
                                palette::TEXT_MUTED
                            }))
                            .child(if st.is_logged_in {
                                format!("● {}", st.account_email)
                            } else {
                                tr(loc, "+ Add machine", "+ 添加远端主机").to_string()
                            })
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.right_drawer = match g.right_drawer {
                                            Some(RightDrawerTab::Mesh) => None,
                                            _ => Some(RightDrawerTab::Mesh),
                                        };
                                    }
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("btn-toggle-locale")
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(loc.label())
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    this.toggle_locale();
                                    cx.notify();
                                }),
                            ),
                    ),
            )
    }

    fn render_unified_topbar(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let active_space = st.active_space();
        let space_sessions: Vec<&TerminalSessionEntry> = match active_space {
            Some(sp) => st
                .sessions
                .iter()
                .filter(|s| s.space_id == sp.id)
                .collect(),
            None => st.sessions.iter().collect(),
        };

        div()
            .h(px(38.0))
            .w_full()
            .flex_shrink_0()
            .bg(rgb(palette::CHROME_BG))
            .border_b_1()
            .border_color(rgb(palette::BORDER))
            .px(px(10.0))
            .when(!st.layout.sidebar_visible && cfg!(target_os = "macos"), |el| {
                el.pl(px(78.0))
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                // Left side: expand sidebar button (if hidden) + session tabs + '+'
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .when(!st.layout.sidebar_visible, |el| {
                        el.child(
                            div()
                                .id("btn-show-sidebar")
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                .text_size(px(11.5))
                                .text_color(rgb(palette::TEXT_MUTED))
                                .cursor_pointer()
                                .child("◧")
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.layout.sidebar_visible = true;
                                        }
                                        this.persist_layout_config();
                                        cx.notify();
                                    }),
                                ),
                        )
                    })
                    .children(space_sessions.into_iter().map(|sess| {
                        let is_selected = st.active_session_id.as_deref() == Some(&sess.id)
                            || (st.layout.split_terminals
                                && st.split_session_id.as_deref() == Some(&sess.id));
                        let dot_color = sess.effective_state().color_u32();
                        let sid = sess.id.clone();
                        let sid_close = sess.id.clone();

                        div()
                            .id(SharedString::from(format!("tab-{}", sess.id)))
                            .h(px(26.0))
                            .px(px(9.0))
                            .rounded(px(5.0))
                            .bg(rgb(if is_selected {
                                palette::HOVER_BG
                            } else {
                                palette::CHROME_BG
                            }))
                            .border_1()
                            .border_color(rgb(if is_selected {
                                palette::BORDER_STRONG
                            } else {
                                palette::CHROME_BG
                            }))
                            .hover(|h| h.bg(rgb(palette::SURFACE_BG)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded(px(99.0))
                                    .bg(rgb(dot_color)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(if is_selected {
                                        palette::TEXT_PRIMARY
                                    } else {
                                        palette::TEXT_MUTED
                                    }))
                                    .child(sess.title.clone()),
                            )
                            .when(sess.attached_hc, |el| {
                                el.child(
                                    div()
                                        .text_size(px(9.5))
                                        .text_color(rgb(accent))
                                        .child("hc"),
                                )
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("tab-close-{}", sess.id)))
                                    .px(px(2.0))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .hover(|h| h.text_color(rgb(palette::TEXT_PRIMARY)))
                                    .child("×")
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.close_session(&sid_close);
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                    this.select_session(&sid);
                                    cx.notify();
                                }),
                            )
                    }))
                    .child(
                        div()
                            .id("btn-new-terminal")
                            .h(px(24.0))
                            .px(px(7.0))
                            .rounded(px(5.0))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(13.0))
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child("+")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    this.spawn_terminal_in_active_space(None);
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("btn-launch-agent")
                            .h(px(22.0))
                            .px(px(7.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(10.5))
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(tr(loc, "+ Agent", "+ 智能体"))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.palette_open = true;
                                        g.palette_query = "agent".to_string();
                                        g.palette_selected = 0;
                                        g.input_target = ActiveInputTarget::CommandPalette;
                                    }
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                // Right side: 4 Layout Switcher + Split + Inspector Drawer triggers
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .p(px(2.0))
                            .rounded(px(6.0))
                            .bg(rgb(palette::SIDEBAR_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .children(
                                [
                                    LayoutMode::Workbench,
                                    LayoutMode::Terminals,
                                    LayoutMode::Code,
                                    LayoutMode::Board,
                                ]
                                .into_iter()
                                .map(|mode| {
                                    let active = st.layout.layout == mode;
                                    div()
                                        .id(SharedString::from(format!(
                                            "mode-{}",
                                            mode.as_slug()
                                        )))
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(if active {
                                            palette::HOVER_BG
                                        } else {
                                            palette::SIDEBAR_BG
                                        }))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(if active {
                                            palette::TEXT_PRIMARY
                                        } else {
                                            palette::TEXT_MUTED
                                        }))
                                        .child(mode.label(loc))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.layout.layout = mode;
                                                }
                                                this.persist_layout_config();
                                                cx.notify();
                                            }),
                                        )
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("btn-split-toggle")
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(5.0))
                            .bg(rgb(if st.layout.split_terminals {
                                palette::HOVER_BG
                            } else {
                                palette::SURFACE_BG
                            }))
                            .border_1()
                            .border_color(rgb(if st.layout.split_terminals {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if st.layout.split_terminals {
                                accent
                            } else {
                                palette::TEXT_MUTED
                            }))
                            .child(tr(loc, "Split ◫", "分屏 ◫"))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    this.toggle_split_terminals();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(self.render_drawer_toggle_btn(
                        st,
                        RightDrawerTab::Files,
                        tr(loc, "Files", "文件"),
                        cx,
                    ))
                    .child(self.render_drawer_toggle_btn(
                        st,
                        RightDrawerTab::Changes,
                        &format!(
                            "{} ({})",
                            tr(loc, "Changes", "变更"),
                            st.repo_snapshot.changed_files.len()
                        ),
                        cx,
                    )),
            )
    }

    fn render_drawer_toggle_btn(
        &self,
        st: &CodaDesktopState,
        tab: RightDrawerTab,
        label: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = st.right_drawer == Some(tab);
        let accent = st.layout.accent_color_u32();
        div()
            .id(SharedString::from(format!("drawer-btn-{tab:?}")))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .bg(rgb(if active {
                palette::HOVER_BG
            } else {
                palette::SURFACE_BG
            }))
            .border_1()
            .border_color(rgb(if active { accent } else { palette::BORDER }))
            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(rgb(if active {
                palette::TEXT_PRIMARY
            } else {
                palette::TEXT_MUTED
            }))
            .child(label.to_string())
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    if let Ok(mut g) = this.state.lock() {
                        g.right_drawer = if g.right_drawer == Some(tab) {
                            None
                        } else {
                            Some(tab)
                        };
                    }
                    cx.notify();
                }),
            )
    }

    fn render_workbench_panes(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let primary_id = st.active_session_id.as_deref();
        let secondary_id = st.split_session_id.as_deref();

        div()
            .size_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .child(self.render_terminal_pane(st, primary_id, SplitPane::Primary, 42, cx)),
            )
            .when(st.layout.split_terminals, |el| {
                el.child(div().w(px(1.0)).h_full().bg(rgb(palette::BORDER)))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .child(self.render_terminal_pane(
                                st,
                                secondary_id,
                                SplitPane::Secondary,
                                42,
                                cx,
                            )),
                    )
            })
    }

    fn render_terminal_pane(
        &self,
        st: &CodaDesktopState,
        session_id: Option<&str>,
        pane_slot: SplitPane,
        visible_rows: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let is_focused = !st.layout.split_terminals || st.focused_pane == pane_slot;
        let sess = session_id.and_then(|id| st.session_by_id(id));
        let font_family: SharedString = st.layout.font_family.clone().into();
        let font_size = px(st.layout.terminal_font_size_px.clamp(10.0, 22.0));

        let Some(sess) = sess else {
            return div()
                .size_full()
                .bg(rgb(palette::CANVAS_BG))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(palette::TEXT_MUTED))
                        .child(tr(loc, "No active terminal in this pane", "当前分屏暂无终端")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("empty-spawn-{pane_slot:?}")))
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(palette::SURFACE_BG))
                        .border_1()
                        .border_color(rgb(palette::BORDER_STRONG))
                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(rgb(palette::TEXT_PRIMARY))
                        .child(tr(loc, "+ Start Shell", "+ 启动终端"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.focused_pane = pane_slot;
                                }
                                this.spawn_terminal_in_active_space(None);
                                cx.notify();
                            }),
                        ),
                )
                .into_any_element();
        };

        let sid_clear = sess.id.clone();
        let sid_clear_scroll = sess.id.clone();
        let sid_sigint = sess.id.clone();
        let sid_focus = sess.id.clone();
        let sid_scroll = sess.id.clone();
        let sid_jump_bottom = sess.id.clone();
        let branch_label = st
            .spaces
            .iter()
            .find(|s| s.id == sess.space_id)
            .map(|s| s.branch.as_str())
            .unwrap_or("main")
            .to_string();
        let osc_badge = sess.osc_status.as_ref().and_then(|o| o.task.clone());

        let total_lines = sess.styled_lines.len();
        let max_off = total_lines.saturating_sub(visible_rows);
        let eff_off = sess.scroll_offset.min(max_off);
        let end = total_lines.saturating_sub(eff_off);
        let start = end.saturating_sub(visible_rows);
        let visible_slice = &sess.styled_lines[start..end];

        div()
            .id(SharedString::from(format!("term-pane-{pane_slot:?}-{}", sess.id)))
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _, cx| {
                let step: isize = match ev.delta {
                    ScrollDelta::Lines(pt) => {
                        if pt.y > 0.1 {
                            3
                        } else if pt.y < -0.1 {
                            -3
                        } else {
                            0
                        }
                    }
                    ScrollDelta::Pixels(pt) => {
                        if pt.y > px(2.0) {
                            2
                        } else if pt.y < px(-2.0) {
                            -2
                        } else {
                            0
                        }
                    }
                };
                if step == 0 {
                    return;
                }
                if let Ok(mut g) = this.state.lock() {
                    if let Some(s) = g.sessions.iter_mut().find(|s| s.id == sid_scroll) {
                        if step > 0 {
                            s.scroll_offset = (s.scroll_offset + step as usize).min(200);
                        } else {
                            s.scroll_offset = s.scroll_offset.saturating_sub((-step) as usize);
                        }
                    }
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    if let Ok(mut g) = this.state.lock() {
                        g.focused_pane = pane_slot;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                        if pane_slot == SplitPane::Secondary {
                            g.split_session_id = Some(sid_focus.clone());
                        } else {
                            g.active_session_id = Some(sid_focus.clone());
                        }
                    }
                    cx.notify();
                }),
            )
            .child(
                // Terminal ANSI viewport (top-aligned, newest lines when at bottom)
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .px(px(14.0))
                    .py(px(10.0))
                    .overflow_hidden()
                    .font_family(font_family)
                    .text_size(font_size)
                    .flex()
                    .flex_col()
                    .gap(px(1.5))
                    .children(visible_slice.iter().map(render_styled_terminal_line)),
            )
            .child(
                // Minimal terminal status footer strip (26px)
                div()
                    .h(px(26.0))
                    .flex_shrink_0()
                    .px(px(12.0))
                    .bg(rgb(palette::SIDEBAR_BG))
                    .border_t_1()
                    .border_color(rgb(if is_focused && st.layout.split_terminals {
                        accent
                    } else {
                        palette::BORDER
                    }))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded(px(99.0))
                                    .bg(rgb(sess.effective_state().color_u32())),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .truncate()
                                    .child(format!(
                                        "{} · {} · {}",
                                        display_compact_path(&sess.cwd),
                                        branch_label,
                                        if sess.attached_hc { "hc" } else { "pty" }
                                    )),
                            )
                            .when_some(osc_badge, |el, badge| {
                                el.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .text_size(px(10.5))
                                        .text_color(rgb(accent))
                                        .truncate()
                                        .child(badge),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .when(eff_off > 0, |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "scroll-reset-{pane_slot:?}"
                                        )))
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .border_1()
                                        .border_color(rgb(accent))
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .text_color(rgb(accent))
                                        .child(format!("↑ -{eff_off} lines"))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    if let Some(s) = g
                                                        .sessions
                                                        .iter_mut()
                                                        .find(|s| s.id == sid_jump_bottom)
                                                    {
                                                        s.scroll_offset = 0;
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(tr(loc, "Direct input", "键盘直连")),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("ctrlc-{pane_slot:?}")))
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child("^C")
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                            if let Ok(mgr) = this.pty_mgr.lock() {
                                                let _ = mgr.send_sigint(&sid_sigint);
                                            }
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("clear-{pane_slot:?}")))
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(tr(loc, "Clear", "清屏"))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                            if let Ok(mgr) = this.pty_mgr.lock() {
                                                mgr.clear_buffer(&sid_clear);
                                            }
                                            if let Ok(mut g) = this.state.lock() {
                                                if let Some(s) = g
                                                    .sessions
                                                    .iter_mut()
                                                    .find(|s| s.id == sid_clear_scroll)
                                                {
                                                    s.scroll_offset = 0;
                                                }
                                            }
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_terminals_grid(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let visible_sessions: Vec<&TerminalSessionEntry> = st.sessions.iter().take(4).collect();
        if visible_sessions.is_empty() {
            return self
                .render_terminal_pane(st, None, SplitPane::Primary, 32, cx)
                .into_any_element();
        }
        let font_family: SharedString = st.layout.font_family.clone().into();
        let font_size = px((st.layout.terminal_font_size_px - 1.0).clamp(10.0, 20.0));

        div()
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .p(px(8.0))
            .flex()
            .flex_row()
            .gap(px(8.0))
            .overflow_hidden()
            .children(visible_sessions.into_iter().map(|sess| {
                let sid = sess.id.clone();
                let is_active = st.active_session_id.as_deref() == Some(&sess.id);
                div()
                    .id(SharedString::from(format!("tile-{}", sess.id)))
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .rounded(px(8.0))
                    .bg(rgb(palette::SIDEBAR_BG))
                    .border_1()
                    .border_color(rgb(if is_active {
                        st.layout.accent_color_u32()
                    } else {
                        palette::BORDER
                    }))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.active_session_id = Some(sid.clone());
                                g.focused_pane = SplitPane::Primary;
                                g.input_target = ActiveInputTarget::TerminalDirect;
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .h(px(28.0))
                            .px(px(10.0))
                            .border_b_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded(px(99.0))
                                            .bg(rgb(sess.effective_state().color_u32())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(sess.title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(sess.elapsed_label()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .p(px(10.0))
                            .bg(rgb(palette::CANVAS_BG))
                            .font_family(font_family.clone())
                            .text_size(font_size)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap(px(1.5))
                            .children({
                                let total = sess.styled_lines.len();
                                let start = total.saturating_sub(32);
                                sess.styled_lines[start..]
                                    .iter()
                                    .map(render_styled_terminal_line)
                            }),
                    )
            }))
            .when(st.sessions.is_empty(), |el| {
                el.child(div().text_color(rgb(palette::TEXT_MUTED)).child(tr(
                    loc,
                    "No open terminals",
                    "暂无终端",
                )))
            })
            .into_any_element()
    }

    fn render_code_workspace(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let active_path = st
            .active_space()
            .map(|s| PathBuf::from(&s.path))
            .unwrap_or_else(|| PathBuf::from("."));
        let diff_lines: Vec<String> = st
            .repo_snapshot
            .diff_text
            .lines()
            .take(180)
            .map(|l| l.to_string())
            .collect();
        let file_lines: Vec<String> = st
            .selected_file_content
            .lines()
            .take(180)
            .map(|l| l.to_string())
            .collect();

        div()
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(
                        // Left file & change tree
                        div()
                            .w(px(230.0))
                            .h_full()
                            .flex_shrink_0()
                            .bg(rgb(palette::SIDEBAR_BG))
                            .border_r_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(32.0))
                                    .px(px(10.0))
                                    .border_b_1()
                                    .border_color(rgb(palette::BORDER))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child(if st.code_show_diff {
                                                tr(loc, "GIT CHANGES", "Git 变更")
                                            } else {
                                                tr(loc, "EXPLORER", "文件树")
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("btn-code-toggle-diff")
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(palette::SURFACE_BG))
                                            .cursor_pointer()
                                            .text_size(px(10.5))
                                            .text_color(rgb(accent))
                                            .child(if st.code_show_diff {
                                                tr(loc, "Files", "文件")
                                            } else {
                                                tr(loc, "Diff", "Diff")
                                            })
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.code_show_diff = !g.code_show_diff;
                                                    }
                                                    cx.notify();
                                                }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("code-tree-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .p(px(6.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .children(st.dir_entries.iter().take(60).map(|entry| {
                                        let rel = entry.rel_path.clone();
                                        let base_path = active_path.clone();
                                        let full = base_path.join(&entry.rel_path);
                                        let is_selected =
                                            st.selected_file_rel.as_deref() == Some(&entry.rel_path);
                                        div()
                                            .id(SharedString::from(format!("file-{}", entry.rel_path)))
                                            .px(px(7.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(if is_selected {
                                                palette::HOVER_BG
                                            } else {
                                                palette::SIDEBAR_BG
                                            }))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .cursor_pointer()
                                            .text_size(px(11.5))
                                            .text_color(rgb(if entry.is_dir {
                                                accent
                                            } else {
                                                palette::TEXT_PRIMARY
                                            }))
                                            .truncate()
                                            .child(if entry.is_dir {
                                                format!("{}/", entry.name)
                                            } else {
                                                entry.name.clone()
                                            })
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                    if full.is_dir() {
                                                        let entries =
                                                            list_workspace_dir(&base_path, &full);
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.dir_entries = entries;
                                                        }
                                                    } else if full.is_file() {
                                                        let content =
                                                            read_workspace_file_preview(&full, 120_000);
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.selected_file_rel = Some(rel.clone());
                                                            g.selected_file_content = content;
                                                            g.code_show_diff = false;
                                                        }
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                    })),
                            ),
                    )
                    .child(
                        // Right code / diff surface
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div()
                                    .h(px(32.0))
                                    .px(px(12.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_b_1()
                                    .border_color(rgb(palette::BORDER))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(if st.code_show_diff {
                                                format!(
                                                    "git diff · {} ({})",
                                                    st.repo_snapshot.branch,
                                                    st.repo_snapshot.changed_files.len()
                                                )
                                            } else {
                                                st.selected_file_rel.clone().unwrap_or_else(|| {
                                                    tr(
                                                        loc,
                                                        "Select a file or toggle Diff",
                                                        "从左侧选择文件或切换 Diff",
                                                    )
                                                    .to_string()
                                                })
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("code-editor-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .p(px(12.0))
                                    .overflow_y_scroll()
                                    .font_family("Menlo")
                                    .text_size(px(12.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .when(st.code_show_diff, |el| {
                                        el.children(diff_lines.into_iter().map(|line| {
                                            let color = if line.starts_with('+') {
                                                palette::DONE_GREEN
                                            } else if line.starts_with('-') {
                                                palette::ERROR_RED
                                            } else if line.starts_with("@@") {
                                                palette::WORKING_SKY
                                            } else {
                                                palette::TEXT_MUTED
                                            };
                                            div().text_color(rgb(color)).child(line)
                                        }))
                                    })
                                    .when(!st.code_show_diff, |el| {
                                        el.children(file_lines.into_iter().enumerate().map(
                                            |(idx, line)| {
                                                div()
                                                    .flex()
                                                    .gap(px(12.0))
                                                    .child(
                                                        div()
                                                            .w(px(36.0))
                                                            .text_color(rgb(palette::TEXT_DIM))
                                                            .child(format!("{:>4}", idx + 1)),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                                            .child(line),
                                                    )
                                            },
                                        ))
                                    }),
                            ),
                    ),
            )
            .when(st.code_bottom_terminal, |el| {
                el.child(div().h(px(1.0)).w_full().bg(rgb(palette::BORDER)))
                    .child(div().h(px(210.0)).w_full().child(
                        self.render_terminal_pane(
                            st,
                            st.active_session_id.as_deref(),
                            SplitPane::Primary,
                            11,
                            cx,
                        ),
                    ))
            })
    }

    fn render_board_kanban(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let cols = [
            (
                AgentLifecycleState::NeedsYou,
                tr(loc, "Needs you", "待处理"),
                palette::NEEDS_YOU_AMBER,
            ),
            (
                AgentLifecycleState::Working,
                tr(loc, "Working", "执行中"),
                palette::WORKING_SKY,
            ),
            (
                AgentLifecycleState::Done,
                tr(loc, "Done", "已完成"),
                palette::DONE_GREEN,
            ),
            (
                AgentLifecycleState::Idle,
                tr(loc, "Idle", "空闲"),
                palette::TEXT_DIM,
            ),
        ];

        div()
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .p(px(14.0))
            .flex()
            .flex_row()
            .gap(px(12.0))
            .overflow_hidden()
            .children(cols.into_iter().map(|(state_kind, title, color)| {
                let matching: Vec<&TerminalSessionEntry> = st
                    .sessions
                    .iter()
                    .filter(|s| s.effective_state() == state_kind)
                    .collect();
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .rounded(px(8.0))
                    .bg(rgb(palette::SIDEBAR_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .p(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .w(px(7.0))
                                            .h(px(7.0))
                                            .rounded(px(99.0))
                                            .bg(rgb(color)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(title),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(format!("{}", matching.len())),
                            ),
                    )
                    .children(matching.into_iter().map(|sess| {
                        let sid = sess.id.clone();
                        let sp_name = st
                            .spaces
                            .iter()
                            .find(|s| s.id == sess.space_id)
                            .map(|s| s.name.as_str())
                            .unwrap_or("local")
                            .to_string();
                        div()
                            .id(SharedString::from(format!("board-card-{}", sess.id)))
                            .p(px(10.0))
                            .rounded(px(6.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .hover(|h| h.border_color(rgb(color)))
                            .cursor_pointer()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(sess.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(sess.elapsed_label()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .truncate()
                                    .child(sp_name),
                            )
                            .child(
                                div()
                                    .font_family("Menlo")
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .truncate()
                                    .child(sess.preview_snippet()),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                    this.select_session(&sid);
                                    cx.notify();
                                }),
                            )
                    }))
            }))
    }

    fn render_right_drawer(
        &self,
        st: &CodaDesktopState,
        tab: RightDrawerTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let active_path = st
            .active_space()
            .map(|s| PathBuf::from(&s.path))
            .unwrap_or_else(|| PathBuf::from("."));

        div()
            .w(px(340.0))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(palette::SIDEBAR_BG))
            .border_l_1()
            .border_color(rgb(palette::BORDER))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .h(px(36.0))
                    .px(px(10.0))
                    .border_b_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .children(
                                [
                                    (RightDrawerTab::Changes, tr(loc, "Changes", "变更")),
                                    (RightDrawerTab::Files, tr(loc, "Files", "文件")),
                                    (RightDrawerTab::Mesh, tr(loc, "Mesh", "节点")),
                                ]
                                .into_iter()
                                .map(|(t, lbl)| {
                                    let active = tab == t;
                                    div()
                                        .id(SharedString::from(format!("drawer-tab-{t:?}")))
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(if active {
                                            palette::HOVER_BG
                                        } else {
                                            palette::SIDEBAR_BG
                                        }))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(if active {
                                            palette::TEXT_PRIMARY
                                        } else {
                                            palette::TEXT_MUTED
                                        }))
                                        .child(lbl)
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.right_drawer = Some(t);
                                                }
                                                cx.notify();
                                            }),
                                        )
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("btn-close-drawer")
                            .px(px(6.0))
                            .py(px(2.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child("×")
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.right_drawer = None;
                                    }
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("right-drawer-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .when(tab == RightDrawerTab::Changes, |el| {
                        let snap = &st.repo_snapshot;
                        let path_for_finder = active_path.to_string_lossy().to_string();
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(format!("{} · {}", snap.branch, snap.changed_files.len())),
                                )
                                .child(
                                    div()
                                        .id("btn-reveal-finder")
                                        .px(px(7.0))
                                        .py(px(3.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child(tr(loc, "Finder", "在 Finder 打开"))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |_, _: &MouseUpEvent, _, _| {
                                                reveal_in_finder(&path_for_finder);
                                            }),
                                        ),
                                ),
                        )
                        .children(snap.changed_files.iter().take(18).map(|f| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .font_family("Menlo")
                                .text_size(px(11.0))
                                .child(
                                    div()
                                        .text_color(rgb(palette::NEEDS_YOU_AMBER))
                                        .child(f.status.clone()),
                                )
                                .child(
                                    div()
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .truncate()
                                        .child(f.path.clone()),
                                )
                        }))
                        .when(!snap.recent_commits.is_empty(), |el| {
                            el.child(
                                div()
                                    .pt(px(6.0))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(tr(loc, "RECENT COMMITS", "最近提交")),
                            )
                            .children(snap.recent_commits.iter().take(6).map(|c| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(px(11.0))
                                    .child(
                                        div()
                                            .font_family("Menlo")
                                            .text_color(rgb(accent))
                                            .child(c.short_sha.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .truncate()
                                            .child(c.subject.clone()),
                                    )
                            }))
                        })
                    })
                    .when(tab == RightDrawerTab::Files, |el| {
                        el.children(st.dir_entries.iter().take(36).map(|entry| {
                            let full = active_path.join(&entry.rel_path);
                            let rel = entry.rel_path.clone();
                            div()
                                .id(SharedString::from(format!("drawer-file-{}", entry.rel_path)))
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .text_color(rgb(if entry.is_dir {
                                    accent
                                } else {
                                    palette::TEXT_PRIMARY
                                }))
                                .truncate()
                                .child(if entry.is_dir {
                                    format!("{}/", entry.name)
                                } else {
                                    entry.name.clone()
                                })
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                        if full.is_file() {
                                            let content = read_workspace_file_preview(&full, 120_000);
                                            if let Ok(mut g) = this.state.lock() {
                                                g.selected_file_rel = Some(rel.clone());
                                                g.selected_file_content = content;
                                                g.layout.layout = LayoutMode::Code;
                                                g.code_show_diff = false;
                                            }
                                            this.persist_layout_config();
                                        }
                                        cx.notify();
                                    }),
                                )
                        }))
                    })
                    .when(tab == RightDrawerTab::Mesh, |el| {
                        self.render_mesh_drawer_body(st, el, cx)
                    }),
            )
    }

    fn render_mesh_drawer_body(
        &self,
        st: &CodaDesktopState,
        el: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let loc = st.locale;
        el.child(
            div()
                .p(px(10.0))
                .rounded(px(6.0))
                .bg(rgb(palette::SURFACE_BG))
                .border_1()
                .border_color(rgb(palette::BORDER))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tr(loc, "Coda Cloud Mesh", "Coda 加密算力网络")),
                )
                .child(
                    div()
                        .id("input-auth-server")
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .bg(rgb(palette::CANVAS_BG))
                        .border_1()
                        .border_color(rgb(if st.input_target == ActiveInputTarget::AuthServerUrl {
                            st.layout.accent_color_u32()
                        } else {
                            palette::BORDER
                        }))
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(rgb(palette::TEXT_PRIMARY))
                        .child(st.server_url.clone())
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthServerUrl;
                                }
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("btn-browser-signin")
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(5.0))
                        .bg(rgb(palette::HOVER_BG))
                        .border_1()
                        .border_color(rgb(palette::BORDER_STRONG))
                        .cursor_pointer()
                        .text_size(px(11.5))
                        .text_color(rgb(palette::TEXT_PRIMARY))
                        .child(tr(loc, "Sign in via Browser (RFC 8628)", "浏览器一键授权登录"))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                let srv = this
                                    .state
                                    .lock()
                                    .map(|g| g.server_url.clone())
                                    .unwrap_or_default();
                                spawn_device_login(
                                    Arc::clone(&this.state),
                                    Arc::clone(&this.ui_dirty),
                                    srv,
                                );
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("input-auth-email")
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .bg(rgb(palette::CANVAS_BG))
                        .border_1()
                        .border_color(rgb(if st.input_target == ActiveInputTarget::AuthAccount {
                            st.layout.accent_color_u32()
                        } else {
                            palette::BORDER
                        }))
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(rgb(if st.auth_account_input.is_empty() {
                            palette::TEXT_DIM
                        } else {
                            palette::TEXT_PRIMARY
                        }))
                        .child(if st.auth_account_input.is_empty() {
                            tr(loc, "Email / Username", "账号邮箱").to_string()
                        } else {
                            st.auth_account_input.clone()
                        })
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthAccount;
                                }
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("input-auth-pass")
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(4.0))
                        .bg(rgb(palette::CANVAS_BG))
                        .border_1()
                        .border_color(rgb(if st.input_target == ActiveInputTarget::AuthPassword {
                            st.layout.accent_color_u32()
                        } else {
                            palette::BORDER
                        }))
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(rgb(if st.auth_password_input.is_empty() {
                            palette::TEXT_DIM
                        } else {
                            palette::TEXT_PRIMARY
                        }))
                        .child(if st.auth_password_input.is_empty() {
                            tr(loc, "Password (Enter submits)", "密码 (回车登录)").to_string()
                        } else {
                            "*".repeat(st.auth_password_input.chars().count())
                        })
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthPassword;
                                }
                                cx.notify();
                            }),
                        ),
                )
                .when_some(st.device_login_code.clone(), |el, code| {
                    el.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(palette::WORKING_SKY))
                            .child(format!("Code: {code}")),
                    )
                })
                .when_some(st.status_notice.clone(), |el, note| {
                    el.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(note),
                    )
                }),
        )
        .child(
            div()
                .text_size(px(10.5))
                .text_color(rgb(palette::TEXT_DIM))
                .child(format!(
                    "{} ({})",
                    tr(loc, "REMOTE HOSTS", "在线节点"),
                    st.remote_hosts.len()
                )),
        )
        .children(st.remote_hosts.iter().map(|host| {
            let host_clone = host.clone();
            div()
                .id(SharedString::from(format!("mesh-host-{}", host.id)))
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(palette::SURFACE_BG))
                .border_1()
                .border_color(rgb(palette::BORDER))
                .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .w(px(6.0))
                                .h(px(6.0))
                                .rounded(px(99.0))
                                .bg(rgb(if host.is_online {
                                    palette::DONE_GREEN
                                } else {
                                    palette::TEXT_DIM
                                })),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(palette::TEXT_PRIMARY))
                                .child(host.name.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(palette::TEXT_DIM))
                        .child(format!("{} · {}", host.platform, host.role)),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                        spawn_probe_remote_host(
                            Arc::clone(&this.state),
                            Arc::clone(&this.ui_dirty),
                            host_clone.clone(),
                        );
                        cx.notify();
                    }),
                )
        }))
    }

    fn render_command_palette_overlay(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let items = st.palette_items();
        let selected = st.palette_selected.min(items.len().saturating_sub(1));

        div()
            .id("palette-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000099))
            .pt(px(72.0))
            .flex()
            .justify_center()
            .items_start()
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if let Ok(mut g) = this.state.lock() {
                        g.palette_open = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("palette-modal")
                    .w(px(540.0))
                    .rounded(px(10.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER_STRONG))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_up(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .h(px(42.0))
                            .px(px(14.0))
                            .border_b_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(accent))
                                    .child("⌘P"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(13.0))
                                    .text_color(rgb(if st.palette_query.is_empty() {
                                        palette::TEXT_DIM
                                    } else {
                                        palette::TEXT_PRIMARY
                                    }))
                                    .child(if st.palette_query.is_empty() {
                                        tr(
                                            loc,
                                            "Jump to space, terminal, layout, or command...",
                                            "搜索工作区、终端会话、视图或命令...",
                                        )
                                        .to_string()
                                    } else {
                                        st.palette_query.clone()
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .max_h(px(360.0))
                            .p(px(6.0))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(items.into_iter().take(12).enumerate().map(|(idx, item)| {
                                let is_sel = idx == selected;
                                let action = item.action.clone();
                                div()
                                    .id(SharedString::from(format!("pal-item-{idx}")))
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(if is_sel {
                                        palette::HOVER_BG
                                    } else {
                                        palette::CHROME_BG
                                    }))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .px(px(5.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(palette::SURFACE_BG))
                                                    .text_size(px(9.5))
                                                    .text_color(rgb(accent))
                                                    .child(item.kind_tag),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.5))
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .child(item.label),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .truncate()
                                            .child(item.detail),
                                    )
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                                            this.execute_palette_action(action.clone());
                                            cx.notify();
                                        }),
                                    )
                            })),
                    ),
            )
    }
}

fn render_styled_terminal_line(line: &StyledTerminalLine) -> impl IntoElement {
    if line.text.is_empty() {
        return div().h(px(17.0)).child(" ").into_any_element();
    }
    let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = line
        .spans
        .iter()
        .filter(|s| s.start < s.end && s.end <= line.text.len())
        .map(|s| {
            let style = HighlightStyle {
                color: s.fg.map(|c| Hsla::from(rgb(c))),
                background_color: s.bg.map(|c| Hsla::from(rgb(c))),
                font_weight: s.bold.then_some(FontWeight::BOLD),
                ..Default::default()
            };
            (s.start..s.end, style)
        })
        .collect();

    div()
        .min_h(px(17.0))
        .text_color(rgb(palette::TEXT_PRIMARY))
        .child(StyledText::new(line.text.clone()).with_highlights(highlights))
        .into_any_element()
}

// ============================================================================
// Background Workers (`herdr-core` persistent sessions & `coda-daemon` Mesh)
// ============================================================================

fn spawn_herdr_discovery(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    pty_mgr: Arc<Mutex<LocalPtyManager>>,
    ui_dirty: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let Some(hc_bin) = discover_hc_binary() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let Ok(hc) = HerdrClient::connect_with_bin(hc_bin.clone()).await else {
                return;
            };
            let ver = hc.info().version.clone();
            let list = hc.list_terminals(false).await.unwrap_or_default();
            if let Ok(mut g) = state_bg.lock() {
                g.hc_available = true;
                g.hc_version = Some(ver);
            }

            let home_cwd = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"));

            for term in list {
                let id = format!("hc-{}", term.address);
                let attached = if let Ok(mut mgr) = pty_mgr.lock() {
                    if !mgr.has_session(&id) {
                        mgr.spawn_herdr_attach(&id, &home_cwd, &hc_bin, &term.address, 120, 36)
                            .is_ok()
                    } else {
                        true
                    }
                } else {
                    false
                };
                if attached {
                    if let Ok(mut g) = state_bg.lock() {
                        if !g.sessions.iter().any(|s| s.id == id) {
                            let space_id = g
                                .spaces
                                .iter()
                                .find(|sp| sp.path == term.cwd)
                                .map(|sp| sp.id.clone())
                                .or_else(|| g.active_space_id.clone())
                                .unwrap_or_else(|| "local".to_string());
                            g.sessions.push(TerminalSessionEntry {
                                id: id.clone(),
                                title: format!(
                                    "hc · {}",
                                    term.name.as_deref().unwrap_or(&term.address)
                                ),
                                space_id,
                                cwd: term.cwd,
                                started_at: Instant::now(),
                                last_output_at: Instant::now(),
                                last_byte_sig: 0,
                                lines: Vec::new(),
                                styled_lines: Vec::new(),
                                scroll_offset: 0,
                                alive: true,
                                attached_hc: true,
                                hc_addr: Some(term.address),
                                remote_host_id: None,
                                osc_status: None,
                            });
                        }
                    }
                }
            }
            ui_dirty.store(true, Ordering::Release);
        });
    });
}

fn spawn_refresh_hosts(state_bg: Arc<Mutex<CodaDesktopState>>, ui_dirty: Arc<AtomicBool>) {
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
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            if let Ok(list) = list_account_hosts(&cfg.server_url, tok).await {
                let hosts: Vec<RemoteHostSummary> =
                    list.into_iter().filter(|h| h.id != cfg.device_id).collect();
                if let Ok(mut g) = state_bg.lock() {
                    for host in &hosts {
                        let sid = format!("remote:{}", host.id);
                        if !g.spaces.iter().any(|s| s.id == sid) {
                            g.spaces.push(SpaceEntry {
                                id: sid,
                                avatar_color: space_avatar_color(&host.name),
                                name: host.name.clone(),
                                path: format!("ssh://{}/", host.name),
                                machine_id: host.id.to_string(),
                                machine_label: host.name.clone(),
                                is_remote: true,
                                branch: if host.is_online { "online" } else { "offline" }.to_string(),
                                dirty_files: 0,
                                ahead: 0,
                                behind: 0,
                            });
                        }
                    }
                    g.remote_hosts = hosts;
                }
                ui_dirty.store(true, Ordering::Release);
            }
        });
    });
}

fn spawn_device_login(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    ui_dirty: Arc<AtomicBool>,
    server_url: String,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            match start_device_login_flow(&server_url, "hybrid", None, None).await {
                Ok(flow) => {
                    let uri = flow.verification_uri_complete.clone();
                    #[cfg(target_os = "macos")]
                    {
                        let _ = std::process::Command::new("open").arg(&uri).spawn();
                    }
                    if let Ok(mut g) = state_bg.lock() {
                        g.device_login_code = Some(flow.user_code.clone());
                        g.device_login_uri = Some(uri);
                        g.status_notice = Some("Approve sign-in in your browser...".to_string());
                    }
                    ui_dirty.store(true, Ordering::Release);

                    match poll_device_login_until_approved(&flow).await {
                        Ok(cfg) => {
                            if let Ok(mut g) = state_bg.lock() {
                                g.is_logged_in = true;
                                g.account_email = cfg
                                    .owner_email
                                    .clone()
                                    .unwrap_or_else(|| cfg.owner_handle.clone());
                                g.device_login_code = None;
                                g.status_notice = Some("Signed in".to_string());
                            }
                            ui_dirty.store(true, Ordering::Release);
                            spawn_refresh_hosts(Arc::clone(&state_bg), Arc::clone(&ui_dirty));
                        }
                        Err(err) => {
                            if let Ok(mut g) = state_bg.lock() {
                                g.status_notice = Some(format!("Login failed: {err}"));
                            }
                            ui_dirty.store(true, Ordering::Release);
                        }
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_notice = Some(format!("Auth init failed: {err}"));
                    }
                    ui_dirty.store(true, Ordering::Release);
                }
            }
        });
    });
}

fn spawn_password_login(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    ui_dirty: Arc<AtomicBool>,
    server_url: String,
    email: String,
    password: String,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            match login_with_password_and_enroll(&server_url, &email, &password, "hybrid", None).await {
                Ok(cfg) => {
                    if let Ok(mut g) = state_bg.lock() {
                        g.is_logged_in = true;
                        g.account_email = cfg.owner_email.clone().unwrap_or(email);
                        g.auth_password_input.clear();
                        g.status_notice = Some("Signed in".to_string());
                    }
                    ui_dirty.store(true, Ordering::Release);
                    spawn_refresh_hosts(Arc::clone(&state_bg), Arc::clone(&ui_dirty));
                }
                Err(err) => {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_notice = Some(format!("Login error: {err}"));
                    }
                    ui_dirty.store(true, Ordering::Release);
                }
            }
        });
    });
}

fn spawn_probe_remote_host(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    ui_dirty: Arc<AtomicBool>,
    host: RemoteHostSummary,
) {
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
                        include_finished: false,
                    };
                    let term_count = match peer.request(&req, TrafficLane::ControlP0).await {
                        Ok(PeerMessage::TerminalsSnapshot { terminals, .. }) => terminals.len(),
                        _ => 0,
                    };
                    if let Ok(mut g) = state_bg.lock() {
                        g.remote_probe_status =
                            Some(format!("{} · {route} · {term_count} sessions", host.name));
                        g.status_notice = g.remote_probe_status.clone();
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_notice = Some(format!("{} unreachable: {err}", host.name));
                    }
                }
            }
            ui_dirty.store(true, Ordering::Release);
        });
    });
}

// ============================================================================
// Application Launcher & Direct PTY / Palette Keyboard Loop
// ============================================================================

pub fn launch_gpui_desktop_app() {
    ensure_macos_gui_path();

    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", QuitApp, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
            KeyBinding::new("cmd-n", NewTerminal, None),
            KeyBinding::new("cmd-t", NewTerminal, None),
            KeyBinding::new("cmd-d", ToggleSplit, None),
            KeyBinding::new("cmd-p", ToggleCommandPalette, None),
            KeyBinding::new("cmd-k", ToggleCommandPalette, None),
            KeyBinding::new("cmd-1", SwitchLayoutWorkbench, None),
            KeyBinding::new("cmd-2", SwitchLayoutTerminals, None),
            KeyBinding::new("cmd-3", SwitchLayoutCode, None),
            KeyBinding::new("cmd-4", SwitchLayoutBoard, None),
            KeyBinding::new("ctrl-c", InterruptActiveTerminal, None),
        ]);
        cx.on_action(|_: &QuitApp, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1380.0), px(860.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: None,
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.0), px(12.0))),
                    }),
                    window_min_size: Some(size(px(960.0), px(600.0))),
                    ..Default::default()
                },
                |_, cx| cx.new(CodaGpuWorkspaceView::new),
            )
            .expect("failed to open Coda GPUI desktop window");

        let view = window
            .update(cx, |this, win, cx| {
                win.focus(&this.focus_handle);
                cx.entity()
            })
            .unwrap();

        cx.observe_keystrokes(move |ev, _, cx| {
            let key = ev.keystroke.key.as_str();
            let mods = &ev.keystroke.modifiers;

            // Cmd+V Paste into focused input target or active PTY
            if mods.platform && key == "v" {
                if let Some(txt) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    view.update(cx, |this, cx| {
                        let (target, focused_id) = this
                            .state
                            .lock()
                            .map(|g| (g.input_target, g.focused_session_id().map(|s| s.to_string())))
                            .unwrap_or((ActiveInputTarget::TerminalDirect, None));
                        match target {
                            ActiveInputTarget::TerminalDirect => {
                                if let (Some(id), Ok(mgr)) = (focused_id, this.pty_mgr.lock()) {
                                    let _ = mgr.write_input(&id, txt.as_bytes());
                                }
                            }
                            ActiveInputTarget::CommandPalette => {
                                if let Ok(mut g) = this.state.lock() {
                                    g.palette_query.push_str(txt.trim());
                                    g.palette_selected = 0;
                                }
                            }
                            ActiveInputTarget::AuthServerUrl => {
                                if let Ok(mut g) = this.state.lock() {
                                    g.server_url.push_str(txt.trim());
                                }
                            }
                            ActiveInputTarget::AuthAccount => {
                                if let Ok(mut g) = this.state.lock() {
                                    g.auth_account_input.push_str(txt.trim());
                                }
                            }
                            ActiveInputTarget::AuthPassword => {
                                if let Ok(mut g) = this.state.lock() {
                                    g.auth_password_input.push_str(&txt);
                                }
                            }
                        }
                        cx.notify();
                    });
                }
                return;
            }

            if mods.platform {
                return;
            }

            view.update(cx, |this, cx| {
                let (target, palette_open, focused_id) = this
                    .state
                    .lock()
                    .map(|g| {
                        (
                            g.input_target,
                            g.palette_open,
                            g.focused_session_id().map(|s| s.to_string()),
                        )
                    })
                    .unwrap_or((ActiveInputTarget::TerminalDirect, false, None));

                if palette_open || target == ActiveInputTarget::CommandPalette {
                    match key {
                        "escape" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.palette_open = false;
                                g.palette_query.clear();
                                g.input_target = ActiveInputTarget::TerminalDirect;
                            }
                        }
                        "up" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.palette_selected = g.palette_selected.saturating_sub(1);
                            }
                        }
                        "down" => {
                            if let Ok(mut g) = this.state.lock() {
                                let len = g.palette_items().len();
                                if g.palette_selected + 1 < len {
                                    g.palette_selected += 1;
                                }
                            }
                        }
                        "enter" => {
                            let selected_action = this.state.lock().ok().and_then(|g| {
                                let items = g.palette_items();
                                items
                                    .get(g.palette_selected.min(items.len().saturating_sub(1)))
                                    .map(|i| i.action.clone())
                            });
                            if let Some(action) = selected_action {
                                this.execute_palette_action(action);
                            }
                        }
                        "backspace" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.palette_query.pop();
                                g.palette_selected = 0;
                            }
                        }
                        _ => {
                            if let Some(ref ch) = ev.keystroke.key_char {
                                if !mods.control {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.palette_query.push_str(ch);
                                        g.palette_selected = 0;
                                    }
                                }
                            }
                        }
                    }
                    cx.notify();
                    return;
                }

                if target == ActiveInputTarget::TerminalDirect {
                    if let (Some(id), Ok(mgr)) = (focused_id, this.pty_mgr.lock()) {
                        if let Ok(mut g) = this.state.lock() {
                            if let Some(s) = g.sessions.iter_mut().find(|s| s.id == id) {
                                s.scroll_offset = 0;
                            }
                        }
                        if mods.control {
                            let ctrl_byte = match key {
                                "c" => Some(0x03u8),
                                "d" => Some(0x04u8),
                                "l" => Some(0x0cu8),
                                "z" => Some(0x1au8),
                                _ => None,
                            };
                            if let Some(b) = ctrl_byte {
                                let _ = mgr.write_input(&id, &[b]);
                            }
                            cx.notify();
                            return;
                        }
                        let seq: Option<&[u8]> = match key {
                            "enter" => Some(b"\r"),
                            "backspace" => Some(&[0x7f]),
                            "tab" => Some(b"\t"),
                            "escape" => Some(b"\x1b"),
                            "up" => Some(b"\x1b[A"),
                            "down" => Some(b"\x1b[B"),
                            "right" => Some(b"\x1b[C"),
                            "left" => Some(b"\x1b[D"),
                            _ => None,
                        };
                        if let Some(bytes) = seq {
                            let _ = mgr.write_input(&id, bytes);
                        } else if let Some(ref ch) = ev.keystroke.key_char {
                            let _ = mgr.write_input(&id, ch.as_bytes());
                        }
                        cx.notify();
                        return;
                    }
                }

                // Auth form input fields inside Mesh drawer
                match target {
                    ActiveInputTarget::AuthServerUrl => match key {
                        "backspace" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.server_url.pop();
                            }
                        }
                        "escape" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.input_target = ActiveInputTarget::TerminalDirect;
                            }
                        }
                        _ => {
                            if let Some(ref ch) = ev.keystroke.key_char {
                                if !mods.control {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.server_url.push_str(ch);
                                    }
                                }
                            }
                        }
                    },
                    ActiveInputTarget::AuthAccount => match key {
                        "backspace" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.auth_account_input.pop();
                            }
                        }
                        "tab" | "enter" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.input_target = ActiveInputTarget::AuthPassword;
                            }
                        }
                        _ => {
                            if let Some(ref ch) = ev.keystroke.key_char {
                                if !mods.control {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.auth_account_input.push_str(ch);
                                    }
                                }
                            }
                        }
                    },
                    ActiveInputTarget::AuthPassword => match key {
                        "backspace" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.auth_password_input.pop();
                            }
                        }
                        "enter" => {
                            let (srv, acc, pw) = {
                                let Ok(g) = this.state.lock() else {
                                    return;
                                };
                                (
                                    g.server_url.clone(),
                                    g.auth_account_input.clone(),
                                    g.auth_password_input.clone(),
                                )
                            };
                            spawn_password_login(
                                Arc::clone(&this.state),
                                Arc::clone(&this.ui_dirty),
                                srv,
                                acc,
                                pw,
                            );
                        }
                        _ => {
                            if let Some(ref ch) = ev.keystroke.key_char {
                                if !mods.control {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.auth_password_input.push_str(ch);
                                    }
                                }
                            }
                        }
                    },
                    ActiveInputTarget::TerminalDirect | ActiveInputTarget::CommandPalette => {}
                }
                cx.notify();
            });
        })
        .detach();

        cx.activate(true);
        let _ = ClipboardItem::new_string;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::VtScreenBuffer;

    #[test]
    fn test_default_state_has_zero_demo_fixtures() {
        let state = CodaDesktopState::default();
        assert!(state.sessions.is_empty());
        assert!(state.spaces.is_empty());
        assert!(!state.is_logged_in);
        assert!(state.account_email.is_empty());
        assert_eq!(state.locale, UiLocale::En);
        assert_eq!(state.layout.layout, LayoutMode::Workbench);
        assert_eq!(state.input_target, ActiveInputTarget::TerminalDirect);
    }

    #[test]
    fn test_vt_truecolor_utf8_boundaries_alt_screen_and_osc7501() {
        let mut vt = VtScreenBuffer::new(80, 24);
        // TrueColor + 256-color + mixed ASCII & Chinese UTF-8 characters
        vt.feed_bytes(
            "\x1b[38;2;56;189;248mHello 中文 \x1b[38;5;196mERR\x1b[0m\r\n"
                .as_bytes(),
        );
        // OSC 7501 status update
        vt.feed_bytes(b"\x1b]7501;state=working;task=Running cargo test\x07");

        let osc = vt.osc_status().expect("expected parsed OSC 7501 status");
        assert_eq!(osc.state, AgentLifecycleState::Working);
        assert_eq!(osc.task.as_deref(), Some("Running cargo test"));

        let styled = vt.render_styled_lines(40, true);
        let first = &styled[0];
        assert_eq!(first.text, "Hello 中文 ERR");
        for span in &first.spans {
            assert!(first.text.is_char_boundary(span.start));
            assert!(first.text.is_char_boundary(span.end));
            assert!(span.start < span.end);
        }
        assert_eq!(first.spans[0].fg, Some(0x38bdf8));

        // Alternate screen switch preserves primary buffer on restore
        vt.feed_bytes(b"\x1b[?1049hALT SCREEN");
        let alt_lines = vt.render_lines(20);
        assert!(alt_lines.iter().any(|l| l.contains("ALT SCREEN")));

        vt.feed_bytes(b"\x1b[?1049l");
        let restored = vt.render_lines(20);
        assert!(restored.iter().any(|l| l.contains("Hello 中文 ERR")));
        assert!(!restored.iter().any(|l| l.contains("ALT SCREEN")));
    }

    #[test]
    fn test_malleable_layout_and_agents_md_generation() {
        let cfg = MalleableLayoutConfig::default();
        assert_eq!(cfg.layout, LayoutMode::Workbench);
        assert_eq!(cfg.accent_color_u32(), 0x38bdf8);
        assert_eq!(MalleableLayoutConfig::parse_hex_color("#22c55e"), Some(0x22c55e));

        let raw = r##"{"layout":"board","split_terminals":true,"accent_hex":"#f59e0b"}"##;
        let parsed: MalleableLayoutConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.layout, LayoutMode::Board);
        assert!(parsed.split_terminals);
        assert_eq!(parsed.accent_color_u32(), 0xf59e0b);
    }

    #[test]
    fn test_real_local_pty_execution_and_output_capture() {
        let dirty = Arc::new(AtomicBool::new(false));
        let mut pty = LocalPtyManager::new(dirty);
        let cwd = std::env::current_dir().unwrap();
        pty.spawn_session("test-pty", &cwd, None, 100, 24).unwrap();
        std::thread::sleep(Duration::from_millis(120));
        pty.send_command_line("test-pty", "echo HERDR_STANDARD_PTY_OK")
            .unwrap();
        std::thread::sleep(Duration::from_millis(250));
        let snaps = pty.snapshot_all(40);
        let snap = snaps.get("test-pty").expect("pty snapshot missing");
        assert!(
            snap.lines.iter().any(|l| l.contains("HERDR_STANDARD_PTY_OK")),
            "expected echoed marker in PTY output: {:?}",
            snap.lines
        );
        pty.kill_session("test-pty");
    }

    #[test]
    fn test_real_git_repo_snapshot_on_workspace() {
        let cwd = std::env::current_dir().unwrap();
        let snap = compute_repo_snapshot(&cwd, None);
        assert!(snap.is_git_repo);
        assert!(!snap.branch.is_empty());
    }
}
