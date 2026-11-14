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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::{
    list_account_hosts, login_with_password_and_enroll, mint_peer_relay_config,
    poll_device_login_until_approved, start_device_login_flow, start_embedded_host_if_configured,
    subscribe_account_events_ws, DaemonConfig, LivePeerSession, PeerAsyncEvent, RemoteHostSummary,
};
use coda_herdr::{discover_hc_binary, ensure_or_install_hc_binary, HerdrClient};
use coda_protocol::{HerdrTerminalSummary, PeerMessage, RemoteFileItem, TrafficLane};
use gpui::{
    actions, div, point, prelude::*, px, rgb, rgba, size, App, Application, Bounds, ClickEvent,
    ClipboardItem, Context, FocusHandle, Focusable, FontWeight, HighlightStyle, Hsla, KeyBinding,
    Menu, MenuItem, MouseButton, MouseDownEvent, ScrollDelta, ScrollWheelEvent, SharedString,
    StyledText, SystemMenuType, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use uuid::Uuid;

use crate::config::{
    config_root_dir, detect_local_machine_name, discover_local_git_repositories,
    display_compact_path, ensure_agents_md_manual, ensure_macos_gui_path,
    pick_folder_via_native_dialog, reveal_in_finder, space_avatar_color, tr, DesktopPreferences,
    LayoutMode, MalleableLayoutConfig, UiLocale,
};
use crate::git_inspector::{
    compute_repo_snapshot, list_workspace_dir, read_workspace_file_preview, ChangedFileEntry,
    RepoInspectorSnapshot, WorkspaceFileEntry,
};
use crate::pty::{AgentLifecycleState, LocalPtyManager, Osc7501Status, StyledTerminalLine};

actions!(
    coda_desktop,
    [
        QuitApp,
        ToggleSettings,
        ToggleSidebar,
        NewTerminal,
        CloseActiveTerminal,
        ToggleSplit,
        ToggleCommandPalette,
        SwitchLayoutWorkbench,
        SwitchLayoutTerminals,
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

/// Sections inside the native Settings modal (`Cmd+,`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    Appearance,
    Runtime,
    Machines,
    Shortcuts,
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
    OpenSettings,
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
    pub settings_open: bool,
    pub settings_section: SettingsSection,
    pub palette_open: bool,
    pub palette_query: String,
    pub palette_selected: usize,

    // Real Git / Files inspector for active space
    pub repo_snapshot: RepoInspectorSnapshot,
    pub dir_entries: Vec<WorkspaceFileEntry>,
    pub current_browse_rel: String,
    pub selected_file_rel: Option<String>,
    pub selected_file_content: String,
    pub selected_change_path: Option<String>,
    pub closed_session_ids: HashSet<String>,
    pub dismissed_spaces: HashSet<String>,
    pub did_auto_seed_local: bool,

    // `herdr-core` runtime state
    pub hc_available: bool,
    pub hc_version: Option<String>,
    pub hc_path: Option<PathBuf>,

    // Cloud mesh (`coda-daemon`) auth & remote hosts
    pub server_url: String,
    pub auth_account_input: String,
    pub auth_password_input: String,
    pub is_logged_in: bool,
    pub auth_form_expanded: bool,
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
            settings_open: false,
            settings_section: SettingsSection::Appearance,
            palette_open: false,
            palette_query: String::new(),
            palette_selected: 0,
            repo_snapshot: RepoInspectorSnapshot::default(),
            dir_entries: Vec::new(),
            current_browse_rel: String::new(),
            selected_file_rel: None,
            selected_file_content: String::new(),
            selected_change_path: None,
            closed_session_ids: HashSet::new(),
            dismissed_spaces: HashSet::new(),
            did_auto_seed_local: false,
            hc_available: false,
            hc_version: None,
            hc_path: None,
            server_url: DesktopPreferences::default().server_url,
            auth_account_input: String::new(),
            auth_password_input: String::new(),
            is_logged_in: false,
            auth_form_expanded: false,
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
        for mode in [LayoutMode::Workbench, LayoutMode::Terminals] {
            items.push(PaletteItem {
                kind_tag: "LAYOUT",
                label: mode.label(self.locale).to_string(),
                detail: format!("Switch layout to {}", mode.as_slug()),
                action: PaletteAction::SetLayout(mode),
            });
        }
        items.push(PaletteItem {
            kind_tag: "APP",
            label: tr(self.locale, "Settings…", "应用设置…").to_string(),
            detail: "Cmd+, · Settings · 设置".to_string(),
            action: PaletteAction::OpenSettings,
        });
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
            label: tr(self.locale, "Toggle Files Panel", "打开文件侧栏").to_string(),
            detail: "Side panel".to_string(),
            action: PaletteAction::OpenDrawer(RightDrawerTab::Files),
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Toggle Git Changes Panel", "打开 Git 变更侧栏").to_string(),
            detail: "Side panel".to_string(),
            action: PaletteAction::OpenDrawer(RightDrawerTab::Changes),
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Connect Remote Machine (Mesh)", "打开远端节点 (Mesh)").to_string(),
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
    pub mesh_hub: Arc<RemoteMeshHub>,
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

        init_state.dismissed_spaces = prefs.dismissed_spaces.iter().cloned().collect();

        // Discover real local repositories + saved user folders, falling back to `$HOME` if none exist.
        let mut candidates: Vec<PathBuf> = discover_local_git_repositories(&prefs.workspace_roots)
            .into_iter()
            .filter(|p| {
                !init_state.dismissed_spaces.contains(p)
                    && !init_state.dismissed_spaces.contains(&format!("local:{p}"))
            })
            .map(PathBuf::from)
            .collect();
        if candidates.is_empty() {
            if let Ok(home) = std::env::var("HOME") {
                if !init_state.dismissed_spaces.contains(&home)
                    && !init_state
                        .dismissed_spaces
                        .contains(&format!("local:{home}"))
                {
                    candidates.push(PathBuf::from(home));
                }
            } else if let Ok(cwd) = std::env::current_dir() {
                let cwd_s = cwd.to_string_lossy().to_string();
                if !init_state.dismissed_spaces.contains(&cwd_s)
                    && !init_state
                        .dismissed_spaces
                        .contains(&format!("local:{cwd_s}"))
                {
                    candidates.push(cwd);
                }
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
        init_state.active_space_id = prefs
            .selected_repo_path
            .as_deref()
            .and_then(|sel| {
                init_state
                    .spaces
                    .iter()
                    .find(|s| s.id == sel || s.path == sel)
                    .map(|s| s.id.clone())
            })
            .or_else(|| init_state.spaces.first().map(|s| s.id.clone()));

        let state = Arc::new(Mutex::new(init_state));
        let ui_dirty = Arc::new(AtomicBool::new(false));
        let pty_mgr = Arc::new(Mutex::new(LocalPtyManager::new(Arc::clone(&ui_dirty))));
        let mesh_hub = RemoteMeshHub::new(
            Arc::clone(&state),
            Arc::clone(&pty_mgr),
            Arc::clone(&ui_dirty),
        );

        let view = Self {
            state: Arc::clone(&state),
            pty_mgr: Arc::clone(&pty_mgr),
            ui_dirty: Arc::clone(&ui_dirty),
            mesh_hub: Arc::clone(&mesh_hub),
            focus_handle: cx.focus_handle(),
        };

        // Inspect active Git/Files workspace and start pure `herdr-core` + real-time mesh sync.
        view.refresh_active_space_inspector();
        spawn_herdr_discovery(Arc::clone(&state), Arc::clone(&pty_mgr), Arc::clone(&ui_dirty));
        mesh_hub.ensure_embedded_host();
        mesh_hub.start_account_sync();

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

    fn send_bytes_to_session(&self, session_id: &str, bytes: &[u8]) {
        let remote_target = self.state.lock().ok().and_then(|g| {
            g.sessions
                .iter()
                .find(|s| s.id == session_id)
                .and_then(|s| {
                    let host = s.remote_host_id.as_deref()?;
                    let host_id = Uuid::parse_str(host).ok()?;
                    let addr = s.hc_addr.clone()?;
                    Some((host_id, addr))
                })
        });
        if let Some((host_id, term_addr)) = remote_target {
            self.mesh_hub
                .send_remote_pty_input(host_id, &term_addr, bytes.to_vec());
        } else if let Ok(mgr) = self.pty_mgr.lock() {
            let _ = mgr.write_input(session_id, bytes);
        }
    }

    fn interrupt_session(&self, session_id: &str) {
        let remote_target = self.state.lock().ok().and_then(|g| {
            g.sessions
                .iter()
                .find(|s| s.id == session_id)
                .and_then(|s| {
                    let host = s.remote_host_id.as_deref()?;
                    let host_id = Uuid::parse_str(host).ok()?;
                    let addr = s.hc_addr.clone()?;
                    Some((host_id, addr))
                })
        });
        if let Some((host_id, term_addr)) = remote_target {
            self.mesh_hub.send_remote_interrupt(host_id, &term_addr);
        } else if let Ok(mgr) = self.pty_mgr.lock() {
            let _ = mgr.send_sigint(session_id);
        }
    }

    fn ensure_active_space(&self) {
        let Ok(mut g) = self.state.lock() else {
            return;
        };
        if g.active_space().is_some() {
            return;
        }
        if let Some(first_id) = g.spaces.first().map(|s| s.id.clone()) {
            g.active_space_id = Some(first_id);
            return;
        }
        let fallback_path = std::env::var("HOME")
            .map(PathBuf::from)
            .ok()
            .filter(|p| p.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        let path_str = fallback_path.to_string_lossy().to_string();
        let name = fallback_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("workspace")
            .to_string();
        let snap = compute_repo_snapshot(&fallback_path, None);
        let id = format!("local:{path_str}");
        let machine_label = g.local_machine_name.clone();
        g.dismissed_spaces.remove(&id);
        g.dismissed_spaces.remove(&path_str);
        g.spaces.push(SpaceEntry {
            id: id.clone(),
            avatar_color: space_avatar_color(&name),
            name,
            path: path_str,
            machine_id: "local".to_string(),
            machine_label,
            is_remote: false,
            branch: snap.branch,
            dirty_files: snap.changed_files.len(),
            ahead: 0,
            behind: 0,
        });
        g.active_space_id = Some(id);
    }

    fn refresh_active_space_inspector(&self) {
        let (space_id, space_path_str, remote_host_id, browse_rel, selected_change) = {
            let Ok(g) = self.state.lock() else {
                return;
            };
            let Some(space) = g.active_space() else {
                return;
            };
            let host_uuid = if space.is_remote {
                Uuid::parse_str(&space.machine_id).ok()
            } else {
                None
            };
            (
                space.id.clone(),
                space.path.clone(),
                host_uuid,
                g.current_browse_rel.clone(),
                g.selected_change_path.as_ref().map(|p| format!("file:{p}")),
            )
        };

        if let Some(host_id) = remote_host_id {
            self.mesh_hub
                .refresh_remote_git_diff(host_id, &space_id, &space_path_str);
            if space_path_str != "." {
                self.mesh_hub.refresh_remote_directory(
                    host_id,
                    &space_id,
                    &space_path_str,
                    &browse_rel,
                );
            }
            return;
        }
        let space_path = PathBuf::from(&space_path_str);
        let browse_dir = if browse_rel.trim().is_empty() {
            space_path.clone()
        } else {
            let candidate = space_path.join(&browse_rel);
            if candidate.is_dir() {
                candidate
            } else {
                space_path.clone()
            }
        };

        let snap = compute_repo_snapshot(&space_path, selected_change.as_deref());
        let entries = list_workspace_dir(&space_path, &browse_dir);

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
        self.ensure_active_space();
        let (space_id, space_name, cwd_str, is_remote, machine_id, hc_path, loc) = {
            let Ok(g) = self.state.lock() else {
                return;
            };
            let Some(sp) = g.active_space() else {
                return;
            };
            (
                sp.id.clone(),
                sp.name.clone(),
                sp.path.clone(),
                sp.is_remote,
                sp.machine_id.clone(),
                g.hc_path.clone().or_else(discover_hc_binary),
                g.locale,
            )
        };

        if is_remote {
            if let Ok(host_id) = Uuid::parse_str(&machine_id) {
                let agent = command.unwrap_or("shell");
                self.mesh_hub
                    .spawn_remote_terminal_or_agent(host_id, agent, &cwd_str, &space_id);
            } else if let Ok(mut g) = self.state.lock() {
                g.status_notice = Some(
                    tr(
                        loc,
                        "Remote host ID invalid; open Mesh to reconnect.",
                        "远端主机 ID 无效，请打开节点面板重新连接。",
                    )
                    .to_string(),
                );
                self.ui_dirty.store(true, Ordering::Release);
            }
            return;
        }

        let cwd = if Path::new(&cwd_str).is_dir() {
            PathBuf::from(&cwd_str)
        } else {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"))
        };
        let effective_cwd_str = cwd.to_string_lossy().to_string();

        if let Some(hc_bin) = hc_path {
            if let Ok(mut g) = self.state.lock() {
                g.status_notice = Some(
                    tr(loc, "Starting terminal...", "正在启动终端...").to_string(),
                );
            }
            self.ui_dirty.store(true, Ordering::Release);
            let state_bg = Arc::clone(&self.state);
            let pty_mgr_bg = Arc::clone(&self.pty_mgr);
            let ui_dirty = Arc::clone(&self.ui_dirty);
            let agent = command.unwrap_or("shell").to_string();
            let title = if command.is_some() {
                format!("{space_name} · {agent}")
            } else {
                format!("{space_name} · hc")
            };
            let hc_name = format!("coda-{}", &Uuid::new_v4().simple().to_string()[..8]);
            std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_notice = Some("Failed to init async runtime for hc".to_string());
                    }
                    ui_dirty.store(true, Ordering::Release);
                    return;
                };
                rt.block_on(async move {
                    match HerdrClient::connect_with_bin(hc_bin.clone()).await {
                        Ok(hc) => {
                            match hc
                                .launch_agent(
                                    &agent,
                                    Some(&hc_name),
                                    Some(&effective_cwd_str),
                                    None,
                                    120,
                                    36,
                                )
                                .await
                            {
                                Ok(term) => {
                                    let id = format!("hc-{}", term.address);
                                    let attach_res = pty_mgr_bg
                                        .lock()
                                        .map_err(|e| e.to_string())
                                        .and_then(|mut mgr| {
                                            mgr.spawn_herdr_attach(
                                                &id,
                                                &cwd,
                                                &hc_bin,
                                                &term.address,
                                                120,
                                                36,
                                            )
                                        });
                                    match attach_res {
                                        Ok(()) => {
                                            if let Ok(mut g) = state_bg.lock() {
                                                g.closed_session_ids.remove(&id);
                                                g.did_auto_seed_local = true;
                                                g.status_notice = None;
                                                if !g.sessions.iter().any(|s| s.id == id) {
                                                    g.sessions.push(TerminalSessionEntry {
                                                        id: id.clone(),
                                                        title,
                                                        space_id,
                                                        cwd: effective_cwd_str,
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
                                                if g.layout.split_terminals
                                                    && g.focused_pane == SplitPane::Secondary
                                                {
                                                    g.split_session_id = Some(id);
                                                } else {
                                                    g.active_session_id = Some(id);
                                                }
                                                g.input_target = ActiveInputTarget::TerminalDirect;
                                            }
                                            ui_dirty.store(true, Ordering::Release);
                                        }
                                        Err(e) => {
                                            let _ = hc.kill_and_remove(&term.address).await;
                                            if let Ok(mut g) = state_bg.lock() {
                                                g.status_notice =
                                                    Some(format!("hc attach failed: {e}"));
                                            }
                                            ui_dirty.store(true, Ordering::Release);
                                        }
                                    }
                                }
                                Err(e) => {
                                    if let Ok(mut g) = state_bg.lock() {
                                        g.status_notice = Some(format!("hc launch failed: {e}"));
                                    }
                                    ui_dirty.store(true, Ordering::Release);
                                }
                            }
                        }
                        Err(e) => {
                            if let Ok(mut g) = state_bg.lock() {
                                g.status_notice = Some(format!("hc connect failed: {e}"));
                            }
                            ui_dirty.store(true, Ordering::Release);
                        }
                    }
                });
            });
            return;
        }

        // Local PTY fallback only when `herdr-core` binary is unavailable.
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
            self.ui_dirty.store(true, Ordering::Release);
            return;
        }

        if let Ok(mut g) = self.state.lock() {
            g.closed_session_ids.remove(&id);
            g.did_auto_seed_local = true;
            g.status_notice = None;
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
        self.ensure_active_space();
        let (space_id, space_name, cwd_str, is_remote, machine_id, hc_path, loc) = {
            let Ok(g) = self.state.lock() else {
                return;
            };
            let Some(sp) = g.active_space() else {
                return;
            };
            (
                sp.id.clone(),
                sp.name.clone(),
                sp.path.clone(),
                sp.is_remote,
                sp.machine_id.clone(),
                g.hc_path.clone().or_else(discover_hc_binary),
                g.locale,
            )
        };

        if is_remote {
            if let Ok(host_id) = Uuid::parse_str(&machine_id) {
                self.mesh_hub
                    .spawn_remote_terminal_or_agent(host_id, agent_cmd, &cwd_str, &space_id);
            } else if let Ok(mut g) = self.state.lock() {
                g.status_notice = Some(
                    tr(
                        loc,
                        "Remote host ID invalid; open Mesh to reconnect.",
                        "远端主机 ID 无效，请打开节点面板重新连接。",
                    )
                    .to_string(),
                );
                self.ui_dirty.store(true, Ordering::Release);
            }
            return;
        }
        let cwd = if Path::new(&cwd_str).is_dir() {
            PathBuf::from(&cwd_str)
        } else {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"))
        };
        let effective_cwd_str = cwd.to_string_lossy().to_string();

        if let Some(hc_bin) = hc_path {
            if let Ok(mut g) = self.state.lock() {
                g.status_notice = Some(format!(
                    "{} {agent_cmd}...",
                    tr(loc, "Launching", "正在启动")
                ));
            }
            self.ui_dirty.store(true, Ordering::Release);
            let state_bg = Arc::clone(&self.state);
            let pty_mgr_bg = Arc::clone(&self.pty_mgr);
            let ui_dirty = Arc::clone(&self.ui_dirty);
            let agent = agent_cmd.to_string();
            let hc_name = format!("coda-{}", &Uuid::new_v4().simple().to_string()[..8]);
            std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    if let Ok(mut g) = state_bg.lock() {
                        g.status_notice = Some("Failed to init async runtime for hc".to_string());
                    }
                    ui_dirty.store(true, Ordering::Release);
                    return;
                };
                rt.block_on(async move {
                    match HerdrClient::connect_with_bin(hc_bin.clone()).await {
                        Ok(hc) => {
                            match hc
                                .launch_agent(
                                    &agent,
                                    Some(&hc_name),
                                    Some(&effective_cwd_str),
                                    None,
                                    120,
                                    36,
                                )
                                .await
                            {
                                Ok(term) => {
                                    let id = format!("hc-{}", term.address);
                                    let attach_res = pty_mgr_bg
                                        .lock()
                                        .map_err(|e| e.to_string())
                                        .and_then(|mut mgr| {
                                            mgr.spawn_herdr_attach(
                                                &id,
                                                &cwd,
                                                &hc_bin,
                                                &term.address,
                                                120,
                                                36,
                                            )
                                        });
                                    match attach_res {
                                        Ok(()) => {
                                            if let Ok(mut g) = state_bg.lock() {
                                                g.closed_session_ids.remove(&id);
                                                g.did_auto_seed_local = true;
                                                g.status_notice = None;
                                                if !g.sessions.iter().any(|s| s.id == id) {
                                                    g.sessions.push(TerminalSessionEntry {
                                                        id: id.clone(),
                                                        title: format!("{space_name} · {agent}"),
                                                        space_id,
                                                        cwd: effective_cwd_str,
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
                                                if g.layout.split_terminals
                                                    && g.focused_pane == SplitPane::Secondary
                                                {
                                                    g.split_session_id = Some(id);
                                                } else {
                                                    g.active_session_id = Some(id);
                                                }
                                                g.input_target = ActiveInputTarget::TerminalDirect;
                                            }
                                            ui_dirty.store(true, Ordering::Release);
                                        }
                                        Err(e) => {
                                            let _ = hc.kill_and_remove(&term.address).await;
                                            if let Ok(mut g) = state_bg.lock() {
                                                g.status_notice =
                                                    Some(format!("hc attach failed ({agent}): {e}"));
                                            }
                                            ui_dirty.store(true, Ordering::Release);
                                        }
                                    }
                                }
                                Err(e) => {
                                    if let Ok(mut g) = state_bg.lock() {
                                        g.status_notice =
                                            Some(format!("hc launch failed ({agent}): {e}"));
                                    }
                                    ui_dirty.store(true, Ordering::Release);
                                }
                            }
                        }
                        Err(e) => {
                            if let Ok(mut g) = state_bg.lock() {
                                g.status_notice =
                                    Some(format!("hc connect failed ({agent}): {e}"));
                            }
                            ui_dirty.store(true, Ordering::Release);
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
        let space_changed = g.active_space_id.as_deref() != Some(space_id.as_str());
        g.active_space_id = Some(space_id);
        if space_changed {
            g.current_browse_rel.clear();
            g.selected_file_rel = None;
            g.selected_file_content.clear();
            g.selected_change_path = None;
        }
        if g.layout.split_terminals && g.focused_pane == SplitPane::Secondary {
            g.split_session_id = Some(session_id.to_string());
        } else {
            g.active_session_id = Some(session_id.to_string());
        }
        g.input_target = ActiveInputTarget::TerminalDirect;
        drop(g);
        self.refresh_active_space_inspector();
    }

    fn select_space(&self, space_id: &str) {
        let mut need_spawn = false;
        if let Ok(mut g) = self.state.lock() {
            g.active_space_id = Some(space_id.to_string());
            g.current_browse_rel.clear();
            g.selected_file_rel = None;
            g.selected_file_content.clear();
            g.selected_change_path = None;
            let mut prefs = DesktopPreferences::load_or_default();
            prefs.selected_repo_path = Some(space_id.to_string());
            let _ = prefs.save();

            let first_sess = g
                .sessions
                .iter()
                .find(|s| s.space_id == space_id)
                .map(|s| s.id.clone());
            g.active_session_id = first_sess.clone();
            if g.layout.split_terminals {
                g.split_session_id = g
                    .sessions
                    .iter()
                    .find(|s| {
                        s.space_id == space_id && Some(&s.id) != g.active_session_id.as_ref()
                    })
                    .map(|s| s.id.clone());
            }
            if first_sess.is_none() {
                if let Some(sp) = g.spaces.iter().find(|s| s.id == space_id) {
                    need_spawn = !sp.is_remote || (sp.branch != "offline" && sp.path != ".");
                }
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
            if g.layout.split_terminals {
                if g.layout.layout == LayoutMode::Terminals {
                    g.layout.layout = LayoutMode::Workbench;
                }
                if g.split_session_id.is_none() {
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
                }
            } else {
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
        let meta = self.state.lock().ok().and_then(|mut g| {
            g.closed_session_ids.insert(session_id.to_string());
            g.did_auto_seed_local = true;
            g.sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| (s.remote_host_id.clone(), s.hc_addr.clone(), s.attached_hc))
        });

        if let Some((Some(host_str), Some(term_addr), _)) = meta {
            if let Ok(host_id) = Uuid::parse_str(&host_str) {
                self.mesh_hub.kill_remote_terminal(host_id, &term_addr);
            }
        } else if let Some((None, Some(term_addr), true)) = meta {
            let hc_bin = self
                .state
                .lock()
                .ok()
                .and_then(|g| g.hc_path.clone())
                .or_else(discover_hc_binary);
            let state_bg = Arc::clone(&self.state);
            let ui_dirty = Arc::clone(&self.ui_dirty);
            if let Some(bin) = hc_bin {
                std::thread::spawn(move || {
                    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        rt.block_on(async move {
                            match HerdrClient::connect_with_bin(bin).await {
                                Ok(hc) => {
                                    if let Err(e) = hc.kill_and_remove(&term_addr).await {
                                        if let Ok(mut g) = state_bg.lock() {
                                            g.status_notice =
                                                Some(format!("hc kill failed: {e}"));
                                        }
                                        ui_dirty.store(true, Ordering::Release);
                                    }
                                }
                                Err(e) => {
                                    if let Ok(mut g) = state_bg.lock() {
                                        g.status_notice = Some(format!("hc kill connect failed: {e}"));
                                    }
                                    ui_dirty.store(true, Ordering::Release);
                                }
                            }
                        });
                    }
                });
            }
        }

        if let Ok(mut mgr) = self.pty_mgr.lock() {
            mgr.kill_session(session_id);
        }
        if let Ok(mut g) = self.state.lock() {
            g.sessions.retain(|s| s.id != session_id);
            let active_space = g.active_space_id.clone();
            if g.active_session_id.as_deref() == Some(session_id) {
                g.active_session_id = active_space.as_ref().and_then(|sp_id| {
                    g.sessions
                        .iter()
                        .find(|s| &s.space_id == sp_id)
                        .map(|s| s.id.clone())
                });
            }
            if g.split_session_id.as_deref() == Some(session_id) {
                g.split_session_id = active_space.as_ref().and_then(|sp_id| {
                    g.sessions
                        .iter()
                        .find(|s| &s.space_id == sp_id && Some(&s.id) != g.active_session_id.as_ref())
                        .map(|s| s.id.clone())
                });
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn remove_space(&self, space_id: &str) {
        let session_ids: Vec<String> = self
            .state
            .lock()
            .ok()
            .map(|g| {
                g.sessions
                    .iter()
                    .filter(|s| s.space_id == space_id)
                    .map(|s| s.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for sid in session_ids {
            self.close_session(&sid);
        }

        if let Ok(mut g) = self.state.lock() {
            let removed_path = g
                .spaces
                .iter()
                .find(|s| s.id == space_id)
                .map(|s| s.path.clone());
            g.dismissed_spaces.insert(space_id.to_string());
            let mut prefs = DesktopPreferences::load_or_default();
            if !prefs.dismissed_spaces.iter().any(|x| x == space_id) {
                prefs.dismissed_spaces.push(space_id.to_string());
            }
            if let Some(ref rp) = removed_path {
                prefs.workspace_roots.retain(|r| r != rp);
            }
            if prefs.selected_repo_path.as_deref() == Some(space_id) {
                prefs.selected_repo_path = None;
            }
            let _ = prefs.save();

            g.spaces.retain(|s| s.id != space_id);
            if g.active_space_id.as_deref() == Some(space_id) {
                let next_sp = g.spaces.first().map(|s| s.id.clone());
                g.active_space_id = next_sp.clone();
                g.current_browse_rel.clear();
                g.selected_file_rel = None;
                g.selected_file_content.clear();
                g.active_session_id = next_sp.as_ref().and_then(|nsp| {
                    g.sessions
                        .iter()
                        .find(|s| &s.space_id == nsp)
                        .map(|s| s.id.clone())
                });
                if g.layout.split_terminals {
                    g.split_session_id = next_sp.as_ref().and_then(|nsp| {
                        g.sessions
                            .iter()
                            .find(|s| {
                                &s.space_id == nsp && Some(&s.id) != g.active_session_id.as_ref()
                            })
                            .map(|s| s.id.clone())
                    });
                }
            }
        }
        self.refresh_active_space_inspector();
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn open_workspace_entry(&self, rel_path: &str, is_dir: bool) {
        let clean_rel = rel_path.trim_matches('/').to_string();
        let info = self.state.lock().ok().and_then(|g| {
            let sp = g.active_space()?;
            let host_uuid = if sp.is_remote {
                Uuid::parse_str(&sp.machine_id).ok()
            } else {
                None
            };
            Some((sp.id.clone(), sp.path.clone(), host_uuid))
        });
        let Some((space_id, space_path_str, remote_host_id)) = info else {
            return;
        };

        if let Some(host_id) = remote_host_id {
            if is_dir {
                if let Ok(mut g) = self.state.lock() {
                    g.current_browse_rel = clean_rel.clone();
                    g.selected_file_rel = None;
                    g.selected_file_content.clear();
                }
                self.mesh_hub
                    .refresh_remote_directory(host_id, &space_id, &space_path_str, &clean_rel);
            } else {
                let full_path = if clean_rel.is_empty() {
                    space_path_str.clone()
                } else {
                    format!("{}/{}", space_path_str.trim_end_matches('/'), clean_rel)
                };
                if let Ok(mut g) = self.state.lock() {
                    g.selected_file_rel = Some(clean_rel.clone());
                    g.right_drawer = Some(RightDrawerTab::Files);
                }
                self.mesh_hub
                    .open_remote_file_preview(host_id, &full_path, &clean_rel);
            }
            self.ui_dirty.store(true, Ordering::Release);
            return;
        }

        let base_path = PathBuf::from(&space_path_str);
        let full = if clean_rel.is_empty() {
            base_path.clone()
        } else {
            base_path.join(&clean_rel)
        };
        if is_dir || full.is_dir() {
            let entries = list_workspace_dir(&base_path, &full);
            if let Ok(mut g) = self.state.lock() {
                g.current_browse_rel = clean_rel;
                g.dir_entries = entries;
                g.selected_file_rel = None;
                g.selected_file_content.clear();
            }
        } else if full.is_file() {
            let content = read_workspace_file_preview(&full, 120_000);
            if let Ok(mut g) = self.state.lock() {
                g.selected_file_rel = Some(clean_rel);
                g.selected_file_content = content;
                g.right_drawer = Some(RightDrawerTab::Files);
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn navigate_parent_dir(&self) {
        let parent_rel = {
            let Ok(g) = self.state.lock() else {
                return;
            };
            let cur = g.current_browse_rel.trim_matches('/');
            if cur.is_empty() {
                return;
            }
            match cur.rsplit_once('/') {
                Some((parent, _)) => parent.to_string(),
                None => String::new(),
            }
        };
        self.open_workspace_entry(&parent_rel, true);
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
            g.dismissed_spaces.remove(&space_id);
            g.dismissed_spaces.remove(&path_str);
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
            prefs
                .dismissed_spaces
                .retain(|x| x != &space_id && x != &path_str);
            if !prefs.workspace_roots.contains(&path_str) {
                prefs.workspace_roots.push(path_str);
            }
            let _ = prefs.save();
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
            PaletteAction::OpenSettings => {
                if let Ok(mut g) = self.state.lock() {
                    g.settings_open = true;
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
        let next = self.state.lock().ok().map(|g| g.locale.toggle());
        if let Some(locale) = next {
            self.set_locale(locale);
        }
    }

    fn set_locale(&self, locale: UiLocale) {
        if let Ok(mut g) = self.state.lock() {
            g.locale = locale;
            let mut prefs = DesktopPreferences::load_or_default();
            prefs.locale = locale;
            let _ = prefs.save();
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn toggle_settings_modal(&self) {
        if let Ok(mut g) = self.state.lock() {
            g.settings_open = !g.settings_open;
            if g.settings_open {
                g.palette_open = false;
            }
            g.input_target = ActiveInputTarget::TerminalDirect;
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn save_server_url_preference(&self, url: &str) {
        let trimmed = url.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        if let Ok(mut g) = self.state.lock() {
            g.server_url = trimmed.clone();
            g.input_target = ActiveInputTarget::TerminalDirect;
        }
        let mut prefs = DesktopPreferences::load_or_default();
        prefs.server_url = trimmed;
        let _ = prefs.save();
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn restore_dismissed_spaces(&self) {
        let mut prefs = DesktopPreferences::load_or_default();
        prefs.dismissed_spaces.clear();
        let _ = prefs.save();

        let candidates = discover_local_git_repositories(&prefs.workspace_roots);
        if let Ok(mut g) = self.state.lock() {
            g.dismissed_spaces.clear();
            let machine_label = g.local_machine_name.clone();
            for path_str in candidates {
                let space_id = format!("local:{path_str}");
                if g.spaces.iter().any(|s| s.id == space_id) {
                    continue;
                }
                let p = PathBuf::from(&path_str);
                let snap = compute_repo_snapshot(&p, None);
                let name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("workspace")
                    .to_string();
                g.spaces.push(SpaceEntry {
                    id: space_id,
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
            if g.active_space_id.is_none() {
                g.active_space_id = g.spaces.first().map(|s| s.id.clone());
            }
        }
        self.refresh_active_space_inspector();
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
            .on_action(cx.listener(|this, _: &CloseActiveTerminal, _, cx| {
                let target_id = this
                    .state
                    .lock()
                    .ok()
                    .and_then(|g| g.focused_session_id().map(|s| s.to_string()));
                if let Some(id) = target_id {
                    this.close_session(&id);
                }
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
                        g.settings_open = false;
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
            .on_action(cx.listener(|this, _: &InterruptActiveTerminal, _, cx| {
                let target_id = this
                    .state
                    .lock()
                    .ok()
                    .and_then(|g| g.focused_session_id().map(|s| s.to_string()));
                if let Some(id) = target_id {
                    this.interrupt_session(&id);
                }
                cx.notify();
            }))
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .text_color(rgb(palette::TEXT_PRIMARY))
            .flex()
            .flex_col()
            .overflow_hidden()
            .relative()
            .child(self.render_window_titlebar(&st, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
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
                    ),
            )
            .when(st.palette_open, |el| {
                el.child(self.render_command_palette_overlay(&st, cx))
            })
            .when(st.settings_open, |el| {
                el.child(self.render_settings_modal_overlay(&st, cx))
            })
    }
}

impl CodaGpuWorkspaceView {
    fn render_window_titlebar(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let active_label = st
            .active_space()
            .map(|sp| format!("{} · {}", sp.machine_label, sp.name))
            .unwrap_or_else(|| tr(loc, "No workspace selected", "未选择工作区").to_string());
        let online_hosts = st.remote_hosts.iter().filter(|h| h.is_online).count();

        div()
            .h(px(28.0))
            .w_full()
            .flex_shrink_0()
            .px(px(12.0))
            .when(cfg!(target_os = "macos"), |el| el.pl(px(78.0)))
            .bg(rgb(palette::SIDEBAR_BG))
            .border_b_1()
            .border_color(rgb(palette::BORDER))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.0))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().w(px(6.0)).h(px(6.0)).rounded(px(99.0)).bg(rgb(accent)))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child("Coda"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(palette::TEXT_DIM))
                            .truncate()
                            .child(format!("· {active_label}")),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when_some(st.status_notice.clone(), |el, note| {
                        el.child(
                            div()
                                .px(px(8.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(palette::SURFACE_BG))
                                .border_1()
                                .border_color(rgb(palette::NEEDS_YOU_AMBER))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .max_w(px(360.0))
                                        .truncate()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::NEEDS_YOU_AMBER))
                                        .child(note),
                                )
                                .child(
                                    div()
                                        .id("btn-dismiss-notice")
                                        .occlude()
                                        .px(px(3.0))
                                        .rounded(px(3.0))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child("×")
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            if let Ok(mut g) = this.state.lock() {
                                                g.status_notice = None;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(if st.hc_available {
                                palette::DONE_GREEN
                            } else {
                                palette::TEXT_DIM
                            }))
                            .child(if st.hc_available {
                                format!("hc ● · mesh {online_hosts}")
                            } else {
                                format!("pty · mesh {online_hosts}")
                            }),
                    ),
            )
    }

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
                // Sidebar header strip (36px)
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
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
                            .child(div().w(px(7.0)).h(px(7.0)).rounded(px(99.0)).bg(rgb(accent)))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(tr(loc, "Workspaces", "工作台")),
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
                                    .occlude()
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
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.palette_open = true;
                                            g.palette_query.clear();
                                            g.palette_selected = 0;
                                            g.input_target = ActiveInputTarget::CommandPalette;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("btn-collapse-sidebar")
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .cursor_pointer()
                                    .child("◧")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.layout.sidebar_visible = false;
                                        }
                                        this.persist_layout_config();
                                        cx.notify();
                                    })),
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
                                            .occlude()
                                            .px(px(5.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .text_size(px(12.0))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .cursor_pointer()
                                            .child("+")
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                this.add_workspace_folder();
                                                cx.notify();
                                            })),
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
                                        let sp_id_close = sp.id.clone();
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
                                            .occlude()
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
                                                    .flex_shrink_0()
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
                                                    })
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "space-close-{}",
                                                                sp.id
                                                            )))
                                                            .occlude()
                                                            .px(px(3.0))
                                                            .rounded(px(3.0))
                                                            .hover(|h| {
                                                                h.bg(rgb(palette::SURFACE_BG))
                                                                    .text_color(rgb(
                                                                        palette::TEXT_PRIMARY,
                                                                    ))
                                                            })
                                                            .text_size(px(10.5))
                                                            .text_color(rgb(palette::TEXT_DIM))
                                                            .child("×")
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                |_, _, cx| cx.stop_propagation(),
                                                            )
                                                            .on_click(cx.listener(
                                                                move |this, _: &ClickEvent, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.remove_space(&sp_id_close);
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    ),
                                            )
                                            .on_click(cx.listener(
                                                move |this, _: &ClickEvent, _, cx| {
                                                    this.select_space(&sp_id);
                                                    cx.notify();
                                                },
                                            ))
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
                                let sid_close = sess.id.clone();
                                let sp_name = st
                                    .spaces
                                    .iter()
                                    .find(|s| s.id == sess.space_id)
                                    .map(|s| s.name.as_str())
                                    .unwrap_or("local")
                                    .to_string();

                                div()
                                    .id(SharedString::from(format!("agent-row-{}", sess.id)))
                                    .occlude()
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
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(10.0))
                                                            .text_color(rgb(palette::TEXT_DIM))
                                                            .child(sess.elapsed_label()),
                                                    )
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "agent-close-{}",
                                                                sess.id
                                                            )))
                                                            .occlude()
                                                            .px(px(3.0))
                                                            .rounded(px(3.0))
                                                            .hover(|h| {
                                                                h.bg(rgb(palette::SURFACE_BG))
                                                                    .text_color(rgb(
                                                                        palette::TEXT_PRIMARY,
                                                                    ))
                                                            })
                                                            .text_size(px(10.5))
                                                            .text_color(rgb(palette::TEXT_DIM))
                                                            .child("×")
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                |_, _, cx| cx.stop_propagation(),
                                                            )
                                                            .on_click(cx.listener(
                                                                move |this, _: &ClickEvent, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.close_session(&sid_close);
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    ),
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
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.select_session(&sid);
                                        cx.notify();
                                    }))
                            })),
                    ),
            )
            .child(
                // Sidebar footer: + Add machine + Settings (Herdr-style)
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .px(px(8.0))
                    .border_t_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(4.0))
                    .child(
                        div()
                            .id("btn-add-machine")
                            .occlude()
                            .min_w_0()
                            .flex_1()
                            .px(px(6.0))
                            .py(px(4.0))
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
                            .truncate()
                            .child(if st.is_logged_in {
                                format!("● {}", st.account_email)
                            } else {
                                tr(loc, "+ Add machine", "+ 添加远端主机").to_string()
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.right_drawer = match g.right_drawer {
                                        Some(RightDrawerTab::Mesh) => None,
                                        _ => Some(RightDrawerTab::Mesh),
                                    };
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("btn-open-settings")
                            .occlude()
                            .flex_shrink_0()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(5.0))
                            .bg(rgb(if st.settings_open {
                                palette::HOVER_BG
                            } else {
                                palette::SURFACE_BG
                            }))
                            .border_1()
                            .border_color(rgb(if st.settings_open {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if st.settings_open {
                                palette::TEXT_PRIMARY
                            } else {
                                palette::TEXT_MUTED
                            }))
                            .child(tr(loc, "⚙ Settings", "⚙ 设置"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.toggle_settings_modal();
                                cx.notify();
                            })),
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
                                .occlude()
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                .text_size(px(11.5))
                                .text_color(rgb(palette::TEXT_MUTED))
                                .cursor_pointer()
                                .child("◧")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.layout.sidebar_visible = true;
                                    }
                                    this.persist_layout_config();
                                    cx.notify();
                                })),
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
                            .occlude()
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
                                    .occlude()
                                    .px(px(3.0))
                                    .rounded(px(3.0))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .hover(|h| {
                                        h.bg(rgb(palette::SURFACE_BG))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                    })
                                    .child("×")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.close_session(&sid_close);
                                        cx.notify();
                                    })),
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.select_session(&sid);
                                cx.notify();
                            }))
                    }))
                    .child(
                        div()
                            .id("btn-new-terminal")
                            .occlude()
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
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.spawn_terminal_in_active_space(None);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("btn-launch-agent")
                            .occlude()
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
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.palette_open = true;
                                    g.palette_query = "agent".to_string();
                                    g.palette_selected = 0;
                                    g.input_target = ActiveInputTarget::CommandPalette;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child({
                // Right side: Split + Divider + Herdr-style Side Panel toggles (Files / Changes / Nodes)
                let online_hosts = st.remote_hosts.iter().filter(|h| h.is_online).count();
                let nodes_label = if online_hosts > 0 {
                    format!("{} ({online_hosts})", tr(loc, "Nodes", "节点"))
                } else {
                    tr(loc, "Nodes", "节点").to_string()
                };
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("btn-split-toggle")
                            .occlude()
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
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.toggle_split_terminals();
                                cx.notify();
                            })),
                    )
                    .child(div().w(px(1.0)).h(px(16.0)).bg(rgb(palette::BORDER)))
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
                    ))
                    .child(self.render_drawer_toggle_btn(
                        st,
                        RightDrawerTab::Mesh,
                        &nodes_label,
                        cx,
                    ))
                    .child(
                        div()
                            .id("btn-topbar-settings")
                            .occlude()
                            .px(px(7.0))
                            .py(px(4.0))
                            .rounded(px(5.0))
                            .bg(rgb(if st.settings_open {
                                palette::HOVER_BG
                            } else {
                                palette::SURFACE_BG
                            }))
                            .border_1()
                            .border_color(rgb(if st.settings_open {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if st.settings_open {
                                palette::TEXT_PRIMARY
                            } else {
                                palette::TEXT_MUTED
                            }))
                            .child("⚙")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.toggle_settings_modal();
                                cx.notify();
                            })),
                    )
            })
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
            .occlude()
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
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.right_drawer = if g.right_drawer == Some(tab) {
                        None
                    } else {
                        Some(tab)
                    };
                }
                cx.notify();
            }))
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
        let font_size_px = st.layout.terminal_font_size_px.clamp(10.0, 22.0);
        let font_size = px(font_size_px);
        let line_h = (font_size_px * 1.32).round();

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
                        .occlude()
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
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.focused_pane = pane_slot;
                            }
                            this.spawn_terminal_in_active_space(None);
                            cx.notify();
                        })),
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
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
                    .children(
                        visible_slice
                            .iter()
                            .map(|line| render_styled_terminal_line(line, line_h)),
                    ),
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
                                        .occlude()
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
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _, cx| {
                                                cx.stop_propagation();
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
                                            },
                                        )),
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
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child("^C")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.interrupt_session(&sid_sigint);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("clear-{pane_slot:?}")))
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(tr(loc, "Clear", "清屏"))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
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
                                    })),
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
        let font_size_px = (st.layout.terminal_font_size_px - 1.0).clamp(10.0, 20.0);
        let font_size = px(font_size_px);
        let tile_line_h = (font_size_px * 1.32).round();

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
                let sid_close = sess.id.clone();
                let is_active = st.active_session_id.as_deref() == Some(&sess.id);
                div()
                    .id(SharedString::from(format!("tile-{}", sess.id)))
                    .occlude()
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
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Ok(mut g) = this.state.lock() {
                            g.active_session_id = Some(sid.clone());
                            g.focused_pane = SplitPane::Primary;
                            g.input_target = ActiveInputTarget::TerminalDirect;
                        }
                        cx.notify();
                    }))
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
                                            .bg(rgb(sess.effective_state().color_u32())),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .truncate()
                                            .text_size(px(11.5))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(sess.title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(sess.elapsed_label()),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "tile-close-{}",
                                                sess.id
                                            )))
                                            .occlude()
                                            .px(px(4.0))
                                            .rounded(px(3.0))
                                            .hover(|h| {
                                                h.bg(rgb(palette::SURFACE_BG))
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                            })
                                            .cursor_pointer()
                                            .text_size(px(11.0))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child("×")
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(
                                                move |this, _: &ClickEvent, _, cx| {
                                                    cx.stop_propagation();
                                                    this.close_session(&sid_close);
                                                    cx.notify();
                                                },
                                            )),
                                    ),
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
                                    .map(|line| render_styled_terminal_line(line, tile_line_h))
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

    fn render_right_drawer(
        &self,
        st: &CodaDesktopState,
        tab: RightDrawerTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let active_space = st.active_space();
        let active_path = active_space
            .map(|s| PathBuf::from(&s.path))
            .unwrap_or_else(|| PathBuf::from("."));
        let is_local_space = active_space.map(|s| !s.is_remote).unwrap_or(false);
        let space_name = active_space
            .map(|s| s.name.as_str())
            .unwrap_or("workspace")
            .to_string();
        let online_hosts = st.remote_hosts.iter().filter(|h| h.is_online).count();

        let (header_title, header_sub): (String, Option<String>) = match tab {
            RightDrawerTab::Files => {
                if let Some(ref rel) = st.selected_file_rel {
                    (rel.clone(), None)
                } else {
                    let sub = if st.current_browse_rel.is_empty() {
                        space_name
                    } else {
                        format!("{space_name}/{}", st.current_browse_rel)
                    };
                    (tr(loc, "Files", "文件").to_string(), Some(sub))
                }
            }
            RightDrawerTab::Changes => {
                let sub = if let Some(ref ch) = st.selected_change_path {
                    ch.clone()
                } else {
                    format!(
                        "{} · {}",
                        st.repo_snapshot.branch,
                        st.repo_snapshot.changed_files.len()
                    )
                };
                (tr(loc, "Changes", "变更").to_string(), Some(sub))
            }
            RightDrawerTab::Mesh => (
                tr(loc, "Remote Nodes", "远端节点").to_string(),
                Some(format!("{online_hosts}/{}", st.remote_hosts.len())),
            ),
        };

        let show_back = match tab {
            RightDrawerTab::Files => st.selected_file_rel.is_some(),
            RightDrawerTab::Changes => st.selected_change_path.is_some(),
            RightDrawerTab::Mesh => false,
        };

        div()
            .w(px(368.0))
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
                    .flex_shrink_0()
                    .px(px(10.0))
                    .border_b_1()
                    .border_color(rgb(palette::BORDER))
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
                            .gap(px(6.0))
                            .when(show_back, |el| {
                                el.child(
                                    div()
                                        .id("btn-drawer-back")
                                        .occlude()
                                        .px(px(5.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(11.5))
                                        .text_color(rgb(accent))
                                        .child("←")
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            match tab {
                                                RightDrawerTab::Files => {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.selected_file_rel = None;
                                                        g.selected_file_content.clear();
                                                    }
                                                }
                                                RightDrawerTab::Changes => {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.selected_change_path = None;
                                                    }
                                                    this.refresh_active_space_inspector();
                                                }
                                                RightDrawerTab::Mesh => {}
                                            }
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .truncate()
                                    .child(header_title),
                            )
                            .when_some(header_sub, |el, sub| {
                                el.child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::TEXT_DIM))
                                        .truncate()
                                        .child(sub),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .when(tab != RightDrawerTab::Mesh && is_local_space, |el| {
                                let path_for_finder = active_path.to_string_lossy().to_string();
                                el.child(
                                    div()
                                        .id("btn-reveal-finder")
                                        .occlude()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child("Finder")
                                        .on_click(cx.listener(move |_, _: &ClickEvent, _, _| {
                                            reveal_in_finder(&path_for_finder);
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("btn-close-drawer")
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child("×")
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        if let Ok(mut g) = this.state.lock() {
                                            g.right_drawer = None;
                                        }
                                        cx.notify();
                                    })),
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
                        if snap.changed_files.is_empty() {
                            el.child(
                                div()
                                    .p(px(10.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .text_size(px(11.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(tr(
                                        loc,
                                        "Working tree clean · no uncommitted changes",
                                        "工作区干净 · 暂无未提交变更",
                                    )),
                            )
                        } else {
                            let diff_lines: Vec<String> = extract_file_diff_lines(
                                &snap.diff_text,
                                st.selected_change_path.as_deref(),
                            )
                            .into_iter()
                            .map(str::to_string)
                            .collect();
                            el.children(snap.changed_files.iter().take(40).map(|f| {
                                let path_rel = f.path.clone();
                                let is_sel =
                                    st.selected_change_path.as_deref() == Some(f.path.as_str());
                                div()
                                    .id(SharedString::from(format!("change-file-{}", f.path)))
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(if is_sel {
                                        palette::HOVER_BG
                                    } else {
                                        palette::SIDEBAR_BG
                                    }))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
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
                                            .min_w_0()
                                            .flex_1()
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .truncate()
                                            .child(f.path.clone()),
                                    )
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.selected_change_path = if g.selected_change_path
                                                .as_deref()
                                                == Some(path_rel.as_str())
                                            {
                                                None
                                            } else {
                                                Some(path_rel.clone())
                                            };
                                        }
                                        this.refresh_active_space_inspector();
                                        cx.notify();
                                    }))
                            }))
                            .child(div().h(px(1.0)).w_full().bg(rgb(palette::BORDER)))
                            .when(diff_lines.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(palette::TEXT_DIM))
                                        .child(tr(loc, "No diff text available", "暂无 Diff 内容")),
                                )
                            })
                            .when(!diff_lines.is_empty(), |d| {
                                d.child(
                                    div()
                                        .font_family("Menlo")
                                        .text_size(px(11.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(1.5))
                                        .children(diff_lines.into_iter().map(|line| {
                                            let color = if line.starts_with('+') {
                                                palette::DONE_GREEN
                                            } else if line.starts_with('-') {
                                                palette::ERROR_RED
                                            } else if line.starts_with("@@") {
                                                palette::WORKING_SKY
                                            } else {
                                                palette::TEXT_MUTED
                                            };
                                            div().text_color(rgb(color)).truncate().child(line)
                                        })),
                                )
                            })
                        }
                        .when(
                            !snap.recent_commits.is_empty() && st.selected_change_path.is_none(),
                            |el| {
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
                            },
                        )
                    })
                    .when(tab == RightDrawerTab::Files, |el| {
                        if st.selected_file_rel.is_some() {
                            if st.selected_file_content.is_empty() {
                                el.child(
                                    div()
                                        .p(px(10.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .text_size(px(11.5))
                                        .text_color(rgb(palette::TEXT_DIM))
                                        .child(tr(loc, "Empty file", "文件内容为空")),
                                )
                            } else {
                                let file_lines: Vec<String> = st
                                    .selected_file_content
                                    .lines()
                                    .take(250)
                                    .map(str::to_string)
                                    .collect();
                                el.child(
                                    div()
                                        .font_family("Menlo")
                                        .text_size(px(11.5))
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .children(file_lines.into_iter().enumerate().map(
                                            |(idx, line)| {
                                                div()
                                                    .flex()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .w(px(34.0))
                                                            .flex_shrink_0()
                                                            .text_color(rgb(palette::TEXT_DIM))
                                                            .child(format!("{:>4}", idx + 1)),
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                                            .child(line),
                                                    )
                                            },
                                        )),
                                )
                            }
                        } else {
                            el.when(!st.current_browse_rel.is_empty(), |el| {
                                el.child(
                                    div()
                                        .id("dir-up-drawer")
                                        .occlude()
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(4.0))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(11.5))
                                        .text_color(rgb(accent))
                                        .child("../")
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.navigate_parent_dir();
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(st.dir_entries.is_empty(), |el| {
                                el.child(
                                    div()
                                        .p(px(10.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .text_size(px(11.5))
                                        .text_color(rgb(palette::TEXT_DIM))
                                        .child(tr(
                                            loc,
                                            "No files in this directory",
                                            "当前目录暂无文件",
                                        )),
                                )
                            })
                            .children(st.dir_entries.iter().take(80).map(|entry| {
                                let rel = entry.rel_path.clone();
                                let is_dir = entry.is_dir;
                                div()
                                    .id(SharedString::from(format!("drawer-file-{}", entry.rel_path)))
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(11.5))
                                    .text_color(rgb(if is_dir {
                                        accent
                                    } else {
                                        palette::TEXT_PRIMARY
                                    }))
                                    .truncate()
                                    .child(if is_dir {
                                        format!("{}/", entry.name)
                                    } else {
                                        entry.name.clone()
                                    })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.open_workspace_entry(&rel, is_dir);
                                        cx.notify();
                                    }))
                            }))
                        }
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
        let show_auth_form = !st.is_logged_in || st.auth_form_expanded;
        el.when(!show_auth_form, |el| {
            el.child(
                div()
                    .p(px(9.0))
                    .rounded(px(6.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
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
                                            .bg(rgb(palette::DONE_GREEN)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .truncate()
                                            .child(st.account_email.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .truncate()
                                    .child(st.server_url.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("btn-expand-auth")
                            .occlude()
                            .flex_shrink_0()
                            .px(px(7.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .bg(rgb(palette::HOVER_BG))
                            .hover(|h| h.bg(rgb(palette::CHROME_BG)))
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .text_color(rgb(palette::TEXT_MUTED))
                            .child(tr(loc, "Switch", "切换账号"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.auth_form_expanded = true;
                                }
                                cx.notify();
                            })),
                    ),
            )
        })
        .when(show_auth_form, |el| {
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
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tr(loc, "Coda Cloud Mesh", "Coda 加密算力网络")),
                            )
                            .when(st.is_logged_in, |h| {
                                h.child(
                                    div()
                                        .id("btn-collapse-auth")
                                        .occlude()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .hover(|hv| hv.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child(tr(loc, "Collapse", "收起"))
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.auth_form_expanded = false;
                                                g.input_target = ActiveInputTarget::TerminalDirect;
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("input-auth-server")
                            .occlude()
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
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthServerUrl;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("btn-browser-signin")
                            .occlude()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::HOVER_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER_STRONG))
                            .hover(|h| h.bg(rgb(palette::SURFACE_BG)))
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child(tr(loc, "Sign in via Browser (RFC 8628)", "浏览器一键授权登录"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                let srv = this
                                    .state
                                    .lock()
                                    .map(|g| g.server_url.clone())
                                    .unwrap_or_default();
                                spawn_device_login(
                                    Arc::clone(&this.state),
                                    Arc::clone(&this.ui_dirty),
                                    Arc::clone(&this.mesh_hub),
                                    srv,
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("input-auth-email")
                            .occlude()
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
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthAccount;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("input-auth-pass")
                            .occlude()
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
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthPassword;
                                }
                                cx.notify();
                            })),
                    )
                    .when_some(st.device_login_code.clone(), |el, code| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(palette::WORKING_SKY))
                                .child(format!("Code: {code}")),
                        )
                    }),
            )
        })
        .child(
            div()
                .text_size(px(10.5))
                .text_color(rgb(palette::TEXT_DIM))
                .child(format!(
                    "{} ({})",
                    tr(loc, "ONLINE NODES", "在线节点"),
                    st.remote_hosts.len()
                )),
        )
        .when(st.remote_hosts.is_empty(), |el| {
            el.child(
                div()
                    .p(px(10.0))
                    .rounded(px(6.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .text_size(px(11.5))
                    .text_color(rgb(palette::TEXT_DIM))
                    .child(tr(loc, "No remote daemons online yet", "暂无在线远端主机")),
            )
        })
        .children(st.remote_hosts.iter().map(|host| {
            let host_clone = host.clone();
            div()
                .id(SharedString::from(format!("mesh-host-{}", host.id)))
                .occlude()
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
                .gap(px(8.0))
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
                                .flex_shrink_0()
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
                                .truncate()
                                .child(host.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(palette::TEXT_DIM))
                                .truncate()
                                .child(format!("{} · {}", host.platform, host.role)),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(10.5))
                        .text_color(rgb(st.layout.accent_color_u32()))
                        .child(tr(loc, "Open", "进入")),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.mesh_hub.probe_host(host_clone.clone());
                    cx.notify();
                }))
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
            .occlude()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000099))
            .pt(px(72.0))
            .flex()
            .justify_center()
            .items_start()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.palette_open = false;
                    g.input_target = ActiveInputTarget::TerminalDirect;
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("palette-modal")
                    .occlude()
                    .w(px(540.0))
                    .rounded(px(10.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER_STRONG))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(|_, _, cx| {
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
                                    .occlude()
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
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.execute_palette_action(action.clone());
                                            cx.notify();
                                        },
                                    ))
                            })),
                    ),
            )
    }

    fn render_settings_modal_overlay(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let active_sec = st.settings_section;
        let section_title = match active_sec {
            SettingsSection::Appearance => tr(loc, "Appearance & Layout", "外观与布局"),
            SettingsSection::Runtime => tr(loc, "Terminals & Spaces", "终端与工作区"),
            SettingsSection::Machines => tr(loc, "Account & Remote Nodes", "账号与远端节点"),
            SettingsSection::Shortcuts => tr(loc, "Keyboard Shortcuts", "快捷键"),
        };

        let nav_items: &[(SettingsSection, &str, &str)] = &[
            (SettingsSection::Appearance, "Appearance", "外观与布局"),
            (SettingsSection::Runtime, "Terminals & Spaces", "终端与工作区"),
            (SettingsSection::Machines, "Account & Nodes", "账号与远端节点"),
            (SettingsSection::Shortcuts, "Shortcuts", "快捷键"),
        ];

        let content = match active_sec {
            SettingsSection::Appearance => {
                self.render_settings_appearance_pane(st, cx).into_any_element()
            }
            SettingsSection::Runtime => {
                self.render_settings_runtime_pane(st, cx).into_any_element()
            }
            SettingsSection::Machines => {
                self.render_settings_machines_pane(st, cx).into_any_element()
            }
            SettingsSection::Shortcuts => self.render_settings_shortcuts_pane(st).into_any_element(),
        };

        div()
            .id("settings-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .bg(rgba(0x000000aa))
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.settings_open = false;
                    g.input_target = ActiveInputTarget::TerminalDirect;
                }
                cx.notify();
            }))
            .child(
                div()
                    .id("settings-modal")
                    .occlude()
                    .w(px(720.0))
                    .h(px(500.0))
                    .rounded(px(10.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER_STRONG))
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        // Left navigation rail (184px)
                        div()
                            .w(px(184.0))
                            .h_full()
                            .flex_shrink_0()
                            .bg(rgb(palette::SIDEBAR_BG))
                            .border_r_1()
                            .border_color(rgb(palette::BORDER))
                            .p(px(10.0))
                            .flex()
                            .flex_col()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .child(tr(loc, "Settings", "设置")),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(palette::TEXT_DIM))
                                                    .child("⌘,"),
                                            ),
                                    )
                                    .children(nav_items.iter().map(|&(sec, en, zh)| {
                                        let is_active = active_sec == sec;
                                        div()
                                            .id(SharedString::from(format!("settings-nav-{sec:?}")))
                                            .occlude()
                                            .px(px(9.0))
                                            .py(px(6.0))
                                            .rounded(px(6.0))
                                            .bg(rgb(if is_active {
                                                palette::HOVER_BG
                                            } else {
                                                palette::SIDEBAR_BG
                                            }))
                                            .border_1()
                                            .border_color(rgb(if is_active {
                                                accent
                                            } else {
                                                palette::SIDEBAR_BG
                                            }))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .cursor_pointer()
                                            .text_size(px(12.0))
                                            .text_color(rgb(if is_active {
                                                palette::TEXT_PRIMARY
                                            } else {
                                                palette::TEXT_MUTED
                                            }))
                                            .child(tr(loc, en, zh))
                                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.settings_section = sec;
                                                    g.input_target =
                                                        ActiveInputTarget::TerminalDirect;
                                                }
                                                cx.notify();
                                            }))
                                    })),
                            )
                            .child(
                                div()
                                    .id("btn-settings-open-config-dir")
                                    .occlude()
                                    .px(px(8.0))
                                    .py(px(6.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(tr(
                                        loc,
                                        "Open ~/.config/coda",
                                        "打开 ~/.config/coda",
                                    ))
                                    .on_click(cx.listener(|_, _: &ClickEvent, _, _| {
                                        if let Some(dir) = config_root_dir() {
                                            let _ = std::fs::create_dir_all(&dir);
                                            reveal_in_finder(&dir.to_string_lossy());
                                        }
                                    })),
                            ),
                    )
                    .child(
                        // Right section content column
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(42.0))
                                    .flex_shrink_0()
                                    .px(px(16.0))
                                    .border_b_1()
                                    .border_color(rgb(palette::BORDER))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(10.0))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                                    .child(section_title),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(palette::TEXT_DIM))
                                                    .child(tr(
                                                        loc,
                                                        "Changes save & apply immediately",
                                                        "更改自动保存并实时生效",
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("btn-close-settings")
                                            .occlude()
                                            .w(px(24.0))
                                            .h(px(24.0))
                                            .rounded(px(5.0))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(14.0))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child("×")
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.settings_open = false;
                                                    g.input_target =
                                                        ActiveInputTarget::TerminalDirect;
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p(px(14.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .overflow_hidden()
                                    .child(content),
                            ),
                    ),
            )
    }

    fn render_settings_appearance_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let font_px = st.layout.terminal_font_size_px.clamp(10.0, 22.0);
        let sidebar_w_px = st.layout.sidebar_width_px.clamp(200.0, 320.0);

        let accent_swatches: &[(&str, u32)] = &[
            ("#38bdf8", 0x38bdf8),
            ("#22c55e", 0x22c55e),
            ("#a78bfa", 0xa78bfa),
            ("#f59e0b", 0xf59e0b),
            ("#f472b6", 0xf472b6),
            ("#2dd4bf", 0x2dd4bf),
        ];
        let font_choices: &[&str] = &["Menlo", "SF Mono", "JetBrains Mono", "Monaco"];

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(render_settings_row(
                tr(loc, "Language", "界面语言"),
                tr(loc, "Desktop UI localization", "切换桌面客户端显示语言"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(self.render_setting_pill(
                        "settings-lang-zh",
                        "中文",
                        loc == UiLocale::Zh,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.set_locale(UiLocale::Zh);
                            cx.notify();
                        }),
                    ))
                    .child(self.render_setting_pill(
                        "settings-lang-en",
                        "English",
                        loc == UiLocale::En,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.set_locale(UiLocale::En);
                            cx.notify();
                        }),
                    )),
            ))
            .child(render_settings_row(
                tr(loc, "Workspace Layout", "默认主视图"),
                tr(
                    loc,
                    "Main workbench or terminals grid",
                    "选择专注终端工作台或平铺网格",
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(self.render_setting_pill(
                        "settings-layout-workbench",
                        tr(loc, "Main", "主工作台"),
                        st.layout.layout == LayoutMode::Workbench,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.layout = LayoutMode::Workbench;
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    ))
                    .child(self.render_setting_pill(
                        "settings-layout-terminals",
                        tr(loc, "Terminals", "终端平铺"),
                        st.layout.layout == LayoutMode::Terminals,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.layout = LayoutMode::Terminals;
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    )),
            ))
            .child(render_settings_row(
                tr(loc, "Accent Color", "主题强调色"),
                tr(loc, "Active indicators & focus rings", "用于高亮标签、边框和活动指示器"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .children(accent_swatches.iter().map(|&(hex, color_val)| {
                        let is_sel = st.layout.accent_hex.eq_ignore_ascii_case(hex);
                        let hex_owned = hex.to_string();
                        div()
                            .id(SharedString::from(format!("settings-swatch-{hex}")))
                            .occlude()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(99.0))
                            .bg(rgb(color_val))
                            .border_2()
                            .border_color(rgb(if is_sel {
                                palette::TEXT_PRIMARY
                            } else {
                                palette::SURFACE_BG
                            }))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.layout.accent_hex = hex_owned.clone();
                                }
                                this.persist_layout_config();
                                cx.notify();
                            }))
                    })),
            ))
            .child(render_settings_row(
                tr(loc, "Terminal Font", "终端字体"),
                tr(loc, "Monospace font family", "终端渲染使用的等宽字体"),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .children(font_choices.iter().map(|&font| {
                        let is_sel = st.layout.font_family.eq_ignore_ascii_case(font);
                        let font_owned = font.to_string();
                        self.render_setting_pill(
                            SharedString::from(format!("settings-font-{font}")),
                            font,
                            is_sel,
                            accent,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.layout.font_family = font_owned.clone();
                                }
                                this.persist_layout_config();
                                cx.notify();
                            }),
                        )
                    })),
            ))
            .child(render_settings_row(
                tr(loc, "Terminal Font Size", "终端字号"),
                tr(loc, "10 px – 22 px", "可调节范围 10 px – 22 px"),
                self.render_stepper_control(
                    "font-size",
                    format!("{} px", font_px as u32),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Ok(mut g) = this.state.lock() {
                            g.layout.terminal_font_size_px =
                                (g.layout.terminal_font_size_px - 1.0).max(10.0);
                        }
                        this.persist_layout_config();
                        cx.notify();
                    }),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Ok(mut g) = this.state.lock() {
                            g.layout.terminal_font_size_px =
                                (g.layout.terminal_font_size_px + 1.0).min(22.0);
                        }
                        this.persist_layout_config();
                        cx.notify();
                    }),
                ),
            ))
            .child(render_settings_row(
                tr(loc, "Sidebar", "左侧边栏"),
                tr(
                    loc,
                    "Toggle visibility and width (200–320 px)",
                    "控制显示状态与宽度 (200–320 px)",
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(self.render_setting_pill(
                        "settings-sidebar-toggle",
                        if st.layout.sidebar_visible {
                            tr(loc, "Visible", "显示")
                        } else {
                            tr(loc, "Hidden", "隐藏")
                        },
                        st.layout.sidebar_visible,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.sidebar_visible = !g.layout.sidebar_visible;
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    ))
                    .child(self.render_stepper_control(
                        "sidebar-width",
                        format!("{} px", sidebar_w_px as u32),
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.sidebar_width_px =
                                    (g.layout.sidebar_width_px - 12.0).max(200.0);
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Ok(mut g) = this.state.lock() {
                                g.layout.sidebar_width_px =
                                    (g.layout.sidebar_width_px + 12.0).min(320.0);
                            }
                            this.persist_layout_config();
                            cx.notify();
                        }),
                    )),
            ))
    }

    fn render_settings_runtime_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let local_spaces_count = st.spaces.iter().filter(|s| !s.is_remote).count();
        let hidden_count = st.dismissed_spaces.len();
        let hc_ver = st
            .hc_version
            .as_deref()
            .unwrap_or_else(|| tr(loc, "not detected", "未检测到"));
        let hc_path_label = st
            .hc_path
            .as_ref()
            .map(|p| display_compact_path(&p.to_string_lossy()))
            .unwrap_or_else(|| "—".to_string());

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .p(px(11.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
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
                                            .bg(rgb(if st.hc_available {
                                                palette::DONE_GREEN
                                            } else {
                                                palette::TEXT_DIM
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(format!("herdr-core (hc) · {hc_ver}")),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .truncate()
                                    .child(hc_path_label),
                            ),
                    )
                    .child(self.render_setting_pill(
                        "settings-recheck-hc",
                        tr(loc, "Re-check", "重新检测"),
                        false,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            let detected = discover_hc_binary();
                            if let Ok(mut g) = this.state.lock() {
                                g.hc_available = detected.is_some();
                                g.hc_path = detected;
                            }
                            cx.notify();
                        }),
                    )),
            )
            .child(render_settings_row(
                tr(loc, "Split Terminals", "双栏终端分屏"),
                tr(loc, "Show two panes side by side (Cmd+D)", "并排显示左右两个终端窗口 (Cmd+D)"),
                self.render_setting_pill(
                    "settings-split-toggle",
                    if st.layout.split_terminals {
                        tr(loc, "Enabled", "开启")
                    } else {
                        tr(loc, "Off", "关闭")
                    },
                    st.layout.split_terminals,
                    accent,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.toggle_split_terminals();
                        cx.notify();
                    }),
                ),
            ))
            .child(render_settings_row(
                tr(loc, "Local Workspaces", "本地工作区"),
                &format!(
                    "{}: {local_spaces_count}",
                    tr(loc, "Active local spaces", "当前本地工作区数量")
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .when(hidden_count > 0, |el| {
                        el.child(self.render_setting_pill(
                            "settings-restore-spaces",
                            &format!(
                                "{} ({hidden_count})",
                                tr(loc, "Restore Hidden", "恢复已隐藏工作区")
                            ),
                            false,
                            accent,
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.restore_dismissed_spaces();
                                cx.notify();
                            }),
                        ))
                    })
                    .child(self.render_setting_pill(
                        "settings-add-folder",
                        tr(loc, "+ Add Folder…", "+ 添加本地目录…"),
                        false,
                        accent,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.add_workspace_folder();
                            cx.notify();
                        }),
                    )),
            ))
            .child(render_settings_row(
                tr(loc, "Reset Appearance", "重置外观配置"),
                tr(
                    loc,
                    "Restore default layout.json parameters",
                    "恢复 layout.json 默认字体、布局与强调色",
                ),
                self.render_setting_pill(
                    "settings-reset-layout",
                    tr(loc, "Reset Layout Defaults", "恢复默认布局参数"),
                    false,
                    accent,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Ok(mut g) = this.state.lock() {
                            g.layout = MalleableLayoutConfig::default();
                        }
                        this.persist_layout_config();
                        cx.notify();
                    }),
                ),
            ))
    }

    fn render_settings_machines_pane(
        &self,
        st: &CodaDesktopState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let loc = st.locale;
        let accent = st.layout.accent_color_u32();
        let show_auth_form = !st.is_logged_in || st.auth_form_expanded;

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                // Control-plane Server URL + quick presets
                div()
                    .p(px(10.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(tr(loc, "Control-Plane Server", "控制面服务器")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(self.render_setting_pill(
                                        "settings-srv-local",
                                        "Local :8080",
                                        st.server_url == "http://127.0.0.1:8080",
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.save_server_url_preference(
                                                "http://127.0.0.1:8080",
                                            );
                                            cx.notify();
                                        }),
                                    ))
                                    .child(self.render_setting_pill(
                                        "settings-srv-cloud",
                                        "codahub.dev",
                                        st.server_url == "https://codahub.dev",
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.save_server_url_preference("https://codahub.dev");
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-input-auth-server")
                            .occlude()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::CANVAS_BG))
                            .border_1()
                            .border_color(rgb(if st.input_target == ActiveInputTarget::AuthServerUrl {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child(st.server_url.clone())
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthServerUrl;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                // Account card (signed in vs login form)
                div()
                    .p(px(10.0))
                    .rounded(px(7.0))
                    .bg(rgb(palette::SURFACE_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .when(!show_auth_form, |el| {
                        el.child(
                            div()
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
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .rounded(px(99.0))
                                                .bg(rgb(palette::DONE_GREEN)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .truncate()
                                                .child(st.account_email.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(self.render_setting_pill(
                                            "settings-switch-acct",
                                            tr(loc, "Switch Account", "切换账号"),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.auth_form_expanded = true;
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.render_setting_pill(
                                            "settings-sign-out",
                                            tr(loc, "Sign Out", "退出登录"),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                let _ =
                                                    std::fs::remove_file(DaemonConfig::default_path());
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.is_logged_in = false;
                                                    g.auth_form_expanded = false;
                                                    g.account_email.clear();
                                                    g.remote_hosts.clear();
                                                    g.input_target =
                                                        ActiveInputTarget::TerminalDirect;
                                                }
                                                cx.notify();
                                            }),
                                        )),
                                ),
                        )
                    })
                    .when(show_auth_form, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(tr(loc, "Coda Account Sign In", "Coda 账号登录")),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(self.render_setting_pill(
                                            "settings-btn-browser-signin",
                                            tr(
                                                loc,
                                                "Sign in via Browser",
                                                "浏览器一键授权登录",
                                            ),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                let srv = this
                                                    .state
                                                    .lock()
                                                    .map(|g| g.server_url.clone())
                                                    .unwrap_or_default();
                                                spawn_device_login(
                                                    Arc::clone(&this.state),
                                                    Arc::clone(&this.ui_dirty),
                                                    Arc::clone(&this.mesh_hub),
                                                    srv,
                                                );
                                                cx.notify();
                                            }),
                                        ))
                                        .when(st.is_logged_in, |c| {
                                            c.child(self.render_setting_pill(
                                                "settings-collapse-auth",
                                                tr(loc, "Cancel", "取消"),
                                                false,
                                                accent,
                                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.auth_form_expanded = false;
                                                        g.input_target =
                                                            ActiveInputTarget::TerminalDirect;
                                                    }
                                                    cx.notify();
                                                }),
                                            ))
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id("settings-input-auth-email")
                                        .occlude()
                                        .flex_1()
                                        .px(px(8.0))
                                        .py(px(5.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::CANVAS_BG))
                                        .border_1()
                                        .border_color(rgb(
                                            if st.input_target == ActiveInputTarget::AuthAccount {
                                                accent
                                            } else {
                                                palette::BORDER
                                            },
                                        ))
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
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.input_target = ActiveInputTarget::AuthAccount;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("settings-input-auth-pass")
                                        .occlude()
                                        .flex_1()
                                        .px(px(8.0))
                                        .py(px(5.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::CANVAS_BG))
                                        .border_1()
                                        .border_color(rgb(
                                            if st.input_target == ActiveInputTarget::AuthPassword {
                                                accent
                                            } else {
                                                palette::BORDER
                                            },
                                        ))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(if st.auth_password_input.is_empty() {
                                            palette::TEXT_DIM
                                        } else {
                                            palette::TEXT_PRIMARY
                                        }))
                                        .child(if st.auth_password_input.is_empty() {
                                            tr(loc, "Password (Enter submits)", "密码 (回车登录)")
                                                .to_string()
                                        } else {
                                            "*".repeat(st.auth_password_input.chars().count())
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.input_target = ActiveInputTarget::AuthPassword;
                                            }
                                            cx.notify();
                                        })),
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
                    }),
            )
            .child(
                // Machines list
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(palette::TEXT_DIM))
                            .child(format!(
                                "{} ({})",
                                tr(loc, "ONLINE MACHINES", "在线设备列表"),
                                st.remote_hosts.len() + 1
                            )),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
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
                                            .bg(rgb(palette::DONE_GREEN)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .child(format!(
                                                "{} · {}",
                                                tr(loc, "This Mac", "本机"),
                                                st.local_machine_name
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child("macos · hybrid"),
                            ),
                    )
                    .children(st.remote_hosts.iter().take(4).map(|host| {
                        let host_clone = host.clone();
                        div()
                            .id(SharedString::from(format!(
                                "settings-mesh-host-{}",
                                host.id
                            )))
                            .occlude()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
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
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .flex_shrink_0()
                                            .rounded(px(99.0))
                                            .bg(rgb(if host.is_online {
                                                palette::DONE_GREEN
                                            } else {
                                                palette::TEXT_DIM
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                            .truncate()
                                            .child(host.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .truncate()
                                            .child(format!("{} · {}", host.platform, host.role)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.0))
                                    .text_color(rgb(accent))
                                    .child(tr(loc, "Open Workspace", "进入工作区")),
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.mesh_hub.probe_host(host_clone.clone());
                                if let Ok(mut g) = this.state.lock() {
                                    g.settings_open = false;
                                    g.input_target = ActiveInputTarget::TerminalDirect;
                                }
                                cx.notify();
                            }))
                    })),
            )
    }

    fn render_settings_shortcuts_pane(&self, st: &CodaDesktopState) -> impl IntoElement {
        let loc = st.locale;
        let left_col: &[(&str, &str, &str)] = &[
            ("⌘ ,", "Open Settings", "打开设置"),
            ("⌘ P / ⌘ K", "Command Palette", "命令面板"),
            ("⌘ N / ⌘ T", "New Terminal", "新建终端"),
            ("⌘ W", "Close Active Terminal", "关闭当前终端"),
            ("⌘ D", "Toggle Split Panes", "分屏终端"),
        ];
        let right_col: &[(&str, &str, &str)] = &[
            ("⌘ B", "Toggle Left Sidebar", "显示/隐藏侧边栏"),
            ("⌘ 1", "Main Workbench", "主工作台"),
            ("⌘ 2", "Terminals Grid", "终端平铺"),
            ("⌃ C", "Interrupt Terminal (SIGINT)", "中断终端进程"),
            ("⌘ V", "Paste into Terminal / Input", "粘贴"),
        ];

        div()
            .flex()
            .flex_row()
            .gap(px(10.0))
            .children([left_col, right_col].into_iter().map(|col| {
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(col.iter().map(|&(keys, en, zh)| {
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(6.0))
                            .bg(rgb(palette::SURFACE_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(palette::TEXT_PRIMARY))
                                    .child(tr(loc, en, zh)),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER_STRONG))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(keys),
                            )
                    }))
            }))
    }

    fn render_setting_pill(
        &self,
        id: impl Into<SharedString>,
        label: &str,
        active: bool,
        accent: u32,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id.into())
            .occlude()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(5.0))
            .bg(rgb(if active {
                palette::HOVER_BG
            } else {
                palette::CHROME_BG
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
            .on_click(on_click)
    }

    fn render_stepper_control(
        &self,
        slug: &'static str,
        value_label: String,
        on_dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .id(SharedString::from(format!("settings-{slug}-dec")))
                    .occlude()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded(px(5.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(rgb(palette::TEXT_PRIMARY))
                    .child("−")
                    .on_click(on_dec),
            )
            .child(
                div()
                    .min_w(px(52.0))
                    .flex()
                    .justify_center()
                    .text_size(px(11.5))
                    .text_color(rgb(palette::TEXT_PRIMARY))
                    .child(value_label),
            )
            .child(
                div()
                    .id(SharedString::from(format!("settings-{slug}-inc")))
                    .occlude()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded(px(5.0))
                    .bg(rgb(palette::CHROME_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER))
                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(rgb(palette::TEXT_PRIMARY))
                    .child("+")
                    .on_click(on_inc),
            )
    }
}

fn render_settings_row(label: &str, hint: &str, control: impl IntoElement) -> impl IntoElement {
    div()
        .px(px(11.0))
        .py(px(8.0))
        .rounded(px(7.0))
        .bg(rgb(palette::SURFACE_BG))
        .border_1()
        .border_color(rgb(palette::BORDER))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(palette::TEXT_PRIMARY))
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(palette::TEXT_DIM))
                        .truncate()
                        .child(hint.to_string()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
}

fn render_styled_terminal_line(line: &StyledTerminalLine, line_height_px: f32) -> impl IntoElement {
    if line.text.is_empty() {
        return div().h(px(line_height_px)).child(" ").into_any_element();
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
        .min_h(px(line_height_px))
        .text_color(rgb(palette::TEXT_PRIMARY))
        .child(StyledText::new(line.text.clone()).with_highlights(highlights))
        .into_any_element()
}

// ============================================================================
// Background Workers (`herdr-core` persistent sessions & `coda-daemon` Mesh)
// ============================================================================

fn herdr_status_to_osc(status: &Option<coda_protocol::HerdrAgentStatus>) -> Option<Osc7501Status> {
    status.as_ref().map(|s| Osc7501Status {
        state: AgentLifecycleState::from_str_lossy(&s.state),
        task: s
            .title
            .clone()
            .or_else(|| s.msg.clone())
            .or_else(|| s.app.clone()),
        updated_at: Instant::now(),
    })
}

fn spawn_local_fallback_shell(
    state_bg: &Arc<Mutex<CodaDesktopState>>,
    pty_mgr: &Arc<Mutex<LocalPtyManager>>,
    ui_dirty: &Arc<AtomicBool>,
) {
    let target = state_bg.lock().ok().and_then(|g| {
        if g.sessions.iter().any(|s| s.remote_host_id.is_none()) {
            return None;
        }
        g.active_space()
            .filter(|sp| !sp.is_remote)
            .map(|sp| (sp.id.clone(), sp.name.clone(), sp.path.clone()))
    });
    let Some((space_id, space_name, cwd_str)) = target else {
        return;
    };
    let cwd = PathBuf::from(&cwd_str);
    let id = format!("pty-{}", &Uuid::new_v4().simple().to_string()[..8]);
    let ok = pty_mgr
        .lock()
        .map(|mut mgr| mgr.spawn_session(&id, &cwd, None, 120, 36).is_ok())
        .unwrap_or(false);
    if ok {
        if let Ok(mut g) = state_bg.lock() {
            g.sessions.push(TerminalSessionEntry {
                id: id.clone(),
                title: format!("{space_name} · shell"),
                space_id,
                cwd: cwd_str,
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
            if g.active_session_id.is_none() {
                g.active_session_id = Some(id);
            }
        }
        ui_dirty.store(true, Ordering::Release);
    }
}

async fn reconcile_local_herdr_sessions(
    hc: &HerdrClient,
    hc_bin: &Path,
    state_bg: &Arc<Mutex<CodaDesktopState>>,
    pty_mgr: &Arc<Mutex<LocalPtyManager>>,
    ui_dirty: &Arc<AtomicBool>,
) {
    let mut list = hc.list_terminals(false).await.unwrap_or_default();

    if list.is_empty() {
        let launch_target = state_bg.lock().ok().and_then(|g| {
            if g.did_auto_seed_local
                || !g.closed_session_ids.is_empty()
                || g.sessions.iter().any(|s| s.remote_host_id.is_none())
            {
                return None;
            }
            g.spaces
                .iter()
                .find(|sp| !sp.is_remote)
                .map(|sp| (sp.name.clone(), sp.path.clone()))
        });
        if let Some((_space_name, cwd_str)) = launch_target {
            if let Ok(mut g) = state_bg.lock() {
                g.did_auto_seed_local = true;
            }
            let hc_name = format!("coda-{}", &Uuid::new_v4().simple().to_string()[..8]);
            if let Ok(term) = hc
                .launch_agent("shell", Some(&hc_name), Some(&cwd_str), None, 120, 36)
                .await
            {
                list.push(term);
            }
        }
    }

    let home_cwd = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));

    let live_ids: HashSet<String> = list
        .iter()
        .map(|t| format!("hc-{}", t.address))
        .collect();
    let closed_snapshot: HashSet<String> = state_bg
        .lock()
        .map(|mut g| {
            g.closed_session_ids
                .retain(|cid| !cid.starts_with("hc-") || live_ids.contains(cid));
            g.closed_session_ids.clone()
        })
        .unwrap_or_default();

    for term in list {
        let id = format!("hc-{}", term.address);
        if closed_snapshot.contains(&id) {
            continue;
        }
        let attach_cwd = if !term.cwd.trim().is_empty() && Path::new(&term.cwd).is_dir() {
            PathBuf::from(&term.cwd)
        } else {
            home_cwd.clone()
        };
        let attached = if let Ok(mut mgr) = pty_mgr.lock() {
            if !mgr.has_session(&id) {
                mgr.spawn_herdr_attach(&id, &attach_cwd, hc_bin, &term.address, 120, 36)
                    .is_ok()
            } else {
                true
            }
        } else {
            false
        };
        if attached {
            if let Ok(mut g) = state_bg.lock() {
                if g.closed_session_ids.contains(&id) {
                    continue;
                }
                let osc = herdr_status_to_osc(&term.status);
                if let Some(existing) = g.sessions.iter_mut().find(|s| s.id == id) {
                    existing.alive = term.alive;
                    if let Some(o) = osc {
                        existing.osc_status = Some(o);
                    }
                } else {
                    let matched_space_id = g
                        .spaces
                        .iter()
                        .filter(|sp| !sp.is_remote)
                        .filter(|sp| {
                            let prefix = format!("{}/", sp.path.trim_end_matches('/'));
                            term.cwd == sp.path || term.cwd.starts_with(&prefix)
                        })
                        .max_by_key(|sp| sp.path.len())
                        .map(|sp| sp.id.clone());

                    let space_id = if let Some(sid) = matched_space_id {
                        sid
                    } else if !term.cwd.trim().is_empty()
                        && Path::new(&term.cwd).is_dir()
                        && !g.dismissed_spaces.contains(&term.cwd)
                        && !g
                            .dismissed_spaces
                            .contains(&format!("local:{}", term.cwd))
                    {
                        let cwd_path = Path::new(&term.cwd);
                        let new_id = format!("local:{}", term.cwd);
                        if !g.spaces.iter().any(|s| s.id == new_id) {
                            let name = cwd_path
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("workspace")
                                .to_string();
                            let snap = compute_repo_snapshot(cwd_path, None);
                            let machine_label = g.local_machine_name.clone();
                            g.spaces.push(SpaceEntry {
                                id: new_id.clone(),
                                avatar_color: space_avatar_color(&name),
                                name,
                                path: term.cwd.clone(),
                                machine_id: "local".to_string(),
                                machine_label,
                                is_remote: false,
                                branch: snap.branch,
                                dirty_files: snap.changed_files.len(),
                                ahead: 0,
                                behind: 0,
                            });
                        }
                        new_id
                    } else {
                        g.spaces
                            .iter()
                            .find(|sp| !sp.is_remote)
                            .map(|sp| sp.id.clone())
                            .unwrap_or_else(|| "local".to_string())
                    };
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
                        alive: term.alive,
                        attached_hc: true,
                        hc_addr: Some(term.address),
                        remote_host_id: None,
                        osc_status: osc,
                    });
                    if g.active_session_id.is_none() {
                        g.active_session_id = Some(id);
                    }
                }
            }
        }
    }

    if let Ok(mut g) = state_bg.lock() {
        for sess in &mut g.sessions {
            if sess.remote_host_id.is_none()
                && sess.attached_hc
                && !live_ids.contains(&sess.id)
            {
                sess.alive = false;
            }
        }
    }
    ui_dirty.store(true, Ordering::Release);
}

