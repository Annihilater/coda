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
pub mod helpers;
pub mod notify;
pub mod pty;
pub mod remote_mesh;
pub mod smart_path;

pub use crate::remote_mesh::RemoteMeshHub;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use coda_daemon::{active_daemon_lock_pid, DaemonConfig, RemoteHostSummary};
use coda_herdr::{discover_hc_binary, ensure_or_install_hc_binary, HerdrClient};
use gpui::{
    actions, canvas, div, point, prelude::*, px, rgb, rgba, size, App, Application, Bounds,
    ClickEvent, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    Focusable, FontWeight, HighlightStyle, Hsla, KeyBinding, KeyDownEvent, Menu, MenuItem,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollDelta,
    ScrollWheelEvent, SharedString, StyledText, SystemMenuType, TitlebarOptions, UTF16Selection,
    Window, WindowBounds, WindowOptions,
};
use uuid::Uuid;

use crate::config::{
    config_root_dir, detect_local_machine_name, discover_local_git_repositories,
    display_compact_path, ensure_agents_md_manual, ensure_macos_gui_path, is_safe_http_url,
    pick_folder_via_native_dialog, reveal_in_finder, space_avatar_color, tr, DesktopPreferences,
    LayoutMode, MalleableLayoutConfig, UiLocale,
};
use crate::git_inspector::{
    compute_repo_snapshot, list_workspace_dir, read_workspace_file_preview, resolve_contained_path,
    RepoInspectorSnapshot, WorkspaceFileEntry,
};
use crate::helpers::{
    extract_file_diff_lines, herdr_status_to_osc, load_desktop_daemon_config, utf16_slice,
};
#[cfg(test)]
use crate::helpers::path_within_root;
use crate::notify::{dispatch_native_notification, DesktopNotificationTracker};
use crate::pty::{AgentLifecycleState, LocalPtyManager, Osc7501Status, StyledTerminalLine};
use crate::remote_mesh::{spawn_device_login, spawn_password_login};

actions!(
    coda_desktop,
    [
        QuitApp,
        NewWindow,
        ToggleSettings,
        ToggleSidebar,
        NewTerminal,
        CloseActiveTerminal,
        ToggleSplit,
        ToggleCommandPalette,
        SwitchLayoutWorkbench,
        SwitchLayoutTerminals,
        InterruptActiveTerminal,
        CopyTerminalAction,
        PasteTerminalAction,
        SelectAllAction,
        ZoomInTerminal,
        ZoomOutTerminal,
        ZoomResetTerminal,
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
    RenameDeviceInput,
    ChangePwCurrent,
    ChangePwNew,
}

/// Primary or Secondary split terminal pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    pub input_selected_all: bool,
    pub is_logged_in: bool,
    pub auth_loading: bool,
    pub auth_error: Option<String>,
    pub auth_form_expanded: bool,
    pub account_email: String,
    pub allow_remote_control: bool,
    pub desktop_notifications: bool,
    pub notify_tracker: DesktopNotificationTracker,
    pub embedded_host_active: bool,
    pub external_daemon_running: bool,
    pub local_host_device_id: Option<Uuid>,
    pub remote_hosts: Vec<RemoteHostSummary>,
    pub selected_remote_host_id: Option<String>,
    pub remote_probe_status: Option<String>,
    pub device_login_code: Option<String>,
    pub device_login_uri: Option<String>,
    pub renaming_host_id: Option<Uuid>,
    pub rename_host_input: String,
    pub pw_change_expanded: bool,
    pub pw_current_input: String,
    pub pw_new_input: String,
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
            input_selected_all: false,
            is_logged_in: false,
            auth_loading: false,
            auth_error: None,
            auth_form_expanded: false,
            account_email: String::new(),
            allow_remote_control: true,
            desktop_notifications: true,
            notify_tracker: DesktopNotificationTracker::default(),
            embedded_host_active: false,
            external_daemon_running: false,
            local_host_device_id: None,
            remote_hosts: Vec::new(),
            selected_remote_host_id: None,
            remote_probe_status: None,
            device_login_code: None,
            device_login_uri: None,
            renaming_host_id: None,
            rename_host_input: String::new(),
            pw_change_expanded: false,
            pw_current_input: String::new(),
            pw_new_input: String::new(),
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

    pub fn record_session_osc_status(&mut self, session_id: &str, osc: Osc7501Status) {
        let title = if let Some(sess) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            sess.osc_status = Some(osc.clone());
            sess.title.clone()
        } else {
            return;
        };
        if !self.desktop_notifications {
            return;
        }
        let active_id = self.focused_session_id().map(str::to_string);
        if let Some(payload) = self.notify_tracker.observe(
            session_id,
            &title,
            &osc,
            active_id.as_deref(),
            self.locale,
            Instant::now(),
        ) {
            dispatch_native_notification(payload);
        }
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

#[derive(Debug, Clone)]
struct TerminalSelection {
    session_id: String,
    anchor: (usize, usize),
    focus: (usize, usize),
    dragging: bool,
}

impl TerminalSelection {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    fn is_non_empty(&self) -> bool {
        self.anchor != self.focus
    }
}

#[derive(Debug, Clone, Copy)]
struct PaneGeometry {
    bounds: Bounds<Pixels>,
    start_line: usize,
    total_lines: usize,
    cell_w: f32,
    row_step: f32,
}

/// Primary GPUI workbench view.
pub struct CodaGpuWorkspaceView {
    pub state: Arc<Mutex<CodaDesktopState>>,
    pub pty_mgr: Arc<Mutex<LocalPtyManager>>,
    pub ui_dirty: Arc<AtomicBool>,
    pub mesh_hub: Arc<RemoteMeshHub>,
    pub focus_handle: FocusHandle,
    ime_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    marked_text: String,
    viewport_dims: Arc<Mutex<HashMap<String, (u16, u16)>>>,
    visible_rows_by_pane: Arc<Mutex<HashMap<SplitPane, usize>>>,
    pane_geometry: Arc<Mutex<HashMap<SplitPane, PaneGeometry>>>,
    terminal_selection: Arc<Mutex<Option<TerminalSelection>>>,
    scroll_pixel_acc: Mutex<HashMap<String, f32>>,
    pty_synced_gen: Mutex<HashMap<String, (u64, bool)>>,
    inspector_seq: Arc<AtomicU64>,
}

impl Focusable for CodaGpuWorkspaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for CodaGpuWorkspaceView {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let len = self.marked_text.encode_utf16().count();
        let clamped_start = range.start.min(len);
        let clamped_end = range.end.min(len).max(clamped_start);
        *adjusted_range = Some(clamped_start..clamped_end);
        Some(utf16_slice(&self.marked_text, clamped_start..clamped_end))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self.marked_text.encode_utf16().count();
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let len = self.marked_text.encode_utf16().count();
        if len == 0 {
            None
        } else {
            Some(0..len)
        }
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.marked_text.is_empty() {
            self.marked_text.clear();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text.clear();
        if !text.is_empty() {
            self.commit_text_to_active_target(text);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = new_text.to_string();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.ime_bounds
            .lock()
            .ok()
            .and_then(|g| *g)
            .or(Some(element_bounds))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl CodaGpuWorkspaceView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        ensure_macos_gui_path();

        let prefs = DesktopPreferences::load_or_default();
        let (layout_cfg, layout_mtime) = MalleableLayoutConfig::load_with_mtime();

        let mut init_state = CodaDesktopState::default();
        init_state.locale = prefs.locale;
        init_state.server_url = prefs.server_url.clone();
        init_state.allow_remote_control = prefs.allow_remote_control;
        init_state.desktop_notifications = prefs.desktop_notifications;
        init_state.layout = layout_cfg;
        init_state.layout_mtime = layout_mtime;
        init_state.hc_path = discover_hc_binary();
        init_state.hc_available = init_state.hc_path.is_some();

        // Hydrate daemon login identity if configured on disk (with macOS Keychain token read-back)
        let daemon_cfg_path = DaemonConfig::default_path();
        init_state.external_daemon_running = active_daemon_lock_pid(&daemon_cfg_path).is_some();
        if let Ok(dcfg) = load_desktop_daemon_config(&daemon_cfg_path) {
            init_state.server_url = dcfg.server_url.clone();
            init_state.local_host_device_id = Some(dcfg.device_id);
            if dcfg.session_token.is_some() {
                init_state.is_logged_in = true;
                init_state.account_email = dcfg
                    .owner_email
                    .clone()
                    .unwrap_or_else(|| dcfg.owner_handle.clone());
            }
        }

        init_state.dismissed_spaces = prefs.dismissed_spaces.iter().cloned().collect();

        // Seed fast placeholder local spaces from saved roots so GPUI first paint never blocks on git subprocesses.
        let machine_label = init_state.local_machine_name.clone();
        let mut seen_local_paths: HashSet<String> = HashSet::new();
        for raw_root in &prefs.workspace_roots {
            let p = PathBuf::from(raw_root.trim());
            if !p.is_dir() {
                continue;
            }
            let canon = p.canonicalize().unwrap_or(p);
            let path_str = canon.to_string_lossy().to_string();
            if !seen_local_paths.insert(path_str.clone()) {
                continue;
            }
            if init_state.dismissed_spaces.contains(&path_str)
                || init_state.dismissed_spaces.contains(&format!("local:{path_str}"))
            {
                continue;
            }
            let name = canon
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("workspace")
                .to_string();
            init_state.spaces.push(SpaceEntry {
                id: format!("local:{path_str}"),
                avatar_color: space_avatar_color(&name),
                name,
                path: path_str,
                machine_id: "local".to_string(),
                machine_label: machine_label.clone(),
                is_remote: false,
                branch: "…".to_string(),
                dirty_files: 0,
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
        let inspector_seq = Arc::new(AtomicU64::new(0));

        let view = Self {
            state: Arc::clone(&state),
            pty_mgr: Arc::clone(&pty_mgr),
            ui_dirty: Arc::clone(&ui_dirty),
            mesh_hub: Arc::clone(&mesh_hub),
            focus_handle: cx.focus_handle(),
            ime_bounds: Arc::new(Mutex::new(None)),
            marked_text: String::new(),
            viewport_dims: Arc::new(Mutex::new(HashMap::new())),
            visible_rows_by_pane: Arc::new(Mutex::new(HashMap::new())),
            pane_geometry: Arc::new(Mutex::new(HashMap::new())),
            terminal_selection: Arc::new(Mutex::new(None)),
            scroll_pixel_acc: Mutex::new(HashMap::new()),
            pty_synced_gen: Mutex::new(HashMap::new()),
            inspector_seq: Arc::clone(&inspector_seq),
        };

        // Run filesystem/git discovery and default config creation off the GPUI UI thread.
        let state_bg = Arc::clone(&state);
        let ui_dirty_bg = Arc::clone(&ui_dirty);
        let saved_roots = prefs.workspace_roots.clone();
        let selected_repo = prefs.selected_repo_path.clone();
        let dismissed_clone = prefs.dismissed_spaces.clone();
        let seq_bg = Arc::clone(&inspector_seq);
        std::thread::spawn(move || {
            MalleableLayoutConfig::save_default_if_missing();
            ensure_agents_md_manual();

            let mut candidates: Vec<PathBuf> = discover_local_git_repositories(&saved_roots)
                .into_iter()
                .filter(|p| {
                    !dismissed_clone.contains(p)
                        && !dismissed_clone.contains(&format!("local:{p}"))
                })
                .map(PathBuf::from)
                .collect();
            if candidates.is_empty() {
                if let Ok(home) = std::env::var("HOME") {
                    if !dismissed_clone.contains(&home)
                        && !dismissed_clone.contains(&format!("local:{home}"))
                    {
                        candidates.push(PathBuf::from(home));
                    }
                } else if let Ok(cwd) = std::env::current_dir() {
                    let cwd_s = cwd.to_string_lossy().to_string();
                    if !dismissed_clone.contains(&cwd_s)
                        && !dismissed_clone.contains(&format!("local:{cwd_s}"))
                    {
                        candidates.push(cwd);
                    }
                }
            }

            let mut discovered_spaces = Vec::new();
            for path in candidates {
                let snap = compute_repo_snapshot(&path, None);
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("workspace")
                    .to_string();
                let path_str = path.to_string_lossy().to_string();
                discovered_spaces.push((
                    SpaceEntry {
                        id: format!("local:{path_str}"),
                        avatar_color: space_avatar_color(&name),
                        name,
                        path: path_str,
                        machine_id: "local".to_string(),
                        machine_label: machine_label.clone(),
                        is_remote: false,
                        branch: snap.branch.clone(),
                        dirty_files: snap.changed_files.len(),
                        ahead: 0,
                        behind: 0,
                    },
                    snap,
                ));
            }

            let first_inspect: Option<(String, PathBuf)> = {
                let Ok(mut g) = state_bg.lock() else {
                    return;
                };
                for (sp, snap) in &discovered_spaces {
                    if !g.dismissed_spaces.contains(&sp.id)
                        && !g.dismissed_spaces.contains(&sp.path)
                    {
                        if let Some(existing) = g.spaces.iter_mut().find(|s| s.id == sp.id) {
                            existing.branch = snap.branch.clone();
                            existing.dirty_files = snap.changed_files.len();
                        } else {
                            g.spaces.push(sp.clone());
                        }
                    }
                }
                if g.active_space_id.is_none() {
                    g.active_space_id = selected_repo
                        .as_deref()
                        .and_then(|sel| {
                            g.spaces
                                .iter()
                                .find(|s| s.id == sel || s.path == sel)
                                .map(|s| s.id.clone())
                        })
                        .or_else(|| g.spaces.first().map(|s| s.id.clone()));
                }
                g.active_space()
                    .filter(|sp| !sp.is_remote)
                    .map(|sp| (sp.id.clone(), PathBuf::from(&sp.path)))
            };

            if let Some((sp_id, sp_path)) = first_inspect {
                let cur_seq = seq_bg.load(Ordering::Acquire);
                let snap = compute_repo_snapshot(&sp_path, None);
                let entries = list_workspace_dir(&sp_path, &sp_path);
                if seq_bg.load(Ordering::Acquire) == cur_seq {
                    if let Ok(mut g) = state_bg.lock() {
                        if g.active_space_id.as_deref() == Some(sp_id.as_str()) {
                            g.repo_snapshot = snap;
                            g.dir_entries = entries;
                        }
                    }
                }
            }
            ui_dirty_bg.store(true, Ordering::Release);
        });

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

    fn update_prefs_async<F>(f: F)
    where
        F: FnOnce(&mut DesktopPreferences) + Send + 'static,
    {
        std::thread::spawn(move || {
            let mut prefs = DesktopPreferences::load_or_default();
            f(&mut prefs);
            let _ = prefs.save();
        });
    }

    fn keystroke_to_terminal_bytes(
        key: &str,
        mods: &gpui::Modifiers,
        key_char: Option<&str>,
        app_cursor_keys: bool,
    ) -> Option<Vec<u8>> {
        if mods.control && !mods.platform && !mods.alt {
            match key {
                "space" => return Some(vec![0x00]),
                "up" => return Some(b"\x1b[1;5A".to_vec()),
                "down" => return Some(b"\x1b[1;5B".to_vec()),
                "right" => return Some(b"\x1b[1;5C".to_vec()),
                "left" => return Some(b"\x1b[1;5D".to_vec()),
                "home" => return Some(b"\x1b[1;5H".to_vec()),
                "end" => return Some(b"\x1b[1;5F".to_vec()),
                _ => {}
            }
            if key.len() == 1 {
                let b = key.as_bytes()[0].to_ascii_lowercase();
                return match b {
                    b'a'..=b'z' => Some(vec![b - b'a' + 1]),
                    b'@' => Some(vec![0x00]),
                    b'[' => Some(vec![0x1b]),
                    b'\\' => Some(vec![0x1c]),
                    b']' => Some(vec![0x1d]),
                    b'^' => Some(vec![0x1e]),
                    b'_' => Some(vec![0x1f]),
                    _ => None,
                };
            }
            return None;
        }

        if mods.alt && !mods.platform && !mods.control {
            match key {
                "backspace" => return Some(vec![0x1b, 0x7f]),
                "left" => return Some(b"\x1b[1;3D".to_vec()),
                "right" => return Some(b"\x1b[1;3C".to_vec()),
                "up" => return Some(b"\x1b[1;3A".to_vec()),
                "down" => return Some(b"\x1b[1;3B".to_vec()),
                _ => {
                    if let Some(ch) = key_char.filter(|s| !s.is_empty()) {
                        let mut out = vec![0x1b];
                        out.extend_from_slice(ch.as_bytes());
                        return Some(out);
                    }
                    if key.len() == 1 && key.is_ascii() {
                        return Some(vec![0x1b, key.as_bytes()[0]]);
                    }
                    return None;
                }
            }
        }

        if mods.control || mods.platform || mods.alt {
            return None;
        }

        if mods.shift {
            match key {
                "up" => return Some(b"\x1b[1;2A".to_vec()),
                "down" => return Some(b"\x1b[1;2B".to_vec()),
                "right" => return Some(b"\x1b[1;2C".to_vec()),
                "left" => return Some(b"\x1b[1;2D".to_vec()),
                "tab" => return Some(b"\x1b[Z".to_vec()),
                _ => {}
            }
        }

        match key {
            "enter" | "return" => Some(b"\r".to_vec()),
            "backspace" => Some(vec![0x7f]),
            "delete" => Some(b"\x1b[3~".to_vec()),
            "tab" => Some(b"\t".to_vec()),
            "escape" => Some(vec![0x1b]),
            "up" => Some(if app_cursor_keys { b"\x1bOA" } else { b"\x1b[A" }.to_vec()),
            "down" => Some(if app_cursor_keys { b"\x1bOB" } else { b"\x1b[B" }.to_vec()),
            "right" => Some(if app_cursor_keys { b"\x1bOC" } else { b"\x1b[C" }.to_vec()),
            "left" => Some(if app_cursor_keys { b"\x1bOD" } else { b"\x1b[D" }.to_vec()),
            "home" => Some(if app_cursor_keys { b"\x1bOH" } else { b"\x1b[H" }.to_vec()),
            "end" => Some(if app_cursor_keys { b"\x1bOF" } else { b"\x1b[F" }.to_vec()),
            "pageup" => Some(b"\x1b[5~".to_vec()),
            "pagedown" => Some(b"\x1b[6~".to_vec()),
            "f1" => Some(b"\x1bOP".to_vec()),
            "f2" => Some(b"\x1bOQ".to_vec()),
            "f3" => Some(b"\x1bOR".to_vec()),
            "f4" => Some(b"\x1bOS".to_vec()),
            "f5" => Some(b"\x1b[15~".to_vec()),
            "f6" => Some(b"\x1b[17~".to_vec()),
            "f7" => Some(b"\x1b[18~".to_vec()),
            "f8" => Some(b"\x1b[19~".to_vec()),
            "f9" => Some(b"\x1b[20~".to_vec()),
            "f10" => Some(b"\x1b[21~".to_vec()),
            "f11" => Some(b"\x1b[23~".to_vec()),
            "f12" => Some(b"\x1b[24~".to_vec()),
            _ => None,
        }
    }

    fn extract_selected_terminal_text(&self, session_id: &str) -> Option<String> {
        let sel = self.terminal_selection.lock().ok()?.clone()?;
        if sel.session_id != session_id || !sel.is_non_empty() {
            return None;
        }
        let ((r0, c0), (r1, c1)) = sel.ordered();
        let g = self.state.lock().ok()?;
        let sess = g.session_by_id(session_id)?;
        if sess.lines.is_empty() {
            return None;
        }
        let last_row = r1.min(sess.lines.len().saturating_sub(1));
        let first_row = r0.min(last_row);
        let mut out_lines = Vec::new();
        for r in first_row..=last_row {
            let line = &sess.lines[r];
            let start_col = if r == first_row { c0 } else { 0 };
            let end_col = if r == last_row { c1 } else { usize::MAX };
            let mut col = 0usize;
            let mut seg = String::new();
            for ch in line.chars() {
                let w = crate::pty::char_display_width(ch).max(1);
                if col + w > start_col && col < end_col {
                    seg.push(ch);
                }
                col += w;
            }
            out_lines.push(seg.trim_end().to_string());
        }
        let joined = out_lines.join("\n");
        if joined.is_empty() { None } else { Some(joined) }
    }

    fn perform_copy_action(&self, force_all_screen: bool, cx: &mut Context<Self>) {
        let copy_res = self.state.lock().ok().and_then(|g| {
            if g.palette_open {
                return (!g.palette_query.is_empty()).then(|| (g.palette_query.clone(), false));
            }
            match g.input_target {
                ActiveInputTarget::AuthServerUrl => {
                    (!g.server_url.is_empty()).then(|| (g.server_url.clone(), false))
                }
                ActiveInputTarget::AuthAccount => {
                    (!g.auth_account_input.is_empty()).then(|| (g.auth_account_input.clone(), false))
                }
                ActiveInputTarget::RenameDeviceInput => {
                    (!g.rename_host_input.is_empty()).then(|| (g.rename_host_input.clone(), false))
                }
                ActiveInputTarget::AuthPassword
                | ActiveInputTarget::ChangePwCurrent
                | ActiveInputTarget::ChangePwNew => None,
                ActiveInputTarget::TerminalDirect if !g.settings_open => {
                    let id = g.focused_session_id()?.to_string();
                    drop(g);
                    if let Some(selected) = self.extract_selected_terminal_text(&id) {
                        return Some((selected, false));
                    }
                    if force_all_screen {
                        let g2 = self.state.lock().ok()?;
                        let sess = g2.session_by_id(&id)?;
                        let joined = sess.lines.join("\n").trim_end().to_string();
                        return (!joined.is_empty()).then_some((joined, true));
                    }
                    None
                }
                _ => None,
            }
        });
        if let Some((text, copied_all)) = copy_res {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            if let Ok(mut g) = self.state.lock() {
                g.status_notice = Some(if copied_all {
                    tr(g.locale, "Copied terminal screen", "已复制终端整屏输出").to_string()
                } else {
                    tr(g.locale, "Copied selection", "已复制选中内容").to_string()
                });
            }
            cx.notify();
        } else if let Ok(mut g) = self.state.lock() {
            if g.input_target == ActiveInputTarget::TerminalDirect
                && !g.settings_open
                && !g.palette_open
            {
                g.status_notice = Some(
                    tr(
                        g.locale,
                        "Drag to select terminal text (or ⌘⇧C to copy screen)",
                        "拖拽选中终端文本后再 ⌘C（或 ⌘⇧C 复制整屏）",
                    )
                    .to_string(),
                );
                cx.notify();
            }
        }
    }

    fn perform_paste_action(&self, cx: &mut Context<Self>) {
        if let Some(txt) = cx.read_from_clipboard().and_then(|i| i.text()) {
            let (target, palette_open, settings_open, focused_id) = self
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
            if !palette_open && !settings_open && target == ActiveInputTarget::TerminalDirect {
                if let Some(id) = focused_id {
                    let bracketed = self
                        .pty_mgr
                        .lock()
                        .map(|m| m.terminal_modes(&id).1)
                        .unwrap_or(false);
                    if let Ok(mut g) = self.state.lock() {
                        if let Some(s) = g.sessions.iter_mut().find(|s| s.id == id) {
                            s.scroll_offset = 0;
                        }
                    }
                    if bracketed {
                        // #38: Wrap paste in ?2004h bracketed paste markers & strip any forged terminator.
                        let safe_txt = txt.replace("\x1b[201~", "");
                        let mut bytes = Vec::with_capacity(safe_txt.len() + 12);
                        bytes.extend_from_slice(b"\x1b[200~");
                        bytes.extend_from_slice(safe_txt.as_bytes());
                        bytes.extend_from_slice(b"\x1b[201~");
                        self.send_bytes_to_session(&id, &bytes);
                    } else {
                        self.send_bytes_to_session(&id, txt.as_bytes());
                    }
                    cx.notify();
                }
                return;
            }
            let payload = if palette_open
                || matches!(
                    target,
                    ActiveInputTarget::CommandPalette
                        | ActiveInputTarget::AuthServerUrl
                        | ActiveInputTarget::AuthAccount
                        | ActiveInputTarget::RenameDeviceInput
                ) {
                txt.trim().to_string()
            } else if matches!(
                target,
                ActiveInputTarget::AuthPassword
                    | ActiveInputTarget::ChangePwCurrent
                    | ActiveInputTarget::ChangePwNew
            ) {
                // #39: Strip trailing newline when pasting from password managers.
                txt.trim_end_matches(&['\r', '\n'][..]).to_string()
            } else {
                txt
            };
            self.commit_text_to_active_target(&payload);
            cx.notify();
        }
    }

    fn adjust_terminal_font_size(&self, delta: Option<f32>) {
        if let Ok(mut g) = self.state.lock() {
            g.layout.terminal_font_size_px = match delta {
                Some(d) => (g.layout.terminal_font_size_px + d).clamp(10.0, 22.0),
                None => 13.0,
            };
        }
        self.persist_layout_config();
    }

    fn commit_text_to_active_target(&self, text: &str) {
        let (target, palette_open, settings_open, focused_id) = self
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
            if let Ok(mut g) = self.state.lock() {
                if g.input_selected_all {
                    g.palette_query.clear();
                    g.input_selected_all = false;
                }
                g.palette_query.push_str(text);
                g.palette_selected = 0;
            }
            return;
        }

        match target {
            ActiveInputTarget::TerminalDirect => {
                if !settings_open {
                    if let Some(id) = focused_id {
                        if let Ok(mut sel) = self.terminal_selection.lock() {
                            *sel = None;
                        }
                        if let Ok(mut g) = self.state.lock() {
                            if let Some(s) = g.sessions.iter_mut().find(|s| s.id == id) {
                                s.scroll_offset = 0;
                            }
                        }
                        self.send_bytes_to_session(&id, text.as_bytes());
                    }
                }
            }
            ActiveInputTarget::AuthServerUrl => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.server_url.clear();
                        g.input_selected_all = false;
                    }
                    g.server_url.push_str(text);
                }
            }
            ActiveInputTarget::AuthAccount => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.auth_account_input.clear();
                        g.input_selected_all = false;
                    }
                    g.auth_account_input.push_str(text);
                }
            }
            ActiveInputTarget::AuthPassword => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.auth_password_input.clear();
                        g.input_selected_all = false;
                    }
                    g.auth_password_input.push_str(text);
                }
            }
            ActiveInputTarget::RenameDeviceInput => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.rename_host_input.clear();
                        g.input_selected_all = false;
                    }
                    g.rename_host_input.push_str(text);
                }
            }
            ActiveInputTarget::ChangePwCurrent => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.pw_current_input.clear();
                        g.input_selected_all = false;
                    }
                    g.pw_current_input.push_str(text);
                }
            }
            ActiveInputTarget::ChangePwNew => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.pw_new_input.clear();
                        g.input_selected_all = false;
                    }
                    g.pw_new_input.push_str(text);
                }
            }
            ActiveInputTarget::CommandPalette => {}
        }
    }

    fn handle_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let mods = &ev.keystroke.modifiers;

        if mods.platform {
            match key {
                "v" => {
                    self.perform_paste_action(cx);
                    cx.stop_propagation();
                }
                "c" => {
                    self.perform_copy_action(mods.shift, cx);
                    cx.stop_propagation();
                }
                "a" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.palette_open || g.input_target != ActiveInputTarget::TerminalDirect {
                            g.input_selected_all = true;
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        if g.palette_open || g.input_target == ActiveInputTarget::CommandPalette {
                            g.palette_query.clear();
                            g.palette_selected = 0;
                        } else {
                            match g.input_target {
                                ActiveInputTarget::AuthServerUrl => g.server_url.clear(),
                                ActiveInputTarget::AuthAccount => g.auth_account_input.clear(),
                                ActiveInputTarget::AuthPassword => g.auth_password_input.clear(),
                                ActiveInputTarget::RenameDeviceInput => g.rename_host_input.clear(),
                                ActiveInputTarget::ChangePwCurrent => g.pw_current_input.clear(),
                                ActiveInputTarget::ChangePwNew => g.pw_new_input.clear(),
                                _ => {}
                            }
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "=" | "+" => {
                    self.adjust_terminal_font_size(Some(1.0));
                    cx.notify();
                    cx.stop_propagation();
                }
                "-" => {
                    self.adjust_terminal_font_size(Some(-1.0));
                    cx.notify();
                    cx.stop_propagation();
                }
                "0" => {
                    self.adjust_terminal_font_size(None);
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }

        let (target, palette_open, settings_open, focused_id) = self
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
                    if let Ok(mut g) = self.state.lock() {
                        g.palette_open = false;
                        g.palette_query.clear();
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "up" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.palette_selected = g.palette_selected.saturating_sub(1);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "down" => {
                    if let Ok(mut g) = self.state.lock() {
                        let len = g.palette_items().len();
                        if g.palette_selected + 1 < len {
                            g.palette_selected += 1;
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    let selected_action = self.state.lock().ok().and_then(|g| {
                        let items = g.palette_items();
                        items
                            .get(g.palette_selected.min(items.len().saturating_sub(1)))
                            .map(|i| i.action.clone())
                    });
                    if let Some(action) = selected_action {
                        self.execute_palette_action(action);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.palette_query.clear();
                            g.input_selected_all = false;
                        } else {
                            g.palette_query.pop();
                        }
                        g.palette_selected = 0;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }

        if settings_open && target == ActiveInputTarget::TerminalDirect {
            if key == "escape" {
                if let Ok(mut g) = self.state.lock() {
                    g.settings_open = false;
                }
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }

        if target == ActiveInputTarget::TerminalDirect {
            if let Some(id) = focused_id {
                if mods.control && !mods.alt && key == "c" {
                    if let Ok(mut g) = self.state.lock() {
                        if let Some(s) = g.sessions.iter_mut().find(|s| s.id == id) {
                            s.scroll_offset = 0;
                        }
                    }
                    self.interrupt_session(&id);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                if mods.control && !mods.alt && key == "l" {
                    if let Ok(mgr) = self.pty_mgr.lock() {
                        mgr.clear_viewport(&id);
                    }
                    self.send_bytes_to_session(&id, &[0x0c]);
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                let app_cursor_keys = self
                    .pty_mgr
                    .lock()
                    .map(|m| m.terminal_modes(&id).2)
                    .unwrap_or(false);
                if let Some(bytes) = Self::keystroke_to_terminal_bytes(
                    key,
                    mods,
                    ev.keystroke.key_char.as_deref(),
                    app_cursor_keys,
                ) {
                    if let Ok(mut sel) = self.terminal_selection.lock() {
                        *sel = None;
                    }
                    if let Ok(mut g) = self.state.lock() {
                        if let Some(s) = g.sessions.iter_mut().find(|s| s.id == id) {
                            s.scroll_offset = 0;
                        }
                    }
                    self.send_bytes_to_session(&id, &bytes);
                    cx.notify();
                    cx.stop_propagation();
                }
            }
            return;
        }

        match target {
            ActiveInputTarget::AuthServerUrl => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.server_url.clear();
                            g.input_selected_all = false;
                        } else {
                            g.server_url.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    let url = self
                        .state
                        .lock()
                        .map(|g| g.server_url.clone())
                        .unwrap_or_default();
                    self.save_server_url_preference(&url);
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::AuthAccount => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.auth_account_input.clear();
                            g.input_selected_all = false;
                        } else {
                            g.auth_account_input.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "tab" | "enter" | "return" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::AuthPassword;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::AuthPassword => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.auth_password_input.clear();
                            g.input_selected_all = false;
                        } else {
                            g.auth_password_input.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.submit_password_login();
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::RenameDeviceInput => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.rename_host_input.clear();
                            g.input_selected_all = false;
                        } else {
                            g.rename_host_input.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.submit_rename_device();
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.renaming_host_id = None;
                        g.rename_host_input.clear();
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::ChangePwCurrent => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.pw_current_input.clear();
                            g.input_selected_all = false;
                        } else {
                            g.pw_current_input.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "tab" | "enter" | "return" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::ChangePwNew;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::ChangePwNew => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.pw_new_input.clear();
                            g.input_selected_all = false;
                        } else {
                            g.pw_new_input.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.submit_change_password();
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::TerminalDirect | ActiveInputTarget::CommandPalette => {}
        }
    }

    fn submit_rename_device(&self) {
        let req = {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            let Some(id) = g.renaming_host_id else {
                return;
            };
            let next_name = g.rename_host_input.trim().to_string();
            if next_name.is_empty() {
                g.status_notice = Some(
                    tr(g.locale, "Host name cannot be empty", "主机名称不能为空").to_string(),
                );
                return;
            }
            g.renaming_host_id = None;
            g.rename_host_input.clear();
            g.input_selected_all = false;
            g.input_target = ActiveInputTarget::TerminalDirect;
            (id, next_name)
        };
        self.mesh_hub.rename_remote_device(req.0, req.1);
    }

    fn submit_change_password(&self) {
        let req = {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            let cur = g.pw_current_input.clone();
            let next = g.pw_new_input.clone();
            if cur.is_empty() || next.len() < 8 {
                g.auth_error = Some(
                    tr(
                        g.locale,
                        "Enter current password and a new password (min 8 chars)",
                        "请输入当前密码与至少 8 位的新密码",
                    )
                    .to_string(),
                );
                return;
            }
            g.auth_error = None;
            g.pw_current_input.clear();
            g.pw_new_input.clear();
            g.input_selected_all = false;
            g.input_target = ActiveInputTarget::TerminalDirect;
            (cur, next)
        };
        self.mesh_hub.change_password(req.0, req.1);
    }

    fn submit_password_login(&self) {
        let (srv, acc, pw) = {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            let s = g.server_url.trim().to_string();
            let a = g.auth_account_input.trim().to_string();
            let p = g.auth_password_input.clone();
            if !is_safe_http_url(&s) {
                g.auth_error = Some(
                    tr(
                        g.locale,
                        "Invalid server URL (must start with http:// or https://)",
                        "无效的服务端地址（必须以 http:// 或 https:// 开头）",
                    )
                    .to_string(),
                );
                return;
            }
            if a.is_empty() || p.is_empty() {
                g.auth_error = Some(
                    tr(g.locale, "Please enter account and password", "请输入账号和密码")
                        .to_string(),
                );
                return;
            }
            g.auth_loading = true;
            g.auth_error = None;
            g.input_selected_all = false;
            (s, a, p)
        };
        spawn_password_login(
            Arc::clone(&self.state),
            Arc::clone(&self.ui_dirty),
            Arc::clone(&self.mesh_hub),
            srv,
            acc,
            pw,
        );
    }

    fn persist_layout_config(&self) {
        let Ok(layout) = self.state.lock().map(|g| g.layout.clone()) else {
            return;
        };
        let state_bg = Arc::clone(&self.state);
        std::thread::spawn(move || {
            let _ = layout.save();
            let (_, mtime) = MalleableLayoutConfig::load_with_mtime();
            if let Ok(mut g) = state_bg.lock() {
                g.layout_mtime = mtime;
            }
        });
    }

    fn sync_pty_snapshots_into_state(&self) {
        let snaps = match self.pty_mgr.lock() {
            Ok(mgr) => mgr.snapshot_all(1200),
            Err(_) => return,
        };
        let Ok(mut synced) = self.pty_synced_gen.lock() else {
            return;
        };
        synced.retain(|id, _| snaps.contains_key(id));
        let Ok(mut g) = self.state.lock() else {
            return;
        };
        let mut osc_updates: Vec<(String, Osc7501Status)> = Vec::new();
        for sess in &mut g.sessions {
            if let Some(snap) = snaps.get(&sess.id) {
                let prev = synced.get(&sess.id).copied();
                if prev == Some((snap.generation, snap.alive)) {
                    continue;
                }
                synced.insert(sess.id.clone(), (snap.generation, snap.alive));
                let old_total = sess.styled_lines.len();
                let new_total = snap.styled_lines.len();
                // #41: Keep scrollback viewport pinned while user is reading history instead of jumping.
                if sess.scroll_offset > 0 && !snap.in_alt_screen && new_total > old_total {
                    sess.scroll_offset = (sess.scroll_offset + (new_total - old_total)).min(1200);
                } else if snap.in_alt_screen {
                    sess.scroll_offset = 0;
                }
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
                    osc_updates.push((sess.id.clone(), osc.clone()));
                }
            }
        }
        for (sid, osc) in osc_updates {
            g.record_session_osc_status(&sid, osc);
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
        Self::refresh_inspector_arcs(
            &self.state,
            &self.mesh_hub,
            &self.inspector_seq,
            &self.ui_dirty,
        );
    }

    fn refresh_inspector_arcs(
        state: &Arc<Mutex<CodaDesktopState>>,
        mesh_hub: &Arc<RemoteMeshHub>,
        inspector_seq: &Arc<AtomicU64>,
        ui_dirty: &Arc<AtomicBool>,
    ) {
        let (space_id, space_path_str, remote_host_id, browse_rel, selected_change, selected_file_rel) = {
            let Ok(g) = state.lock() else {
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
                g.selected_file_rel.clone(),
            )
        };

        if let Some(host_id) = remote_host_id {
            mesh_hub.refresh_remote_git_diff(host_id, &space_id, &space_path_str);
            if space_path_str != "." {
                mesh_hub.refresh_remote_directory(host_id, &space_id, &space_path_str, &browse_rel);
            }
            return;
        }

        let seq = inspector_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let seq_ref = Arc::clone(inspector_seq);
        let state_bg = Arc::clone(state);
        let ui_dirty = Arc::clone(ui_dirty);
        std::thread::spawn(move || {
            let space_path = PathBuf::from(&space_path_str);
            let browse_dir = if browse_rel.trim().is_empty() {
                space_path.clone()
            } else if let Some(contained) = resolve_contained_path(&space_path, &browse_rel) {
                if contained.is_dir() {
                    contained
                } else {
                    space_path.clone()
                }
            } else {
                space_path.clone()
            };

            let snap = compute_repo_snapshot(&space_path, selected_change.as_deref());
            let entries = list_workspace_dir(&space_path, &browse_dir);
            let file_preview = selected_file_rel.as_deref().and_then(|rel| {
                let full = resolve_contained_path(&space_path, rel)?;
                full.is_file()
                    .then(|| read_workspace_file_preview(&full, 120_000))
            });

            if seq_ref.load(Ordering::Acquire) != seq {
                return;
            }
            if let Ok(mut g) = state_bg.lock() {
                if let Some(sp) = g.spaces.iter_mut().find(|s| s.id == space_id) {
                    sp.branch = snap.branch.clone();
                    sp.dirty_files = snap.changed_files.len();
                    sp.ahead = 0;
                    sp.behind = 0;
                }
                if g.active_space_id.as_deref() == Some(space_id.as_str()) {
                    g.repo_snapshot = snap;
                    g.dir_entries = entries;
                    if let Some(preview) = file_preview {
                        g.selected_file_content = preview;
                    }
                }
            }
            ui_dirty.store(true, Ordering::Release);
        });
    }

    fn spawn_terminal_in_active_space(&self, command: Option<&str>) {
        self.ensure_active_space();
        Self::spawn_terminal_in_active_space_arcs(
            &self.state,
            &self.pty_mgr,
            &self.mesh_hub,
            &self.ui_dirty,
            command,
        );
    }

    fn spawn_terminal_in_active_space_arcs(
        state: &Arc<Mutex<CodaDesktopState>>,
        pty_mgr: &Arc<Mutex<LocalPtyManager>>,
        mesh_hub: &Arc<RemoteMeshHub>,
        ui_dirty: &Arc<AtomicBool>,
        command: Option<&str>,
    ) {
        let (space_id, space_name, cwd_str, is_remote, machine_id, hc_path, loc) = {
            let Ok(g) = state.lock() else {
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
                mesh_hub.spawn_remote_terminal_or_agent(host_id, agent, &cwd_str, &space_id);
            } else if let Ok(mut g) = state.lock() {
                g.status_notice = Some(
                    tr(
                        loc,
                        "Remote host ID invalid; open Mesh to reconnect.",
                        "远端主机 ID 无效，请打开节点面板重新连接。",
                    )
                    .to_string(),
                );
                ui_dirty.store(true, Ordering::Release);
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
            if let Ok(mut g) = state.lock() {
                g.status_notice = Some(
                    tr(loc, "Starting terminal...", "正在启动终端...").to_string(),
                );
            }
            ui_dirty.store(true, Ordering::Release);
            let state_bg = Arc::clone(state);
            let pty_mgr_bg = Arc::clone(pty_mgr);
            let ui_dirty = Arc::clone(ui_dirty);
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

        let spawned = match pty_mgr.lock() {
            Ok(mut mgr) => mgr.spawn_session(&id, &cwd, command, 120, 36),
            Err(e) => Err(e.to_string()),
        };

        if let Err(err) = spawned {
            if let Ok(mut g) = state.lock() {
                g.status_notice = Some(err);
            }
            ui_dirty.store(true, Ordering::Release);
            return;
        }

        if let Ok(mut g) = state.lock() {
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
        ui_dirty.store(true, Ordering::Release);
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
        Self::select_space_arcs(
            &self.state,
            &self.pty_mgr,
            &self.mesh_hub,
            &self.inspector_seq,
            &self.ui_dirty,
            space_id,
        );
    }

    fn select_space_arcs(
        state: &Arc<Mutex<CodaDesktopState>>,
        pty_mgr: &Arc<Mutex<LocalPtyManager>>,
        mesh_hub: &Arc<RemoteMeshHub>,
        inspector_seq: &Arc<AtomicU64>,
        ui_dirty: &Arc<AtomicBool>,
        space_id: &str,
    ) {
        let mut need_spawn = false;
        if let Ok(mut g) = state.lock() {
            g.active_space_id = Some(space_id.to_string());
            g.current_browse_rel.clear();
            g.selected_file_rel = None;
            g.selected_file_content.clear();
            g.selected_change_path = None;
            let sp_owned = space_id.to_string();
            Self::update_prefs_async(move |prefs| {
                prefs.selected_repo_path = Some(sp_owned);
            });

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
        Self::refresh_inspector_arcs(state, mesh_hub, inspector_seq, ui_dirty);
        if need_spawn {
            Self::spawn_terminal_in_active_space_arcs(state, pty_mgr, mesh_hub, ui_dirty, None);
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
            let sp_owned = space_id.to_string();
            let rp_owned = removed_path.clone();
            Self::update_prefs_async(move |prefs| {
                if !prefs.dismissed_spaces.iter().any(|x| x == &sp_owned) {
                    prefs.dismissed_spaces.push(sp_owned.clone());
                }
                if let Some(ref rp) = rp_owned {
                    prefs.workspace_roots.retain(|r| r != rp);
                }
                if prefs.selected_repo_path.as_deref() == Some(sp_owned.as_str()) {
                    prefs.selected_repo_path = None;
                }
            });

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

        let seq = self.inspector_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let seq_ref = Arc::clone(&self.inspector_seq);
        let state_bg = Arc::clone(&self.state);
        let ui_dirty = Arc::clone(&self.ui_dirty);
        std::thread::spawn(move || {
            let base_path = PathBuf::from(&space_path_str);
            let target = if clean_rel.is_empty() {
                base_path.clone()
            } else if let Some(contained) = resolve_contained_path(&base_path, &clean_rel) {
                contained
            } else {
                if let Ok(mut g) = state_bg.lock() {
                    g.status_notice = Some("Blocked path outside active workspace".to_string());
                }
                ui_dirty.store(true, Ordering::Release);
                return;
            };
            if is_dir || target.is_dir() {
                let entries = list_workspace_dir(&base_path, &target);
                if seq_ref.load(Ordering::Acquire) != seq {
                    return;
                }
                if let Ok(mut g) = state_bg.lock() {
                    if g.active_space_id.as_deref() == Some(space_id.as_str()) {
                        g.current_browse_rel = clean_rel;
                        g.dir_entries = entries;
                        g.selected_file_rel = None;
                        g.selected_file_content.clear();
                    }
                }
            } else if target.is_file() {
                let content = read_workspace_file_preview(&target, 120_000);
                if seq_ref.load(Ordering::Acquire) != seq {
                    return;
                }
                if let Ok(mut g) = state_bg.lock() {
                    if g.active_space_id.as_deref() == Some(space_id.as_str()) {
                        g.selected_file_rel = Some(clean_rel);
                        g.selected_file_content = content;
                        g.right_drawer = Some(RightDrawerTab::Files);
                    }
                }
            }
            ui_dirty.store(true, Ordering::Release);
        });
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
        let state_bg = Arc::clone(&self.state);
        let pty_mgr_bg = Arc::clone(&self.pty_mgr);
        let mesh_hub_bg = Arc::clone(&self.mesh_hub);
        let seq_bg = Arc::clone(&self.inspector_seq);
        let ui_dirty = Arc::clone(&self.ui_dirty);
        std::thread::spawn(move || {
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

            if let Ok(mut g) = state_bg.lock() {
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
            }
            let space_id_pref = space_id.clone();
            let path_pref = path_str.clone();
            Self::update_prefs_async(move |prefs| {
                prefs
                    .dismissed_spaces
                    .retain(|x| x != &space_id_pref && x != &path_pref);
                if !prefs.workspace_roots.contains(&path_pref) {
                    prefs.workspace_roots.push(path_pref);
                }
            });
            Self::select_space_arcs(
                &state_bg,
                &pty_mgr_bg,
                &mesh_hub_bg,
                &seq_bg,
                &ui_dirty,
                &space_id,
            );
            ui_dirty.store(true, Ordering::Release);
        });
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
        }
        Self::update_prefs_async(move |prefs| {
            prefs.locale = locale;
        });
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
        let trimmed = url.trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            return;
        }
        if !is_safe_http_url(&trimmed) {
            if let Ok(mut g) = self.state.lock() {
                let msg = tr(
                    g.locale,
                    "Invalid server URL (must start with http:// or https://)",
                    "无效的服务端地址（必须以 http:// 或 https:// 开头）",
                )
                .to_string();
                g.auth_error = Some(msg.clone());
                g.status_notice = Some(msg);
            }
            self.ui_dirty.store(true, Ordering::Release);
            return;
        }
        // #41: Switching to a different server URL while logged in cleanly signs out of the old server.
        let switching_while_logged_in = self
            .state
            .lock()
            .map(|g| {
                g.is_logged_in
                    && g.server_url.trim().trim_end_matches('/') != trimmed.as_str()
            })
            .unwrap_or(false);
        if switching_while_logged_in {
            self.mesh_hub.sign_out();
        }
        if let Ok(mut g) = self.state.lock() {
            g.server_url = trimmed.clone();
            g.auth_error = None;
            g.input_selected_all = false;
            g.input_target = ActiveInputTarget::TerminalDirect;
        }
        Self::update_prefs_async(move |prefs| {
            prefs.server_url = trimmed;
        });
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn restore_dismissed_spaces(&self) {
        if let Ok(mut g) = self.state.lock() {
            g.dismissed_spaces.clear();
        }
        let state_bg = Arc::clone(&self.state);
        let mesh_hub_bg = Arc::clone(&self.mesh_hub);
        let seq_bg = Arc::clone(&self.inspector_seq);
        let ui_dirty = Arc::clone(&self.ui_dirty);
        std::thread::spawn(move || {
            let mut prefs = DesktopPreferences::load_or_default();
            prefs.dismissed_spaces.clear();
            let _ = prefs.save();

            let candidates = discover_local_git_repositories(&prefs.workspace_roots);
            let mut parsed = Vec::new();
            for path_str in candidates {
                let p = PathBuf::from(&path_str);
                let snap = compute_repo_snapshot(&p, None);
                let name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("workspace")
                    .to_string();
                parsed.push((path_str, name, snap));
            }
            if let Ok(mut g) = state_bg.lock() {
                let machine_label = g.local_machine_name.clone();
                for (path_str, name, snap) in parsed {
                    let space_id = format!("local:{path_str}");
                    if g.spaces.iter().any(|s| s.id == space_id) {
                        continue;
                    }
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
            Self::refresh_inspector_arcs(&state_bg, &mesh_hub_bg, &seq_bg, &ui_dirty);
            ui_dirty.store(true, Ordering::Release);
        });
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

        let focused_pane_has_sess = if st.layout.split_terminals && st.focused_pane == SplitPane::Secondary {
            st.split_session_id
                .as_deref()
                .and_then(|id| st.session_by_id(id))
                .is_some()
        } else {
            st.active_session_id
                .as_deref()
                .and_then(|id| st.session_by_id(id))
                .is_some()
        };
        let terminal_ime_active = !st.palette_open
            && !st.settings_open
            && st.input_target == ActiveInputTarget::TerminalDirect
            && st.layout.layout == LayoutMode::Workbench
            && focused_pane_has_sess;

        let main_area = match st.layout.layout {
            LayoutMode::Workbench => self.render_workbench_panes(&st, cx).into_any_element(),
            LayoutMode::Terminals => self.render_terminals_grid(&st, cx).into_any_element(),
        };

        let fallback_focus = self.focus_handle.clone();
        let fallback_entity = cx.entity();
        let fallback_ime_bounds = Arc::clone(&self.ime_bounds);

        div()
            .id("coda-root")
            .key_context("CodaWorkbench")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
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
            .on_action(cx.listener(|this, _: &CopyTerminalAction, _, cx| {
                this.perform_copy_action(false, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PasteTerminalAction, _, cx| {
                this.perform_paste_action(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectAllAction, _, cx| {
                if let Ok(mut g) = this.state.lock() {
                    if g.input_target != ActiveInputTarget::TerminalDirect {
                        g.input_selected_all = true;
                    } else if let Some(sid) = g.focused_session_id().map(str::to_string) {
                        if let Some(sess) = g.session_by_id(&sid) {
                            let last_line = sess.styled_lines.len().saturating_sub(1);
                            let last_col = sess
                                .styled_lines
                                .get(last_line)
                                .map(|l| l.text.chars().count())
                                .unwrap_or(0);
                            if let Ok(mut sel) = this.terminal_selection.lock() {
                                *sel = Some(TerminalSelection {
                                    session_id: sid,
                                    anchor: (0, 0),
                                    focus: (last_line, last_col),
                                    dragging: false,
                                });
                            }
                        }
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomInTerminal, _, cx| {
                this.adjust_terminal_font_size(Some(1.0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomOutTerminal, _, cx| {
                this.adjust_terminal_font_size(Some(-1.0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomResetTerminal, _, cx| {
                this.adjust_terminal_font_size(None);
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
            .when(!terminal_ime_active, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .w(px(1.0))
                        .h(px(1.0))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, (), window, cx| {
                                    let ime_box = Bounds {
                                        origin: point(bounds.origin.x + px(40.0), bounds.origin.y + px(80.0)),
                                        size: size(px(240.0), px(24.0)),
                                    };
                                    if let Ok(mut g) = fallback_ime_bounds.lock() {
                                        *g = Some(ime_box);
                                    }
                                    window.handle_input(
                                        &fallback_focus,
                                        ElementInputHandler::new(ime_box, fallback_entity.clone()),
                                        cx,
                                    );
                                },
                            )
                            .size_full(),
                        ),
                )
            })
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
                let opened = if let Ok(mut g) = this.state.lock() {
                    if g.right_drawer == Some(tab) {
                        g.right_drawer = None;
                        false
                    } else {
                        g.right_drawer = Some(tab);
                        true
                    }
                } else {
                    false
                };
                if opened && matches!(tab, RightDrawerTab::Changes | RightDrawerTab::Files) {
                    this.refresh_active_space_inspector();
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
        let sid_sel_down = sess.id.clone();
        let sid_sel_move = sess.id.clone();
        let sid_scroll = sess.id.clone();
        let sid_jump_bottom = sess.id.clone();
        let sid_resize = sess.id.clone();
        let remote_host_uuid = sess
            .remote_host_id
            .as_deref()
            .and_then(|s| Uuid::parse_str(s).ok());
        let hc_addr_clone = sess.hc_addr.clone();
        let pane_ime_active = is_focused
            && !st.palette_open
            && !st.settings_open
            && st.input_target == ActiveInputTarget::TerminalDirect
            && st.layout.layout == LayoutMode::Workbench;
        let ime_marked = self.marked_text.clone();
        let viewport_dims_bg = Arc::clone(&self.viewport_dims);
        let visible_rows_bg = Arc::clone(&self.visible_rows_by_pane);
        let pane_geometry_bg = Arc::clone(&self.pane_geometry);
        let ime_bounds_bg = Arc::clone(&self.ime_bounds);
        let pty_mgr_bg = Arc::clone(&self.pty_mgr);
        let mesh_hub_bg = Arc::clone(&self.mesh_hub);
        let ui_dirty_bg = Arc::clone(&self.ui_dirty);
        let focus_handle_bg = self.focus_handle.clone();
        let view_entity_bg = cx.entity();

        let measured_rows = self
            .visible_rows_by_pane
            .lock()
            .ok()
            .and_then(|m| m.get(&pane_slot).copied())
            .unwrap_or(visible_rows);

        let branch_label = st
            .spaces
            .iter()
            .find(|s| s.id == sess.space_id)
            .map(|s| s.branch.as_str())
            .unwrap_or("main")
            .to_string();
        let osc_badge = sess.osc_status.as_ref().and_then(|o| o.task.clone());
        let needs_you_active = sess
            .osc_status
            .as_ref()
            .map(|o| o.state == AgentLifecycleState::NeedsYou)
            .unwrap_or(false);
        let needs_you_msg = sess
            .osc_status
            .as_ref()
            .and_then(|o| o.task.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                tr(
                    loc,
                    "Agent is waiting for your approval",
                    "AI Agent 正在等待你的交互确认",
                )
                .to_string()
            });
        let sid_approve_yes = sess.id.clone();
        let sid_approve_always = sess.id.clone();
        let sid_approve_deny = sess.id.clone();

        let total_lines = sess.styled_lines.len();
        let max_off = total_lines.saturating_sub(measured_rows);
        let eff_off = sess.scroll_offset.min(max_off);
        let end = total_lines.saturating_sub(eff_off);
        let start = end.saturating_sub(measured_rows);
        let visible_slice = &sess.styled_lines[start..end];
        let active_sel = self
            .terminal_selection
            .lock()
            .ok()
            .and_then(|s| s.clone())
            .filter(|sel| sel.session_id == sess.id && sel.is_non_empty());

        div()
            .id(SharedString::from(format!("term-pane-{pane_slot:?}-{}", sess.id)))
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _, cx| {
                let (in_alt, app_cursor, _) = this
                    .pty_mgr
                    .lock()
                    .map(|mgr| mgr.terminal_modes(&sid_scroll))
                    .unwrap_or((false, false, false));

                let step: isize = match ev.delta {
                    ScrollDelta::Lines(pt) => {
                        if pt.y.abs() > 0.05 {
                            (pt.y * 3.0).round() as isize
                        } else {
                            0
                        }
                    }
                    ScrollDelta::Pixels(pt) => {
                        let dy = f32::from(pt.y);
                        let row_px = (line_h + 1.5).max(12.0);
                        if let Ok(mut acc_map) = this.scroll_pixel_acc.lock() {
                            let acc = acc_map.entry(sid_scroll.clone()).or_insert(0.0);
                            *acc += dy;
                            let lines = (*acc / row_px).trunc() as isize;
                            if lines != 0 {
                                *acc -= (lines as f32) * row_px;
                            }
                            lines
                        } else {
                            0
                        }
                    }
                };
                if step == 0 {
                    return;
                }
                if in_alt {
                    // Forward wheel scroll as arrow keys inside TUI / alt-screen apps (vim, less, htop).
                    let key_seq: &[u8] = if step > 0 {
                        if app_cursor { b"\x1bOA" } else { b"\x1b[A" }
                    } else if app_cursor {
                        b"\x1bOB"
                    } else {
                        b"\x1b[B"
                    };
                    let repeats = step.unsigned_abs().min(5);
                    let mut payload = Vec::with_capacity(key_seq.len() * repeats);
                    for _ in 0..repeats {
                        payload.extend_from_slice(key_seq);
                    }
                    this.send_bytes_to_session(&sid_scroll, &payload);
                    return;
                }
                if let Ok(mut g) = this.state.lock() {
                    if let Some(s) = g.sessions.iter_mut().find(|s| s.id == sid_scroll) {
                        let max_scroll = s.styled_lines.len().saturating_sub(measured_rows).min(1200);
                        if step > 0 {
                            s.scroll_offset = (s.scroll_offset + step as usize).min(max_scroll);
                        } else {
                            s.scroll_offset = s.scroll_offset.saturating_sub((-step) as usize);
                        }
                    }
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    if let Ok(mut g) = this.state.lock() {
                        g.focused_pane = pane_slot;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                        if pane_slot == SplitPane::Secondary {
                            g.split_session_id = Some(sid_focus.clone());
                        } else {
                            g.active_session_id = Some(sid_focus.clone());
                        }
                    }
                    if let Ok(geo_map) = this.pane_geometry.lock() {
                        if let Some(geo) = geo_map.get(&pane_slot).copied() {
                            let rel_x = (f32::from(ev.position.x - geo.bounds.origin.x) - 14.0).max(0.0);
                            let rel_y = (f32::from(ev.position.y - geo.bounds.origin.y) - 10.0).max(0.0);
                            let vis_row = (rel_y / geo.row_step).floor() as usize;
                            let col = (rel_x / geo.cell_w).floor() as usize;
                            let abs_line = (geo.start_line + vis_row).min(geo.total_lines.saturating_sub(1));
                            if let Ok(mut sel) = this.terminal_selection.lock() {
                                *sel = Some(TerminalSelection {
                                    session_id: sid_sel_down.clone(),
                                    anchor: (abs_line, col),
                                    focus: (abs_line, col),
                                    dragging: true,
                                });
                            }
                        }
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                let is_dragging = this
                    .terminal_selection
                    .lock()
                    .ok()
                    .and_then(|s| s.as_ref().map(|x| x.dragging && x.session_id == sid_sel_move))
                    .unwrap_or(false);
                if !is_dragging {
                    return;
                }
                if !ev.dragging() {
                    if let Ok(mut sel) = this.terminal_selection.lock() {
                        if let Some(ref mut cur) = *sel {
                            cur.dragging = false;
                        }
                    }
                    return;
                }
                if let Ok(geo_map) = this.pane_geometry.lock() {
                    if let Some(geo) = geo_map.get(&pane_slot).copied() {
                        let rel_x = (f32::from(ev.position.x - geo.bounds.origin.x) - 14.0).max(0.0);
                        let rel_y = (f32::from(ev.position.y - geo.bounds.origin.y) - 10.0).max(0.0);
                        let vis_row = (rel_y / geo.row_step).floor() as usize;
                        let col = (rel_x / geo.cell_w).floor() as usize;
                        let abs_line = (geo.start_line + vis_row).min(geo.total_lines.saturating_sub(1));
                        if let Ok(mut sel) = this.terminal_selection.lock() {
                            if let Some(ref mut cur) = *sel {
                                cur.focus = (abs_line, col);
                            }
                        }
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    if let Ok(mut sel) = this.terminal_selection.lock() {
                        if let Some(ref mut cur) = *sel {
                            cur.dragging = false;
                            if !cur.is_non_empty() {
                                *sel = None;
                            }
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
                    .relative()
                    .font_family(font_family)
                    .text_size(font_size)
                    .flex()
                    .flex_col()
                    .gap(px(1.5))
                    .children(visible_slice.iter().enumerate().map(|(vis_idx, line)| {
                        let abs_idx = start + vis_idx;
                        let sel_cols = active_sel.as_ref().and_then(|sel| {
                            let ((r0, c0), (r1, c1)) = sel.ordered();
                            if abs_idx < r0 || abs_idx > r1 {
                                return None;
                            }
                            let line_len = line.text.chars().count();
                            let s_col = if abs_idx == r0 { c0 } else { 0 };
                            let e_col = if abs_idx == r1 { c1 } else { line_len };
                            (s_col < e_col).then_some((s_col, e_col))
                        });
                        render_styled_terminal_line(line, line_h, sel_cols)
                    }))
                    .child(
                        div().absolute().inset_0().child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, (), window, cx| {
                                    let inner_w = (f32::from(bounds.size.width) - 28.0).max(120.0);
                                    let inner_h = (f32::from(bounds.size.height) - 20.0).max(60.0);
                                    let cell_w = (font_size_px * 0.605).max(6.0);
                                    let row_step = (line_h + 1.5).max(10.0);
                                    let cols = (inner_w / cell_w).floor().clamp(24.0, 400.0) as u16;
                                    let rows = (inner_h / row_step).floor().clamp(6.0, 200.0) as u16;

                                    if let Ok(mut geos) = pane_geometry_bg.lock() {
                                        geos.insert(
                                            pane_slot,
                                            PaneGeometry {
                                                bounds,
                                                start_line: start,
                                                total_lines,
                                                cell_w,
                                                row_step,
                                            },
                                        );
                                    }

                                    if let Ok(mut map) = visible_rows_bg.lock() {
                                        let r = rows as usize;
                                        if map.get(&pane_slot).copied() != Some(r) {
                                            map.insert(pane_slot, r);
                                            ui_dirty_bg.store(true, Ordering::Release);
                                        }
                                    }

                                    let mut dim_changed = false;
                                    if let Ok(mut dims) = viewport_dims_bg.lock() {
                                        if dims.get(&sid_resize).copied() != Some((cols, rows)) {
                                            dims.insert(sid_resize.clone(), (cols, rows));
                                            dim_changed = true;
                                        }
                                    }
                                    if dim_changed {
                                        if let Ok(mut mgr) = pty_mgr_bg.lock() {
                                            mgr.resize(&sid_resize, cols, rows);
                                        }
                                        if let (Some(hid), Some(ref term_addr)) =
                                            (remote_host_uuid, hc_addr_clone.as_ref())
                                        {
                                            mesh_hub_bg.resize_remote_pty(hid, term_addr, cols, rows);
                                        }
                                    }

                                    if pane_ime_active {
                                        let cursor_y = (bounds.origin.y + bounds.size.height
                                            - px(line_h + 14.0))
                                        .max(bounds.origin.y + px(10.0));
                                        let cursor_bounds = Bounds {
                                            origin: point(bounds.origin.x + px(14.0), cursor_y),
                                            size: size(px(220.0), px(line_h)),
                                        };
                                        if let Ok(mut ib) = ime_bounds_bg.lock() {
                                            *ib = Some(cursor_bounds);
                                        }
                                        window.handle_input(
                                            &focus_handle_bg,
                                            ElementInputHandler::new(
                                                cursor_bounds,
                                                view_entity_bg.clone(),
                                            ),
                                            cx,
                                        );
                                    }
                                },
                            )
                            .size_full(),
                        ),
                    ),
            )
            .when(needs_you_active, |el| {
                el.child(
                    // OSC 7501 Needs-You one-tap approval bar (matches Web / Apple / Prototype)
                    div()
                        .flex_shrink_0()
                        .px(px(12.0))
                        .py(px(6.0))
                        .bg(rgba(0xf59e0b1c))
                        .border_t_1()
                        .border_color(rgba(0xf59e0b66))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .w(px(7.0))
                                        .h(px(7.0))
                                        .rounded(px(99.0))
                                        .bg(rgb(palette::NEEDS_YOU_AMBER)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .truncate()
                                        .child(needs_you_msg.clone()),
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
                                        .id(SharedString::from(format!(
                                            "approve-yes-{pane_slot:?}"
                                        )))
                                        .occlude()
                                        .px(px(9.0))
                                        .py(px(3.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(palette::NEEDS_YOU_AMBER))
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0x09090b))
                                        .cursor_pointer()
                                        .child(tr(loc, "1 · Yes", "1 · 允许一次"))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.send_bytes_to_session(&sid_approve_yes, b"1\r");
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "approve-always-{pane_slot:?}"
                                        )))
                                        .occlude()
                                        .px(px(9.0))
                                        .py(px(3.0))
                                        .rounded(px(5.0))
                                        .bg(rgba(0xf59e0b2e))
                                        .border_1()
                                        .border_color(rgba(0xf59e0b66))
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .cursor_pointer()
                                        .child(tr(loc, "2 · Always", "2 · 始终允许"))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.send_bytes_to_session(&sid_approve_always, b"2\r");
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "approve-deny-{pane_slot:?}"
                                        )))
                                        .occlude()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(5.0))
                                        .border_1()
                                        .border_color(rgb(palette::BORDER_STRONG))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .text_size(px(11.0))
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .cursor_pointer()
                                        .child(tr(loc, "Esc · Deny", "Esc · 拒绝"))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.send_bytes_to_session(&sid_approve_deny, b"\x1b");
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
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
                                        .child(format!(
                                            "↓ {} (-{eff_off})",
                                            tr(loc, "Latest", "回到最新")
                                        ))
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
                            .when(is_focused && !ime_marked.is_empty(), |el| {
                                el.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .text_size(px(10.5))
                                        .text_color(rgb(accent))
                                        .child(format!("[IME: {ime_marked}]")),
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
                                            mgr.clear_viewport(&sid_clear);
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
                                    .map(|line| render_styled_terminal_line(line, tile_line_h, None))
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
                            .when(tab != RightDrawerTab::Mesh, |el| {
                                el.child(
                                    div()
                                        .id("btn-drawer-refresh")
                                        .occlude()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child("↻")
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.refresh_active_space_inspector();
                                            cx.notify();
                                        })),
                                )
                            })
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
                        if !snap.is_git_repo {
                            el.child(
                                div()
                                    .p(px(10.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .text_size(px(11.5))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .child(tr(
                                        loc,
                                        "Not a Git repository",
                                        "当前工作区不是 Git 仓库",
                                    )),
                            )
                        } else if snap.changed_files.is_empty() {
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
        let accent = st.layout.accent_color_u32();
        let show_auth_form = !st.is_logged_in || st.auth_form_expanded;
        let host_mode_on = st.allow_remote_control;
        let host_status_hint = if st.external_daemon_running {
            tr(
                loc,
                "Standalone coda-daemon running",
                "独立 coda-daemon 后台服务运行中",
            )
        } else if st.embedded_host_active {
            tr(
                loc,
                "Remote control active (E2EE)",
                "已开启远端被控 (E2EE 在线)",
            )
        } else if host_mode_on {
            tr(
                loc,
                "Sign in to accept remote control",
                "登录后即允许其他设备控制本机",
            )
        } else {
            tr(
                loc,
                "Controller-only (this Mac is hidden)",
                "仅控制端模式（本机不对外被控）",
            )
        };
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
        .child(
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
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(palette::TEXT_PRIMARY))
                                .child(tr(loc, "Allow Remote Control", "允许远端控制本机")),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(palette::TEXT_DIM))
                                .truncate()
                                .child(host_status_hint),
                        ),
                )
                .child(
                    div()
                        .id("drawer-toggle-remote-host")
                        .occlude()
                        .flex_shrink_0()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(rgb(if host_mode_on {
                            palette::HOVER_BG
                        } else {
                            palette::CHROME_BG
                        }))
                        .border_1()
                        .border_color(rgb(if host_mode_on {
                            accent
                        } else {
                            palette::BORDER
                        }))
                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                        .cursor_pointer()
                        .text_size(px(10.5))
                        .text_color(rgb(if host_mode_on {
                            palette::DONE_GREEN
                        } else {
                            palette::TEXT_MUTED
                        }))
                        .child(if host_mode_on {
                            tr(loc, "Enabled", "已开启")
                        } else {
                            tr(loc, "Off", "已关闭")
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.mesh_hub.set_allow_remote_control(!host_mode_on);
                            cx.notify();
                        })),
                ),
        )
        .when(show_auth_form, |el| {
            let srv_focused = st.input_target == ActiveInputTarget::AuthServerUrl;
            let acct_focused = st.input_target == ActiveInputTarget::AuthAccount;
            let pass_focused = st.input_target == ActiveInputTarget::AuthPassword;
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
                            .border_color(rgb(if srv_focused {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child(if srv_focused {
                                format!("{}|", st.server_url)
                            } else {
                                st.server_url.clone()
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthServerUrl;
                                    g.input_selected_all = false;
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
                                    .map(|mut g| {
                                        g.auth_loading = true;
                                        g.auth_error = None;
                                        g.server_url.clone()
                                    })
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
                            .border_color(rgb(if acct_focused {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if st.auth_account_input.is_empty() && !acct_focused {
                                palette::TEXT_DIM
                            } else {
                                palette::TEXT_PRIMARY
                            }))
                            .child(if st.auth_account_input.is_empty() {
                                if acct_focused {
                                    "|".to_string()
                                } else {
                                    tr(loc, "Email / Username", "账号邮箱").to_string()
                                }
                            } else if acct_focused {
                                format!("{}|", st.auth_account_input)
                            } else {
                                st.auth_account_input.clone()
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthAccount;
                                    g.input_selected_all = false;
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
                            .border_color(rgb(if pass_focused {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(rgb(if st.auth_password_input.is_empty() && !pass_focused {
                                palette::TEXT_DIM
                            } else {
                                palette::TEXT_PRIMARY
                            }))
                            .child(if st.auth_password_input.is_empty() {
                                if pass_focused {
                                    "|".to_string()
                                } else {
                                    tr(loc, "Password", "密码").to_string()
                                }
                            } else {
                                let dots = "*".repeat(st.auth_password_input.chars().count());
                                if pass_focused {
                                    format!("{dots}|")
                                } else {
                                    dots
                                }
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthPassword;
                                    g.input_selected_all = false;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("btn-drawer-password-signin")
                            .occlude()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(5.0))
                            .bg(rgb(accent))
                            .hover(|h| h.opacity(0.9))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(palette::CANVAS_BG))
                            .child(if st.auth_loading {
                                tr(loc, "Signing in…", "正在登录…")
                            } else {
                                tr(loc, "Sign In", "登录")
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.submit_password_login();
                                cx.notify();
                            })),
                    )
                    .when_some(st.auth_error.clone(), |el, err| {
                        el.child(
                            div()
                                .px(px(8.0))
                                .py(px(5.0))
                                .rounded(px(4.0))
                                .bg(rgb(palette::CANVAS_BG))
                                .border_1()
                                .border_color(rgb(palette::ERROR_RED))
                                .text_size(px(10.5))
                                .text_color(rgb(palette::ERROR_RED))
                                .child(err),
                        )
                    })
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
                                        format!("{}|", st.palette_query)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("palette-list")
                            .max_h(px(360.0))
                            .p(px(6.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(items.into_iter().enumerate().map(|(idx, item)| {
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
        let host_mode_on = st.allow_remote_control;
        let srv_focused = st.input_target == ActiveInputTarget::AuthServerUrl;
        let acct_focused = st.input_target == ActiveInputTarget::AuthAccount;
        let pass_focused = st.input_target == ActiveInputTarget::AuthPassword;

        div()
            .id("settings-machines-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
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
                            .border_color(rgb(if srv_focused {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(rgb(palette::TEXT_PRIMARY))
                            .child(if srv_focused {
                                format!("{}|", st.server_url)
                            } else {
                                st.server_url.clone()
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Ok(mut g) = this.state.lock() {
                                    g.input_target = ActiveInputTarget::AuthServerUrl;
                                    g.input_selected_all = false;
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
                                            "settings-toggle-pw-change",
                                            tr(loc, "Password", "改密"),
                                            st.pw_change_expanded,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.pw_change_expanded = !g.pw_change_expanded;
                                                    g.auth_error = None;
                                                    if g.pw_change_expanded {
                                                        g.input_target = ActiveInputTarget::ChangePwCurrent;
                                                    } else {
                                                        g.pw_current_input.clear();
                                                        g.pw_new_input.clear();
                                                        g.input_target = ActiveInputTarget::TerminalDirect;
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ))
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
                                                this.mesh_hub.sign_out();
                                                cx.notify();
                                            }),
                                        )),
                                ),
                        )
                        .when(st.pw_change_expanded, |el2| {
                            let cur_focused = st.input_target == ActiveInputTarget::ChangePwCurrent;
                            let new_focused = st.input_target == ActiveInputTarget::ChangePwNew;
                            let cur_mask = "•".repeat(st.pw_current_input.chars().count());
                            let new_mask = "•".repeat(st.pw_new_input.chars().count());
                            el2.child(
                                div()
                                    .mt(px(4.0))
                                    .p(px(8.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .id("settings-pw-current")
                                                    .occlude()
                                                    .flex_1()
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if cur_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(if st.pw_current_input.is_empty() {
                                                        palette::TEXT_DIM
                                                    } else {
                                                        palette::TEXT_PRIMARY
                                                    }))
                                                    .child(if st.pw_current_input.is_empty() {
                                                        if cur_focused {
                                                            "|".to_string()
                                                        } else {
                                                            tr(loc, "Current password", "当前密码").to_string()
                                                        }
                                                    } else if cur_focused {
                                                        format!("{cur_mask}|")
                                                    } else {
                                                        cur_mask
                                                    })
                                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.input_target = ActiveInputTarget::ChangePwCurrent;
                                                            g.input_selected_all = false;
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("settings-pw-new")
                                                    .occlude()
                                                    .flex_1()
                                                    .px(px(8.0))
                                                    .py(px(5.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CANVAS_BG))
                                                    .border_1()
                                                    .border_color(rgb(if new_focused {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .cursor_pointer()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(if st.pw_new_input.is_empty() {
                                                        palette::TEXT_DIM
                                                    } else {
                                                        palette::TEXT_PRIMARY
                                                    }))
                                                    .child(if st.pw_new_input.is_empty() {
                                                        if new_focused {
                                                            "|".to_string()
                                                        } else {
                                                            tr(loc, "New password (8+ chars)", "新密码（至少 8 位）").to_string()
                                                        }
                                                    } else if new_focused {
                                                        format!("{new_mask}|")
                                                    } else {
                                                        new_mask
                                                    })
                                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                        if let Ok(mut g) = this.state.lock() {
                                                            g.input_target = ActiveInputTarget::ChangePwNew;
                                                            g.input_selected_all = false;
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(self.render_setting_pill(
                                                "settings-pw-submit",
                                                tr(loc, "Update", "确认更新"),
                                                true,
                                                accent,
                                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                                    this.submit_change_password();
                                                    cx.notify();
                                                }),
                                            )),
                                    )
                                    .when_some(st.auth_error.clone(), |err_el, err_msg| {
                                        err_el.child(
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(rgb(palette::ERROR_RED))
                                                .child(err_msg),
                                        )
                                    }),
                            )
                        })
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
                                                    .map(|mut g| {
                                                        g.auth_loading = true;
                                                        g.auth_error = None;
                                                        g.server_url.clone()
                                                    })
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
                                        .border_color(rgb(if acct_focused {
                                            accent
                                        } else {
                                            palette::BORDER
                                        }))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(
                                            if st.auth_account_input.is_empty() && !acct_focused {
                                                palette::TEXT_DIM
                                            } else {
                                                palette::TEXT_PRIMARY
                                            },
                                        ))
                                        .child(if st.auth_account_input.is_empty() {
                                            if acct_focused {
                                                "|".to_string()
                                            } else {
                                                tr(loc, "Email / Username", "账号邮箱").to_string()
                                            }
                                        } else if acct_focused {
                                            format!("{}|", st.auth_account_input)
                                        } else {
                                            st.auth_account_input.clone()
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.input_target = ActiveInputTarget::AuthAccount;
                                                g.input_selected_all = false;
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
                                        .border_color(rgb(if pass_focused {
                                            accent
                                        } else {
                                            palette::BORDER
                                        }))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(
                                            if st.auth_password_input.is_empty() && !pass_focused {
                                                palette::TEXT_DIM
                                            } else {
                                                palette::TEXT_PRIMARY
                                            },
                                        ))
                                        .child(if st.auth_password_input.is_empty() {
                                            if pass_focused {
                                                "|".to_string()
                                            } else {
                                                tr(loc, "Password", "密码").to_string()
                                            }
                                        } else {
                                            let dots =
                                                "*".repeat(st.auth_password_input.chars().count());
                                            if pass_focused {
                                                format!("{dots}|")
                                            } else {
                                                dots
                                            }
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.input_target = ActiveInputTarget::AuthPassword;
                                                g.input_selected_all = false;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("settings-btn-password-signin")
                                        .occlude()
                                        .flex_shrink_0()
                                        .px(px(12.0))
                                        .py(px(5.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(accent))
                                        .hover(|h| h.opacity(0.9))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(palette::CANVAS_BG))
                                        .child(if st.auth_loading {
                                            tr(loc, "Signing in…", "登录中…")
                                        } else {
                                            tr(loc, "Sign In", "登录")
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.submit_password_login();
                                            cx.notify();
                                        })),
                                ),
                        )
                        .when_some(st.auth_error.clone(), |el, err| {
                            el.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(palette::ERROR_RED))
                                    .text_size(px(10.5))
                                    .text_color(rgb(palette::ERROR_RED))
                                    .child(err),
                            )
                        })
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
                // Machines list (local Mac + Host Mode toggle + ALL remote hosts with Remove action)
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
                                            .bg(rgb(if st.embedded_host_active || st.external_daemon_running {
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
                                            .child(format!(
                                                "{} · {}",
                                                tr(loc, "This Mac", "本机"),
                                                st.local_machine_name
                                            )),
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
                                            .child(if st.external_daemon_running {
                                                "daemon"
                                            } else if host_mode_on {
                                                "hybrid"
                                            } else {
                                                "client"
                                            }),
                                    )
                                    .child(self.render_setting_pill(
                                        "settings-toggle-remote-host",
                                        if host_mode_on {
                                            tr(loc, "Allow Control: ON", "允许被控: 开")
                                        } else {
                                            tr(loc, "Allow Control: OFF", "允许被控: 关")
                                        },
                                        host_mode_on,
                                        accent,
                                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.mesh_hub.set_allow_remote_control(!host_mode_on);
                                            cx.notify();
                                        }),
                                    ))
                                    .child({
                                        let notify_on = st.desktop_notifications;
                                        self.render_setting_pill(
                                            "settings-toggle-desktop-notify",
                                            if notify_on {
                                                tr(loc, "Alerts: ON", "通知: 开")
                                            } else {
                                                tr(loc, "Alerts: OFF", "通知: 关")
                                            },
                                            notify_on,
                                            accent,
                                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                                let next = !notify_on;
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.desktop_notifications = next;
                                                    g.status_notice = Some(
                                                        if next {
                                                            tr(
                                                                g.locale,
                                                                "Native macOS OSC 7501 alerts enabled",
                                                                "已开启 macOS 原生 OSC 7501 系统通知",
                                                            )
                                                        } else {
                                                            tr(
                                                                g.locale,
                                                                "Native macOS OSC 7501 alerts muted",
                                                                "已关闭 macOS 原生系统通知",
                                                            )
                                                        }
                                                        .to_string(),
                                                    );
                                                }
                                                CodaGpuWorkspaceView::update_prefs_async(move |p| {
                                                    p.desktop_notifications = next;
                                                });
                                                cx.notify();
                                            }),
                                        )
                                    })
                                    .child(self.render_setting_pill(
                                        "settings-probe-lan-now",
                                        tr(loc, "Probe LAN", "探测直连"),
                                        false,
                                        accent,
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.mesh_hub.probe_all_relay_peers_now();
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    )
                    .children(st.remote_hosts.iter().map(|host| {
                        let host_clone = host.clone();
                        let rename_id = host.id;
                        let rename_cur_name = host.name.clone();
                        let remove_id = host.id;
                        let is_renaming = st.renaming_host_id == Some(host.id);
                        let rename_focused = st.input_target == ActiveInputTarget::RenameDeviceInput;
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
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
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(accent))
                                                    .child(tr(loc, "Open Workspace", "进入工作区")),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "settings-rename-host-{}",
                                                        host.id
                                                    )))
                                                    .occlude()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CHROME_BG))
                                                    .border_1()
                                                    .border_color(rgb(if is_renaming {
                                                        accent
                                                    } else {
                                                        palette::BORDER
                                                    }))
                                                    .hover(|h| {
                                                        h.border_color(rgb(accent))
                                                            .text_color(rgb(palette::TEXT_PRIMARY))
                                                    })
                                                    .cursor_pointer()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(palette::TEXT_MUTED))
                                                    .child(tr(loc, "Rename", "改名"))
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                        cx.stop_propagation();
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            if let Ok(mut g) = this.state.lock() {
                                                                if g.renaming_host_id == Some(rename_id) {
                                                                    g.renaming_host_id = None;
                                                                    g.rename_host_input.clear();
                                                                    g.input_target = ActiveInputTarget::TerminalDirect;
                                                                } else {
                                                                    g.renaming_host_id = Some(rename_id);
                                                                    g.rename_host_input = rename_cur_name.clone();
                                                                    g.input_selected_all = true;
                                                                    g.input_target = ActiveInputTarget::RenameDeviceInput;
                                                                }
                                                            }
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "settings-remove-host-{}",
                                                        host.id
                                                    )))
                                                    .occlude()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(palette::CHROME_BG))
                                                    .border_1()
                                                    .border_color(rgb(palette::BORDER))
                                                    .hover(|h| {
                                                        h.border_color(rgb(palette::ERROR_RED))
                                                            .text_color(rgb(palette::ERROR_RED))
                                                    })
                                                    .cursor_pointer()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(palette::TEXT_MUTED))
                                                    .child(tr(loc, "Remove", "移除"))
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                        cx.stop_propagation();
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.mesh_hub.delete_remote_device(remove_id);
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.mesh_hub.probe_host(host_clone.clone());
                                        if let Ok(mut g) = this.state.lock() {
                                            g.settings_open = false;
                                            g.input_target = ActiveInputTarget::TerminalDirect;
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(is_renaming, |row_el| {
                                row_el.child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(5.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(palette::CHROME_BG))
                                        .border_1()
                                        .border_color(rgb(palette::BORDER))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .id("settings-rename-input")
                                                .occlude()
                                                .flex_1()
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(palette::CANVAS_BG))
                                                .border_1()
                                                .border_color(rgb(if rename_focused {
                                                    accent
                                                } else {
                                                    palette::BORDER
                                                }))
                                                .cursor_pointer()
                                                .text_size(px(11.0))
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .child(if rename_focused {
                                                    format!("{}|", st.rename_host_input)
                                                } else {
                                                    st.rename_host_input.clone()
                                                })
                                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.input_target = ActiveInputTarget::RenameDeviceInput;
                                                        g.input_selected_all = false;
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .child(self.render_setting_pill(
                                            "settings-rename-save",
                                            tr(loc, "Save", "保存"),
                                            true,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                this.submit_rename_device();
                                                cx.notify();
                                            }),
                                        ))
                                        .child(self.render_setting_pill(
                                            "settings-rename-cancel",
                                            tr(loc, "Cancel", "取消"),
                                            false,
                                            accent,
                                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                                if let Ok(mut g) = this.state.lock() {
                                                    g.renaming_host_id = None;
                                                    g.rename_host_input.clear();
                                                    g.input_target = ActiveInputTarget::TerminalDirect;
                                                }
                                                cx.notify();
                                            }),
                                        )),
                                )
                            })
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

fn render_styled_terminal_line(
    line: &StyledTerminalLine,
    line_height_px: f32,
    selection_cols: Option<(usize, usize)>,
) -> impl IntoElement {
    if line.text.is_empty() {
        return div()
            .h(px(line_height_px))
            .overflow_hidden()
            .whitespace_nowrap()
            .child(" ")
            .into_any_element();
    }
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = line
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

    if let Some((col_start, col_end)) = selection_cols {
        if col_start < col_end {
            let char_offsets: Vec<usize> = line
                .text
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(line.text.len()))
                .collect();
            let last_idx = char_offsets.len().saturating_sub(1);
            let b_start = char_offsets[col_start.min(last_idx)];
            let b_end = char_offsets[col_end.min(last_idx)];
            if b_start < b_end {
                highlights.push((
                    b_start..b_end,
                    HighlightStyle {
                        background_color: Some(Hsla::from(rgba(0x38bdf855))),
                        ..Default::default()
                    },
                ));
            }
        }
    }

    div()
        .h(px(line_height_px))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_color(rgb(palette::TEXT_PRIMARY))
        .child(StyledText::new(line.text.clone()).with_highlights(highlights))
        .into_any_element()
}

// ============================================================================
// Background Workers (`herdr-core` persistent sessions & `coda-daemon` Mesh)
// ============================================================================

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
                        g.record_session_osc_status(&id, o);
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
                                        if let Some(osc) = herdr_status_to_osc(&Some(raw_st)) {
                                            g.record_session_osc_status(&sid, osc);
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

// ============================================================================
// Application Launcher & Direct PTY / Palette Keyboard Loop
// ============================================================================

fn open_main_workbench_window(cx: &mut App) -> gpui::Entity<CodaGpuWorkspaceView> {
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

    window
        .update(cx, |this, win, cx| {
            win.focus(&this.focus_handle);
            cx.entity()
        })
        .expect("failed to focus Coda GPUI desktop view")
}

pub fn launch_gpui_desktop_app() {
    ensure_macos_gui_path();

    let app = Application::new();
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            let _ = open_main_workbench_window(cx);
        }
        cx.activate(true);
    });
    app.run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", QuitApp, None),
            KeyBinding::new("cmd-shift-n", NewWindow, None),
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
            KeyBinding::new("cmd-c", CopyTerminalAction, None),
            KeyBinding::new("cmd-v", PasteTerminalAction, None),
            KeyBinding::new("cmd-a", SelectAllAction, None),
            KeyBinding::new("cmd-=", ZoomInTerminal, None),
            KeyBinding::new("cmd-+", ZoomInTerminal, None),
            KeyBinding::new("cmd--", ZoomOutTerminal, None),
            KeyBinding::new("cmd-0", ZoomResetTerminal, None),
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
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Copy", CopyTerminalAction),
                    MenuItem::action("Paste", PasteTerminalAction),
                    MenuItem::separator(),
                    MenuItem::action("Select All", SelectAllAction),
                ],
            },
            Menu {
                name: "Shell".into(),
                items: vec![
                    MenuItem::action("New Window", NewWindow),
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
                    MenuItem::action("Zoom In", ZoomInTerminal),
                    MenuItem::action("Zoom Out", ZoomOutTerminal),
                    MenuItem::action("Actual Size", ZoomResetTerminal),
                    MenuItem::separator(),
                    MenuItem::action("Main Workbench", SwitchLayoutWorkbench),
                    MenuItem::action("Terminals Grid", SwitchLayoutTerminals),
                ],
            },
        ]);
        cx.on_action(|_: &QuitApp, cx| cx.quit());
        cx.on_action(|_: &NewWindow, cx| {
            let _ = open_main_workbench_window(cx);
            cx.activate(true);
        });

        let view = open_main_workbench_window(cx);

        let view_settings = view.clone();
        cx.on_action(move |_: &ToggleSettings, cx| {
            if cx.windows().is_empty() {
                let reopened = open_main_workbench_window(cx);
                reopened.update(cx, |this, cx| {
                    this.toggle_settings_modal();
                    cx.notify();
                });
            } else {
                let _ = view_settings.update(cx, |this, cx| {
                    this.toggle_settings_modal();
                    cx.notify();
                });
            }
        });

        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::set_config_root_override;
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
        struct OverrideReset;
        impl Drop for OverrideReset {
            fn drop(&mut self) {
                set_config_root_override(None);
            }
        }

        let tmp_dir = std::env::temp_dir().join(format!("coda-layout-test-{}", Uuid::new_v4()));
        set_config_root_override(Some(tmp_dir.clone()));
        let _guard = OverrideReset;

        MalleableLayoutConfig::save_default_if_missing();
        assert!(tmp_dir.join("layout.json").is_file());
        let agents_path = ensure_agents_md_manual().expect("expected AGENTS.md path");
        assert!(agents_path.is_file());

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

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_vt_cjk_wide_cells_charset_utf8_osc_and_path_boundaries() {
        let mut vt = VtScreenBuffer::new(20, 6);
        // CJK wide char ("中") occupies 2 columns, followed immediately by ASCII 'a', then overwrite col 0
        vt.feed_bytes("中a\rX".as_bytes());
        // ESC ( B charset designator sequence must be consumed cleanly without leaking 'B'
        vt.feed_bytes(b"\x1b(B!");
        let lines = vt.render_lines(10);
        assert_eq!(lines[0], "X!a", "overwriting left half of wide char should clear continuation cell cleanly: {lines:?}");

        // UTF-8 multi-byte OSC 7501 status task
        vt.feed_bytes("\x1b]7501;state=working;title=测试任务中\x07".as_bytes());
        let osc = vt.osc_status().expect("expected UTF-8 OSC 7501 status");
        assert_eq!(osc.state, AgentLifecycleState::Working);
        assert_eq!(osc.task.as_deref(), Some("测试任务中"));

        // Path containment and compact display '/' boundary checks
        assert!(path_within_root("/Users/alice/repo/sub", "/Users/alice/repo"));
        assert!(!path_within_root("/Users/alice/repo2/sub", "/Users/alice/repo"));

        if let Ok(home) = std::env::var("HOME") {
            let trimmed_home = home.trim_end_matches('/');
            if !trimmed_home.is_empty() {
                let sibling = format!("{trimmed_home}-other/proj");
                assert_eq!(display_compact_path(&sibling), sibling);
                let inside = format!("{trimmed_home}/proj");
                assert_eq!(display_compact_path(&inside), "~/proj");
            }
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