fn spawn_herdr_discovery(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    pty_mgr: Arc<Mutex<LocalPtyManager>>,
    ui_dirty: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let hc_bin = match discover_hc_binary() {
                Some(p) => p,
                None => match ensure_or_install_hc_binary().await {
                    Ok(p) => p,
                    Err(_) => {
                        spawn_local_fallback_shell(&state_bg, &pty_mgr, &ui_dirty);
                        return;
                    }
                },
            };

            let Ok(hc) = HerdrClient::connect_with_bin(hc_bin.clone()).await else {
                spawn_local_fallback_shell(&state_bg, &pty_mgr, &ui_dirty);
                return;
            };
            let ver = hc.info().version.clone();
            if let Ok(mut g) = state_bg.lock() {
                g.hc_available = true;
                g.hc_version = Some(ver);
                g.hc_path = Some(hc_bin.clone());
            }

            reconcile_local_herdr_sessions(&hc, &hc_bin, &state_bg, &pty_mgr, &ui_dirty).await;

            // Continuously subscribe to `herdr-core` `events.subscribe` so terminals created or
            // updated by iOS/iPadOS on this Mac appear & attach live in GPUI Desktop.
            loop {
                if let Ok(mut ev_rx) = hc.subscribe_live_events().await {
                    while let Some(ev) = ev_rx.recv().await {
                        let kind = ev
                            .get("kind")
                            .or_else(|| ev.get("type"))
                            .or_else(|| ev.get("event"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if kind == "status" {
                            if let Some(addr) = ev
                                .get("address")
                                .or_else(|| ev.get("terminal").and_then(|t| t.get("address")))
                                .and_then(|v| v.as_str())
                            {
                                let sid = format!("hc-{addr}");
                                let st_val = ev.get("status").or_else(|| {
                                    ev.get("terminal").and_then(|t| t.get("status"))
                                });
                                if let Some(raw_st) = st_val.and_then(|v| {
                                    serde_json::from_value::<coda_protocol::HerdrAgentStatus>(
                                        v.clone(),
                                    )
                                    .ok()
                                }) {
                                    if let Ok(mut g) = state_bg.lock() {
                                        if let Some(s) = g.sessions.iter_mut().find(|s| s.id == sid) {
                                            s.osc_status = herdr_status_to_osc(&Some(raw_st));
                                        }
                                    }
                                    ui_dirty.store(true, Ordering::Release);
                                    continue;
                                }
                            }
                        }
                        reconcile_local_herdr_sessions(&hc, &hc_bin, &state_bg, &pty_mgr, &ui_dirty)
                            .await;
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    });
}

static EMBEDDED_HOST_STARTED: AtomicBool = AtomicBool::new(false);

fn normalize_newlines_for_vt(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 32);
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' && (i == 0 || bytes[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

fn parse_git_status_short(status_short: &str) -> Vec<ChangedFileEntry> {
    status_short
        .lines()
        .filter(|l| l.len() >= 3)
        .map(|l| ChangedFileEntry {
            status: l[..2].trim().to_string(),
            path: l[3..].trim().to_string(),
        })
        .collect()
}

pub struct RemoteMeshHub {
    rt: Option<tokio::runtime::Runtime>,
    state: Arc<Mutex<CodaDesktopState>>,
    pty_mgr: Arc<Mutex<LocalPtyManager>>,
    ui_dirty: Arc<AtomicBool>,
    peers: tokio::sync::Mutex<HashMap<Uuid, Arc<LivePeerSession>>>,
    client_identities: tokio::sync::Mutex<HashMap<Uuid, (Uuid, coda_protocol::StoredDeviceIdentity)>>,
    attached_terms: tokio::sync::Mutex<HashSet<(Uuid, String)>>,
    watcher_gen: AtomicU64,
}

impl Drop for RemoteMeshHub {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl RemoteMeshHub {
    pub fn new(
        state: Arc<Mutex<CodaDesktopState>>,
        pty_mgr: Arc<Mutex<LocalPtyManager>>,
        ui_dirty: Arc<AtomicBool>,
    ) -> Arc<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("failed to start Coda mesh tokio runtime");
        Arc::new(Self {
            rt: Some(rt),
            state,
            pty_mgr,
            ui_dirty,
            peers: tokio::sync::Mutex::new(HashMap::new()),
            client_identities: tokio::sync::Mutex::new(HashMap::new()),
            attached_terms: tokio::sync::Mutex::new(HashSet::new()),
            watcher_gen: AtomicU64::new(0),
        })
    }

    fn spawn_bg<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(ref rt) = self.rt {
            rt.spawn(fut);
        }
    }

    pub fn ensure_embedded_host(self: &Arc<Self>) {
        let cfg_path = DaemonConfig::default_path();
        if DaemonConfig::load(&cfg_path).is_err() {
            return;
        }
        if EMBEDDED_HOST_STARTED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.spawn_bg(async move {
            if start_embedded_host_if_configured(&cfg_path).await.is_none() {
                EMBEDDED_HOST_STARTED.store(false, Ordering::Release);
            }
        });
    }

    pub fn start_account_sync(self: &Arc<Self>) {
        let my_gen = self.watcher_gen.fetch_add(1, Ordering::AcqRel) + 1;
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            hub.sync_all_hosts_and_sessions().await;

            loop {
                if hub.watcher_gen.load(Ordering::Acquire) != my_gen {
                    break;
                }
                let Ok(cfg) = DaemonConfig::load(&DaemonConfig::default_path()) else {
                    break;
                };
                let Some(ref tok) = cfg.session_token else {
                    break;
                };

                match subscribe_account_events_ws(&cfg.server_url, tok).await {
                    Ok(mut rx) => {
                        while let Some(ev) = rx.recv().await {
                            if hub.watcher_gen.load(Ordering::Acquire) != my_gen {
                                return;
                            }
                            if ev.role.as_deref() == Some("client") {
                                continue;
                            }
                            if matches!(
                                ev.kind.as_str(),
                                "connected" | "devices_changed" | "peer_presence"
                            ) {
                                hub.sync_all_hosts_and_sessions().await;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("account /api/v1/events/ws reconnect scheduled: {e}");
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    async fn sync_all_hosts_and_sessions(self: &Arc<Self>) {
        let Ok(cfg) = DaemonConfig::load(&DaemonConfig::default_path()) else {
            return;
        };
        let Some(ref tok) = cfg.session_token else {
            return;
        };
        let Ok(list) = list_account_hosts(&cfg.server_url, tok).await else {
            return;
        };
        let hosts: Vec<RemoteHostSummary> =
            list.into_iter().filter(|h| h.id != cfg.device_id).collect();

        {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            let online_map: HashMap<String, bool> =
                hosts.iter().map(|h| (h.id.to_string(), h.is_online)).collect();
            for sp in &mut g.spaces {
                if sp.is_remote {
                    match online_map.get(&sp.machine_id) {
                        Some(true) => {
                            if sp.branch == "offline" {
                                sp.branch = "online".to_string();
                            }
                        }
                        _ => sp.branch = "offline".to_string(),
                    }
                }
            }
            for host in &hosts {
                let sid = format!("remote:{}", host.id);
                if !g.spaces.iter().any(|s| s.machine_id == host.id.to_string()) {
                    g.spaces.push(SpaceEntry {
                        id: sid,
                        avatar_color: space_avatar_color(&host.name),
                        name: host.name.clone(),
                        path: ".".to_string(),
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
            g.remote_hosts = hosts.clone();
        }
        self.ui_dirty.store(true, Ordering::Release);

        for host in &hosts {
            if !host.is_online {
                self.drop_peer(host.id).await;
                continue;
            }
            self.sync_single_remote_host(&cfg, host).await;
        }
    }

    async fn drop_peer(self: &Arc<Self>, host_id: Uuid) {
        if let Some(peer) = self.peers.lock().await.remove(&host_id) {
            peer.close();
        }
        {
            let mut att = self.attached_terms.lock().await;
            att.retain(|(id, _)| *id != host_id);
        }
        let host_str = host_id.to_string();
        let affected: Vec<String> = self
            .state
            .lock()
            .map(|mut g| {
                let mut ids = Vec::new();
                for s in &mut g.sessions {
                    if s.remote_host_id.as_deref() == Some(host_str.as_str()) {
                        s.alive = false;
                        ids.push(s.id.clone());
                    }
                }
                ids
            })
            .unwrap_or_default();
        if let Ok(mgr) = self.pty_mgr.lock() {
            for sid in affected {
                mgr.set_remote_alive(&sid, false);
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    async fn get_or_connect_peer(
        self: &Arc<Self>,
        cfg: &DaemonConfig,
        host: &RemoteHostSummary,
    ) -> anyhow::Result<Arc<LivePeerSession>> {
        if let Some(existing) = self.peers.lock().await.get(&host.id).cloned() {
            return Ok(existing);
        }
        let (client_dev_id, client_identity) = {
            if let Some(existing) = self.client_identities.lock().await.get(&host.id).cloned() {
                existing
            } else if let Some(tok) = cfg.session_token.as_deref() {
                if let Ok(created) = coda_daemon::register_client_identity(
                    &cfg.server_url,
                    tok,
                    &format!("desktop-client-{}", &host.id.to_string()[..8]),
                    "macos",
                )
                .await
                {
                    self.client_identities
                        .lock()
                        .await
                        .insert(host.id, created.clone());
                    created
                } else {
                    (cfg.device_id, cfg.identity.clone())
                }
            } else {
                (cfg.device_id, cfg.identity.clone())
            }
        };
        let relay_cfg = match cfg.session_token.as_deref() {
            Some(tok) => mint_peer_relay_config(
                &cfg.server_url,
                tok,
                client_dev_id,
                host.id,
                &host.x25519_pubkey,
            )
            .await
            .ok(),
            None => None,
        };
        let session = LivePeerSession::connect_auto(
            client_identity,
            client_dev_id,
            host.id,
            host.endpoints.clone(),
            Some("255.255.255.255:24444".parse().unwrap()),
            relay_cfg,
        )
        .await?;
        let arc_sess = Arc::new(session);
        let mut rx = arc_sess.subscribe_events();
        {
            self.peers.lock().await.insert(host.id, Arc::clone(&arc_sess));
        }

        let hub = Arc::clone(self);
        let host_id = host.id;
        let host_clone = host.clone();
        let peer_for_task = Arc::clone(&arc_sess);
        self.spawn_bg(async move {
            loop {
                match rx.recv().await {
                    Ok(PeerAsyncEvent::PtyOutputChunk {
                        terminal,
                        data_b64,
                        ..
                    }) => {
                        if let Ok(bytes) = B64.decode(&data_b64) {
                            let sid = format!("remote:{host_id}:{terminal}");
                            if let Ok(mut mgr) = hub.pty_mgr.lock() {
                                mgr.ensure_remote_buffer(&sid, 120, 36);
                                mgr.feed_remote_output(&sid, &bytes);
                            }
                        }
                    }
                    Ok(PeerAsyncEvent::HerdrLiveEvent { terminal, .. }) => {
                        if let Some(term) = terminal {
                            let sid = format!("remote:{}:{}", host_id, term.address);
                            if let Ok(mut g) = hub.state.lock() {
                                if let Some(sess) = g.sessions.iter_mut().find(|s| s.id == sid) {
                                    sess.alive = term.alive;
                                    if let Some(osc) = herdr_status_to_osc(&term.status) {
                                        sess.osc_status = Some(osc);
                                    }
                                }
                            }
                            if let Ok(mgr) = hub.pty_mgr.lock() {
                                mgr.set_remote_alive(&sid, term.alive);
                            }
                            hub.ui_dirty.store(true, Ordering::Release);
                        }
                        hub.sync_with_connected_peer(Arc::clone(&peer_for_task), &host_clone)
                            .await;
                    }
                    Ok(PeerAsyncEvent::Disconnected(_))
                    | Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        hub.drop_peer(host_id).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });

        Ok(arc_sess)
    }

    async fn sync_single_remote_host(
        self: &Arc<Self>,
        cfg: &DaemonConfig,
        host: &RemoteHostSummary,
    ) {
        let Ok(peer) = self.get_or_connect_peer(cfg, host).await else {
            return;
        };
        self.sync_with_connected_peer(peer, host).await;
    }

    async fn sync_with_connected_peer(
        self: &Arc<Self>,
        peer: Arc<LivePeerSession>,
        host: &RemoteHostSummary,
    ) {

        let ws_reply = peer
            .request(
                PeerMessage::ListWorkspaces {
                    req_id: format!("ws-{}", Uuid::new_v4().simple()),
                    root_dirs: Vec::new(),
                },
                TrafficLane::BulkP1,
                Duration::from_secs(10),
            )
            .await;
        let repos = match ws_reply {
            Ok(PeerMessage::WorkspacesSnapshot { repos, .. }) => repos,
            _ => Vec::new(),
        };

        let terms_reply = peer
            .request(
                PeerMessage::ListTerminals {
                    req_id: format!("lt-{}", Uuid::new_v4().simple()),
                    include_finished: false,
                },
                TrafficLane::ControlP0,
                Duration::from_secs(10),
            )
            .await;
        let terms = match terms_reply {
            Ok(PeerMessage::TerminalsSnapshot { terminals, .. }) => terminals,
            _ => return,
        };

        let host_id_str = host.id.to_string();
        let remote_sid_prefix = format!("remote:{}:", host.id);
        let fallback_space_id = format!("remote:{}", host.id);
        let fallback_path = terms
            .iter()
            .find(|t| !t.cwd.trim().is_empty())
            .map(|t| t.cwd.clone())
            .unwrap_or_else(|| ".".to_string());

        let mut closed_snapshot = HashSet::new();
        let mut active_remote_refresh: Option<(String, String, String)> = None;
        {
            if let Ok(mut g) = self.state.lock() {
                let mut first_repo_id: Option<String> = None;
                for repo in &repos {
                    let sp_id = format!("remote:{}:{}", host.id, repo.root_path);
                    if g.dismissed_spaces.contains(&sp_id) {
                        continue;
                    }
                    if first_repo_id.is_none() {
                        first_repo_id = Some(sp_id.clone());
                    }
                    let primary_branch = repo
                        .branches
                        .first()
                        .map(|b| b.branch.clone())
                        .filter(|b| !b.is_empty())
                        .unwrap_or_else(|| "online".to_string());
                    let dirty_count = repo
                        .branches
                        .iter()
                        .map(|b| (b.diff_added + b.diff_deleted) as usize)
                        .sum();
                    if let Some(sp) = g.spaces.iter_mut().find(|s| s.id == sp_id) {
                        sp.branch = primary_branch;
                        sp.dirty_files = dirty_count;
                    } else {
                        g.spaces.push(SpaceEntry {
                            id: sp_id,
                            avatar_color: space_avatar_color(&repo.repo_name),
                            name: repo.repo_name.clone(),
                            path: repo.root_path.clone(),
                            machine_id: host_id_str.clone(),
                            machine_label: host.name.clone(),
                            is_remote: true,
                            branch: primary_branch,
                            dirty_files: dirty_count,
                            ahead: 0,
                            behind: 0,
                        });
                    }
                }

                if let Some(ref primary_repo_id) = first_repo_id {
                    g.spaces.retain(|s| s.id != fallback_space_id);
                    if g.active_space_id.as_deref() == Some(fallback_space_id.as_str()) {
                        g.active_space_id = Some(primary_repo_id.clone());
                    }
                    for sess in &mut g.sessions {
                        if sess.space_id == fallback_space_id {
                            sess.space_id = primary_repo_id.clone();
                        }
                    }
                } else if repos.is_empty()
                    && !g.dismissed_spaces.contains(&fallback_space_id)
                    && !g.spaces.iter().any(|s| s.id == fallback_space_id)
                {
                    g.spaces.push(SpaceEntry {
                        id: fallback_space_id.clone(),
                        avatar_color: space_avatar_color(&host.name),
                        name: host.name.clone(),
                        path: fallback_path.clone(),
                        machine_id: host_id_str.clone(),
                        machine_label: host.name.clone(),
                        is_remote: true,
                        branch: "online".to_string(),
                        dirty_files: 0,
                        ahead: 0,
                        behind: 0,
                    });
                }

                let live_sids: HashSet<String> = terms
                    .iter()
                    .map(|t| format!("remote:{}:{}", host.id, t.address))
                    .collect();
                g.closed_session_ids
                    .retain(|cid| !cid.starts_with(&remote_sid_prefix) || live_sids.contains(cid));
                closed_snapshot = g.closed_session_ids.clone();

                for sess in &mut g.sessions {
                    if sess.remote_host_id.as_deref() == Some(host_id_str.as_str())
                        && !live_sids.contains(&sess.id)
                    {
                        sess.alive = false;
                    }
                }

                for term in &terms {
                    let sid = format!("remote:{}:{}", host.id, term.address);
                    if closed_snapshot.contains(&sid) {
                        continue;
                    }
                    let space_id = g
                        .spaces
                        .iter()
                        .filter(|sp| sp.machine_id == host_id_str)
                        .filter(|sp| !term.cwd.is_empty() && term.cwd.starts_with(&sp.path))
                        .max_by_key(|sp| sp.path.len())
                        .or_else(|| g.spaces.iter().find(|sp| sp.machine_id == host_id_str))
                        .map(|sp| sp.id.clone())
                        .unwrap_or_else(|| fallback_space_id.clone());

                    let osc = herdr_status_to_osc(&term.status);
                    let label = term
                        .name
                        .as_deref()
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or(&term.address);
                    if let Some(existing) = g.sessions.iter_mut().find(|s| s.id == sid) {
                        existing.alive = term.alive;
                        existing.space_id = space_id;
                        if let Some(o) = osc {
                            existing.osc_status = Some(o);
                        }
                    } else {
                        g.sessions.push(TerminalSessionEntry {
                            id: sid.clone(),
                            title: format!("{} · {}", host.name, label),
                            space_id,
                            cwd: term.cwd.clone(),
                            started_at: Instant::now(),
                            last_output_at: Instant::now(),
                            last_byte_sig: 0,
                            lines: Vec::new(),
                            styled_lines: Vec::new(),
                            scroll_offset: 0,
                            alive: term.alive,
                            attached_hc: true,
                            hc_addr: Some(term.address.clone()),
                            remote_host_id: Some(host_id_str.clone()),
                            osc_status: osc,
                        });
                        if g.active_session_id.is_none() {
                            g.active_session_id = Some(sid);
                        }
                    }
                }

                if let Some(sp) = g.active_space() {
                    if sp.machine_id == host_id_str && sp.path != "." && g.dir_entries.is_empty() {
                        active_remote_refresh = Some((
                            sp.id.clone(),
                            sp.path.clone(),
                            g.current_browse_rel.clone(),
                        ));
                    }
                }
            }
        }

        if let Some((sp_id, sp_path, browse_rel)) = active_remote_refresh {
            self.refresh_remote_git_diff(host.id, &sp_id, &sp_path);
            self.refresh_remote_directory(host.id, &sp_id, &sp_path, &browse_rel);
        }

        for term in terms {
            let sid = format!("remote:{}:{}", host.id, term.address);
            if closed_snapshot.contains(&sid) {
                continue;
            }
            if let Ok(mut mgr) = self.pty_mgr.lock() {
                mgr.ensure_remote_buffer(&sid, term.cols.max(80), term.rows.max(24));
                mgr.set_remote_alive(&sid, term.alive);
            }
            let need_attach = {
                let mut att = self.attached_terms.lock().await;
                att.insert((host.id, term.address.clone()))
            };
            if need_attach {
                self.seed_and_attach_remote_term(Arc::clone(&peer), host.id, &term)
                    .await;
            }
        }

        self.ui_dirty.store(true, Ordering::Release);
    }

    async fn seed_and_attach_remote_term(
        self: &Arc<Self>,
        peer: Arc<LivePeerSession>,
        host_id: Uuid,
        term: &HerdrTerminalSummary,
    ) {
        let sid = format!("remote:{}:{}", host_id, term.address);
        if let Ok(PeerMessage::TerminalScreenSnapshot { text, .. }) = peer
            .request(
                PeerMessage::ReadTerminalScreen {
                    req_id: format!("scr-{}", Uuid::new_v4().simple()),
                    terminal: term.address.clone(),
                    history_lines: 120,
                },
                TrafficLane::BulkP1,
                Duration::from_secs(6),
            )
            .await
        {
            if !text.is_empty() {
                let norm = normalize_newlines_for_vt(&text);
                if let Ok(mgr) = self.pty_mgr.lock() {
                    mgr.feed_remote_output(&sid, &norm);
                }
            }
        }

        let attached_ok = matches!(
            peer.request(
                PeerMessage::PtyAttach {
                    req_id: format!("att-{}", Uuid::new_v4().simple()),
                    terminal: term.address.clone(),
                    cols: 120,
                    rows: 36,
                },
                TrafficLane::ControlP0,
                Duration::from_secs(6),
            )
            .await,
            Ok(PeerMessage::PtyAttached { ok: true, .. })
        );

        if !attached_ok {
            self.attached_terms
                .lock()
                .await
                .remove(&(host_id, term.address.clone()));
        }
    }

    async fn ensure_peer_for_host(self: &Arc<Self>, host_id: Uuid) -> Option<Arc<LivePeerSession>> {
        if let Some(p) = self.peers.lock().await.get(&host_id).cloned() {
            return Some(p);
        }
        let cfg = DaemonConfig::load(&DaemonConfig::default_path()).ok()?;
        let host = self
            .state
            .lock()
            .ok()
            .and_then(|g| g.remote_hosts.iter().find(|h| h.id == host_id).cloned())?;
        self.get_or_connect_peer(&cfg, &host).await.ok()
    }

    pub fn spawn_remote_terminal_or_agent(
        self: &Arc<Self>,
        host_id: Uuid,
        agent: &str,
        cwd: &str,
        space_id: &str,
    ) {
        let hub = Arc::clone(self);
        let agent = agent.to_string();
        let cwd = cwd.to_string();
        let space_id = space_id.to_string();
        if let Ok(mut g) = hub.state.lock() {
            g.status_notice = Some(format!("Starting {agent} on remote host..."));
        }
        hub.ui_dirty.store(true, Ordering::Release);
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some("Remote host is not connected".to_string());
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let host_name = hub
                .state
                .lock()
                .ok()
                .and_then(|g| {
                    g.remote_hosts
                        .iter()
                        .find(|h| h.id == host_id)
                        .map(|h| h.name.clone())
                })
                .unwrap_or_else(|| "remote".to_string());

            let cwd_opt = if cwd.trim().is_empty() || cwd == "." {
                None
            } else {
                Some(cwd.clone())
            };
            let hc_name = format!("coda-{}", &Uuid::new_v4().simple().to_string()[..8]);
            let req = PeerMessage::LaunchAgent {
                req_id: format!("la-{}", Uuid::new_v4().simple()),
                agent: agent.clone(),
                name: Some(hc_name),
                cwd: cwd_opt,
                initial_prompt: None,
                cols: 120,
                rows: 36,
            };
            match peer
                .request(req, TrafficLane::ControlP0, Duration::from_secs(12))
                .await
            {
                Ok(PeerMessage::LaunchAgentResult {
                    ok: true,
                    terminal: Some(term),
                    ..
                }) => {
                    let sid = format!("remote:{}:{}", host_id, term.address);
                    if let Ok(mut mgr) = hub.pty_mgr.lock() {
                        mgr.ensure_remote_buffer(&sid, 120, 36);
                    }
                    {
                        if let Ok(mut g) = hub.state.lock() {
                            g.closed_session_ids.remove(&sid);
                            g.status_notice = None;
                            if !g.sessions.iter().any(|s| s.id == sid) {
                                g.sessions.push(TerminalSessionEntry {
                                    id: sid.clone(),
                                    title: format!("{host_name} · {agent}"),
                                    space_id,
                                    cwd: term.cwd.clone(),
                                    started_at: Instant::now(),
                                    last_output_at: Instant::now(),
                                    last_byte_sig: 0,
                                    lines: Vec::new(),
                                    styled_lines: Vec::new(),
                                    scroll_offset: 0,
                                    alive: true,
                                    attached_hc: true,
                                    hc_addr: Some(term.address.clone()),
                                    remote_host_id: Some(host_id.to_string()),
                                    osc_status: None,
                                });
                            }
                            if g.layout.split_terminals && g.focused_pane == SplitPane::Secondary {
                                g.split_session_id = Some(sid);
                            } else {
                                g.active_session_id = Some(sid);
                            }
                            g.input_target = ActiveInputTarget::TerminalDirect;
                        }
                    }
                    {
                        hub.attached_terms
                            .lock()
                            .await
                            .insert((host_id, term.address.clone()));
                    }
                    hub.seed_and_attach_remote_term(Arc::clone(&peer), host_id, &term)
                        .await;
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Ok(PeerMessage::LaunchAgentResult { error, .. }) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(error.unwrap_or_else(|| "remote launch failed".into()));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Err(e) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("remote launch error: {e}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                _ => {}
            }
        });
    }

    pub fn send_remote_pty_input(self: &Arc<Self>, host_id: Uuid, terminal: &str, bytes: Vec<u8>) {
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        let data_b64 = B64.encode(bytes);
        self.spawn_bg(async move {
            if let Some(peer) = hub.peers.lock().await.get(&host_id).cloned() {
                let _ = peer
                    .send_oneway(PeerMessage::PtyInput { terminal, data_b64 }, TrafficLane::ControlP0)
                    .await;
            }
        });
    }

    pub fn send_remote_interrupt(self: &Arc<Self>, host_id: Uuid, terminal: &str) {
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            if let Some(peer) = hub.peers.lock().await.get(&host_id).cloned() {
                let _ = peer
                    .send_oneway(
                        PeerMessage::PtyInput {
                            terminal: terminal.clone(),
                            data_b64: B64.encode([0x03u8]),
                        },
                        TrafficLane::ControlP0,
                    )
                    .await;
                let _ = peer
                    .request(
                        PeerMessage::InterruptTerminal {
                            req_id: format!("int-{}", Uuid::new_v4().simple()),
                            terminal,
                        },
                        TrafficLane::ControlP0,
                        Duration::from_secs(4),
                    )
                    .await;
            }
        });
    }

    pub fn detach_remote_pty(self: &Arc<Self>, host_id: Uuid, terminal: &str) {
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            hub.attached_terms
                .lock()
                .await
                .remove(&(host_id, terminal.clone()));
            if let Some(peer) = hub.peers.lock().await.get(&host_id).cloned() {
                let _ = peer
                    .request(
                        PeerMessage::PtyDetach {
                            req_id: format!("det-{}", Uuid::new_v4().simple()),
                            terminal,
                        },
                        TrafficLane::ControlP0,
                        Duration::from_secs(4),
                    )
                    .await;
            }
        });
    }

    pub fn kill_remote_terminal(self: &Arc<Self>, host_id: Uuid, terminal: &str) {
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            hub.attached_terms
                .lock()
                .await
                .remove(&(host_id, terminal.clone()));
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                return;
            };
            match peer
                .request(
                    PeerMessage::KillTerminal {
                        req_id: format!("kt-{}", Uuid::new_v4().simple()),
                        terminal: terminal.clone(),
                    },
                    TrafficLane::ControlP0,
                    Duration::from_secs(6),
                )
                .await
            {
                Ok(PeerMessage::ActionAck { ok: false, detail, .. }) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote kill failed: {detail}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Err(e) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote kill error ({terminal}): {e}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                _ => {}
            }
        });
    }

    pub fn refresh_remote_directory(
        self: &Arc<Self>,
        host_id: Uuid,
        space_id: &str,
        root_path: &str,
        sub_path: &str,
    ) {
        let hub = Arc::clone(self);
        let space_id = space_id.to_string();
        let root_path = root_path.to_string();
        let sub_path = sub_path.to_string();
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                return;
            };
            let req = PeerMessage::ListDirectory {
                req_id: format!("ls-{}", Uuid::new_v4().simple()),
                root_path: root_path.clone(),
                sub_path: sub_path.clone(),
            };
            if let Ok(PeerMessage::DirectoryListing { entries, error, .. }) = peer
                .request(req, TrafficLane::BulkP1, Duration::from_secs(8))
                .await
            {
                if let Some(err) = error {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote ls error: {err}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                    return;
                }
                let root_trim = root_path.trim_end_matches('/');
                let mapped: Vec<WorkspaceFileEntry> = entries
                    .into_iter()
                    .map(|item: RemoteFileItem| WorkspaceFileEntry {
                        name: item.name,
                        full_path: format!("{root_trim}/{}", item.rel_path),
                        rel_path: item.rel_path,
                        is_dir: item.is_dir,
                        size_bytes: item.size_bytes,
                    })
                    .collect();
                if let Ok(mut g) = hub.state.lock() {
                    if g.active_space_id.as_deref() == Some(space_id.as_str()) {
                        g.dir_entries = mapped;
                    }
                }
                hub.ui_dirty.store(true, Ordering::Release);
            }
        });
    }

    pub fn open_remote_file_preview(
        self: &Arc<Self>,
        host_id: Uuid,
        full_path: &str,
        rel_path: &str,
    ) {
        let hub = Arc::clone(self);
        let full_path = full_path.to_string();
        let rel_path = rel_path.to_string();
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some("Remote host is not connected".to_string());
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let req = PeerMessage::ReadFilePreview {
                req_id: format!("rf-{}", Uuid::new_v4().simple()),
                path: full_path,
                max_bytes: 120_000,
            };
            match peer
                .request(req, TrafficLane::BulkP1, Duration::from_secs(8))
                .await
            {
                Ok(PeerMessage::FilePreviewResponse { content, error, .. }) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.selected_file_rel = Some(rel_path);
                        g.selected_file_content = match error {
                            Some(err) => format!("[Error reading remote file: {err}]"),
                            None => content,
                        };
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Err(e) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote read error: {e}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                _ => {}
            }
        });
    }

    pub fn refresh_remote_git_diff(
        self: &Arc<Self>,
        host_id: Uuid,
        space_id: &str,
        cwd: &str,
    ) {
        let hub = Arc::clone(self);
        let space_id = space_id.to_string();
        let cwd = cwd.to_string();
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                return;
            };
            let req = PeerMessage::GitDiffRequest {
                req_id: format!("gd-{}", Uuid::new_v4().simple()),
                cwd: cwd.clone(),
            };
            if let Ok(PeerMessage::GitDiffResponse {
                branch,
                status_short,
                diff_unified,
                ..
            }) = peer
                .request(req, TrafficLane::BulkP1, Duration::from_secs(8))
                .await
            {
                let changed_files = parse_git_status_short(&status_short);
                let clean_branch = if branch.trim().is_empty() {
                    "online".to_string()
                } else {
                    branch.trim().to_string()
                };
                if let Ok(mut g) = hub.state.lock() {
                    if let Some(sp) = g.spaces.iter_mut().find(|s| s.id == space_id) {
                        sp.branch = clean_branch.clone();
                        sp.dirty_files = changed_files.len();
                    }
                    if g.active_space_id.as_deref() == Some(space_id.as_str()) {
                        let repo_name = Path::new(&cwd)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("remote")
                            .to_string();
                        g.repo_snapshot = RepoInspectorSnapshot {
                            repo_path: cwd.clone(),
                            repo_name,
                            is_git_repo: !status_short.is_empty() || !diff_unified.is_empty() || clean_branch != "online",
                            branch: clean_branch,
                            head_summary: format!("{} changed (remote)", changed_files.len()),
                            diff_added: diff_unified.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count() as u32,
                            diff_deleted: diff_unified.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count() as u32,
                            changed_files,
                            diff_title: format!("{cwd} · remote diff"),
                            diff_text: if diff_unified.trim().is_empty() {
                                "Clean working tree on remote host.".to_string()
                            } else {
                                diff_unified
                            },
                            ..Default::default()
                        };
                    }
                }
                hub.ui_dirty.store(true, Ordering::Release);
            }
        });
    }

    pub fn probe_host(self: &Arc<Self>, host: RemoteHostSummary) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = DaemonConfig::load(&DaemonConfig::default_path()) else {
                return;
            };
            let exact_prefix = format!("remote:{}", host.id);
            let repo_prefix = format!("remote:{}:", host.id);
            if let Ok(mut g) = hub.state.lock() {
                g.dismissed_spaces
                    .retain(|id| id != &exact_prefix && !id.starts_with(&repo_prefix));
            }
            let mut prefs = DesktopPreferences::load_or_default();
            let before_len = prefs.dismissed_spaces.len();
            prefs
                .dismissed_spaces
                .retain(|id| id != &exact_prefix && !id.starts_with(&repo_prefix));
            if prefs.dismissed_spaces.len() != before_len {
                let _ = prefs.save();
            }

            match hub.get_or_connect_peer(&cfg, &host).await {
                Ok(peer) => {
                    let route = peer.route_label().to_string();
                    hub.sync_single_remote_host(&cfg, &host).await;
                    let host_id_str = host.id.to_string();
                    let (term_count, target_space) = hub
                        .state
                        .lock()
                        .map(|mut g| {
                            let found_sp = g
                                .spaces
                                .iter()
                                .find(|s| s.machine_id == host_id_str)
                                .map(|s| (s.id.clone(), s.path.clone()));
                            if let Some((ref sp_id, _)) = found_sp {
                                g.active_space_id = Some(sp_id.clone());
                                g.current_browse_rel.clear();
                                g.selected_file_rel = None;
                                g.selected_file_content.clear();
                                g.selected_change_path = None;
                                g.active_session_id = g
                                    .sessions
                                    .iter()
                                    .find(|s| &s.space_id == sp_id)
                                    .map(|s| s.id.clone());
                                if g.layout.split_terminals {
                                    g.split_session_id = g
                                        .sessions
                                        .iter()
                                        .find(|s| {
                                            &s.space_id == sp_id
                                                && Some(&s.id) != g.active_session_id.as_ref()
                                        })
                                        .map(|s| s.id.clone());
                                }
                                g.input_target = ActiveInputTarget::TerminalDirect;
                            }
                            let cnt = g
                                .sessions
                                .iter()
                                .filter(|s| s.remote_host_id.as_deref() == Some(host_id_str.as_str()))
                                .count();
                            (cnt, found_sp)
                        })
                        .unwrap_or((0, None));
                    if let Some((sp_id, sp_path)) = target_space {
                        let mut p = DesktopPreferences::load_or_default();
                        p.selected_repo_path = Some(sp_id.clone());
                        let _ = p.save();
                        hub.refresh_remote_git_diff(host.id, &sp_id, &sp_path);
                        if sp_path != "." {
                            hub.refresh_remote_directory(host.id, &sp_id, &sp_path, "");
                        }
                    }
                    if let Ok(mut g) = hub.state.lock() {
                        g.remote_probe_status =
                            Some(format!("{} · {route} · {term_count} sessions", host.name));
                        g.status_notice = g.remote_probe_status.clone();
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("{} unreachable: {err}", host.name));
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }
}

fn extract_file_diff_lines<'a>(diff_text: &'a str, selected_path: Option<&str>) -> Vec<&'a str> {
    if let Some(target) = selected_path {
        if diff_text.matches("diff --git ").count() > 1 {
            let b_suffix = format!(" b/{target}");
            let a_prefix = format!("diff --git a/{target} ");
            let mut in_block = false;
            let mut filtered = Vec::new();
            for line in diff_text.lines() {
                if line.starts_with("diff --git ") {
                    in_block = line.ends_with(&b_suffix) || line.starts_with(&a_prefix);
                }
                if in_block {
                    filtered.push(line);
                    if filtered.len() >= 160 {
                        break;
                    }
                }
            }
            if !filtered.is_empty() {
                return filtered;
            }
        }
    }
    diff_text.lines().take(160).collect()
}

fn spawn_device_login(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    ui_dirty: Arc<AtomicBool>,
    mesh_hub: Arc<RemoteMeshHub>,
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
                            let _ = cfg.save(&DaemonConfig::default_path());
                            if let Ok(mut g) = state_bg.lock() {
                                g.is_logged_in = true;
                                g.auth_form_expanded = false;
                                g.input_target = ActiveInputTarget::TerminalDirect;
                                g.account_email = cfg
                                    .owner_email
                                    .clone()
                                    .unwrap_or_else(|| cfg.owner_handle.clone());
                                g.device_login_code = None;
                                g.status_notice = Some("Signed in".to_string());
                            }
                            ui_dirty.store(true, Ordering::Release);
                            mesh_hub.ensure_embedded_host();
                            mesh_hub.start_account_sync();
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
    mesh_hub: Arc<RemoteMeshHub>,
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
                    let _ = cfg.save(&DaemonConfig::default_path());
                    if let Ok(mut g) = state_bg.lock() {
                        g.is_logged_in = true;
                        g.auth_form_expanded = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                        g.account_email = cfg.owner_email.clone().unwrap_or(email);
                        g.auth_password_input.clear();
                        g.status_notice = Some("Signed in".to_string());
                    }
                    ui_dirty.store(true, Ordering::Release);
                    mesh_hub.ensure_embedded_host();
                    mesh_hub.start_account_sync();
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

// ============================================================================
// Application Launcher & Direct PTY / Palette Keyboard Loop
// ============================================================================

pub fn launch_gpui_desktop_app() {
    ensure_macos_gui_path();

    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", QuitApp, None),
            KeyBinding::new("cmd-,", ToggleSettings, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
            KeyBinding::new("cmd-n", NewTerminal, None),
            KeyBinding::new("cmd-t", NewTerminal, None),
            KeyBinding::new("cmd-w", CloseActiveTerminal, None),
            KeyBinding::new("cmd-d", ToggleSplit, None),
            KeyBinding::new("cmd-p", ToggleCommandPalette, None),
            KeyBinding::new("cmd-k", ToggleCommandPalette, None),
            KeyBinding::new("cmd-1", SwitchLayoutWorkbench, None),
            KeyBinding::new("cmd-2", SwitchLayoutTerminals, None),
            KeyBinding::new("ctrl-c", InterruptActiveTerminal, None),
        ]);
        cx.set_menus(vec![
            Menu {
                name: "Coda".into(),
                items: vec![
                    MenuItem::action("Settings…", ToggleSettings),
                    MenuItem::separator(),
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action("Quit Coda", QuitApp),
                ],
            },
            Menu {
                name: "Shell".into(),
                items: vec![
                    MenuItem::action("New Terminal", NewTerminal),
                    MenuItem::action("Toggle Split Terminals", ToggleSplit),
                    MenuItem::separator(),
                    MenuItem::action("Close Terminal", CloseActiveTerminal),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Command Palette…", ToggleCommandPalette),
                    MenuItem::action("Toggle Sidebar", ToggleSidebar),
                    MenuItem::separator(),
                    MenuItem::action("Main Workbench", SwitchLayoutWorkbench),
                    MenuItem::action("Terminals Grid", SwitchLayoutTerminals),
                ],
            },
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
                        traffic_light_position: Some(point(px(14.0), px(8.0))),
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

        let view_settings = view.clone();
        cx.on_action(move |_: &ToggleSettings, cx| {
            view_settings.update(cx, |this, cx| {
                this.toggle_settings_modal();
                cx.notify();
            });
        });

        cx.observe_keystrokes(move |ev, _, cx| {
            let key = ev.keystroke.key.as_str();
            let mods = &ev.keystroke.modifiers;

            // Cmd+V Paste into focused input target or active PTY
            if mods.platform && key == "v" {
                if let Some(txt) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    view.update(cx, |this, cx| {
                        let (target, settings_open, focused_id) = this
                            .state
                            .lock()
                            .map(|g| {
                                (
                                    g.input_target,
                                    g.settings_open,
                                    g.focused_session_id().map(|s| s.to_string()),
                                )
                            })
                            .unwrap_or((ActiveInputTarget::TerminalDirect, false, None));
                        match target {
                            ActiveInputTarget::TerminalDirect => {
                                if !settings_open {
                                    if let Some(id) = focused_id {
                                        this.send_bytes_to_session(&id, txt.as_bytes());
                                    }
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
                let (target, palette_open, settings_open, focused_id) = this
                    .state
                    .lock()
                    .map(|g| {
                        (
                            g.input_target,
                            g.palette_open,
                            g.settings_open,
                            g.focused_session_id().map(|s| s.to_string()),
                        )
                    })
                    .unwrap_or((ActiveInputTarget::TerminalDirect, false, false, None));

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

                if settings_open && target == ActiveInputTarget::TerminalDirect {
                    if key == "escape" {
                        if let Ok(mut g) = this.state.lock() {
                            g.settings_open = false;
                        }
                        cx.notify();
                    }
                    return;
                }

                if target == ActiveInputTarget::TerminalDirect {
                    if let Some(id) = focused_id {
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
                                this.send_bytes_to_session(&id, &[b]);
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
                            this.send_bytes_to_session(&id, bytes);
                        } else if let Some(ref ch) = ev.keystroke.key_char {
                            this.send_bytes_to_session(&id, ch.as_bytes());
                        }
                        cx.notify();
                        return;
                    }
                }

                // Auth form input fields inside Mesh drawer or Settings -> Machines
                match target {
                    ActiveInputTarget::AuthServerUrl => match key {
                        "backspace" => {
                            if let Ok(mut g) = this.state.lock() {
                                g.server_url.pop();
                            }
                        }
                        "enter" => {
                            let url = this
                                .state
                                .lock()
                                .map(|g| g.server_url.clone())
                                .unwrap_or_default();
                            this.save_server_url_preference(&url);
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
                                Arc::clone(&this.mesh_hub),
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
        assert!(!state.settings_open);
        assert_eq!(state.settings_section, SettingsSection::Appearance);
    }

    #[test]
    fn test_settings_palette_entry_and_search() {
        let mut state = CodaDesktopState::default();
        let all = state.palette_items();
        assert!(
            all.iter()
                .any(|item| matches!(item.action, PaletteAction::OpenSettings)),
            "Command Palette must expose OpenSettings"
        );

        state.palette_query = "settings".to_string();
        let filtered = state.palette_items();
        assert!(
            filtered
                .iter()
                .any(|item| matches!(item.action, PaletteAction::OpenSettings)),
            "searching 'settings' in Command Palette must match OpenSettings"
        );
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

        let terminals_raw =
            r##"{"layout":"terminals","split_terminals":true,"accent_hex":"#f59e0b"}"##;
        let terminals_parsed: MalleableLayoutConfig = serde_json::from_str(terminals_raw).unwrap();
        assert_eq!(terminals_parsed.layout, LayoutMode::Terminals);
        assert!(terminals_parsed.split_terminals);
        assert_eq!(terminals_parsed.accent_color_u32(), 0xf59e0b);

        for legacy in ["code", "board", "main"] {
            let raw = format!(r#"{{"layout":"{legacy}"}}"#);
            let parsed: MalleableLayoutConfig = serde_json::from_str(&raw).unwrap();
            assert_eq!(parsed.layout, LayoutMode::Workbench);
        }
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

    #[test]
    fn test_real_desktop_local_and_remote_spawn_open_close_flow() {
        let cwd = std::env::current_dir().unwrap();
        let src_dir = cwd.join("src");
        let entries = list_workspace_dir(&cwd, &src_dir);
        assert!(
            entries.iter().any(|e| e.name == "lib.rs" && !e.is_dir),
            "expected src/lib.rs when browsing subdir: {:?}",
            entries.iter().map(|e| &e.rel_path).collect::<Vec<_>>()
        );
        let preview = read_workspace_file_preview(&cwd.join("Cargo.toml"), 4096);
        assert!(preview.contains("coda-desktop"));

        let prefs_json = r#"{"locale":"zh","server_url":"http://127.0.0.1:8080","workspace_roots":[],"selected_repo_path":"local:/tmp/demo","dismissed_spaces":["local:/tmp/dismissed"]}"#;
        let prefs: DesktopPreferences = serde_json::from_str(prefs_json).unwrap();
        assert_eq!(prefs.dismissed_spaces, vec!["local:/tmp/dismissed".to_string()]);
        assert_eq!(prefs.selected_repo_path.as_deref(), Some("local:/tmp/demo"));

        if let Some(hc_bin) = discover_hc_binary() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let hc = HerdrClient::connect_with_bin(hc_bin.clone()).await.unwrap();
                let term = hc
                    .launch_agent(
                        "shell",
                        Some("coda 中文 test · hc"),
                        Some(&cwd.to_string_lossy()),
                        None,
                        100,
                        24,
                    )
                    .await
                    .expect("launch_agent with unicode display title must sanitize and succeed");
                let sid = format!("hc-{}", term.address);

                let mut st = CodaDesktopState::default();
                st.did_auto_seed_local = true;
                st.closed_session_ids.insert(sid.clone());
                let state = Arc::new(Mutex::new(st));
                let dirty = Arc::new(AtomicBool::new(false));
                let pty_mgr = Arc::new(Mutex::new(LocalPtyManager::new(Arc::clone(&dirty))));

                reconcile_local_herdr_sessions(&hc, &hc_bin, &state, &pty_mgr, &dirty).await;
                {
                    let g = state.lock().unwrap();
                    assert!(
                        !g.sessions.iter().any(|s| s.id == sid),
                        "closed_session_ids tombstone must prevent reconcile from resurrecting {sid}"
                    );
                }

                state.lock().unwrap().closed_session_ids.clear();
                reconcile_local_herdr_sessions(&hc, &hc_bin, &state, &pty_mgr, &dirty).await;
                {
                    let g = state.lock().unwrap();
                    assert!(
                        g.sessions.iter().any(|s| s.id == sid),
                        "reconcile should attach live session once tombstone removed"
                    );
                }

                if let Ok(mut mgr) = pty_mgr.lock() {
                    mgr.kill_session(&sid);
                }
                hc.kill_and_remove(&term.address).await.unwrap();
                let live_after = hc.list_terminals(false).await.unwrap_or_default();
                assert!(
                    !live_after.iter().any(|t| t.address == term.address),
                    "killed terminal must not remain in live herdr-core list"
                );
            });
        }
    }
}
