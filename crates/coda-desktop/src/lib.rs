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
pub mod modals;
pub mod notify;
pub mod port_preview;
pub mod pty;
pub mod remote_mesh;
pub mod search_export;
pub mod smart_path;
pub mod voice_dock;

pub use crate::remote_mesh::{NotificationWebhookSettings, RemoteMeshHub};

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
    detect_local_machine_name, discover_local_git_repositories, display_compact_path,
    ensure_agents_md_manual, ensure_macos_gui_path, is_safe_http_url,
    pick_folder_via_native_dialog, reveal_in_finder, space_avatar_color, tr, DesktopPreferences,
    LayoutMode, MalleableLayoutConfig, UiLocale,
};
use crate::git_inspector::{
    compute_repo_snapshot, list_workspace_dir, read_workspace_file_preview, resolve_contained_path,
    RepoInspectorSnapshot, WorkspaceFileEntry,
};
use crate::helpers::{
    classify_diff_line, extract_file_diff_lines, format_pasted_terminal_path, herdr_status_to_osc,
    load_desktop_daemon_config, utf16_slice, DiffLineKind,
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
        ToggleTerminalSearch,
        ExportActiveSession,
        ToggleVoiceDictation,
        JumpNextNeedsYou,
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
    TerminalSearch,
    AuthServerUrl,
    AuthAccount,
    AuthPassword,
    RenameDeviceInput,
    ChangePwCurrent,
    ChangePwNew,
    VoiceBaseUrl,
    VoiceModel,
    VoiceApiKey,
    PreviewPort,
    PreviewPath,
    WebhookBarkUrl,
    WebhookTelegramToken,
    WebhookTelegramChatId,
    WebhookFeishuUrl,
    WebhookDiscordUrl,
    WebhookGenericUrl,
    BootstrapParentDir,
    BootstrapFolderName,
    BootstrapGitCloneUrl,
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
    Preview,
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
        if !self.alive {
            return AgentLifecycleState::Done;
        }
        if let Some(ref osc) = self.osc_status {
            return osc.state;
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
    OpenRemoteBootstrapModal,
    OpenDrawer(RightDrawerTab),
    OpenSettings,
    ToggleLocale,
    OpenTerminalSearch,
    ExportActiveSession,
    ToggleVoiceDictation,
    JumpNextNeedsYou,
}

/// Modal state for creating a new remote workspace folder or running `git clone` over E2EE (`Gap #2`).
#[derive(Debug, Clone)]
pub struct RemoteBootstrapModalState {
    pub open: bool,
    pub host_id: Option<Uuid>,
    pub parent_dir: String,
    pub folder_name: String,
    pub git_clone_url: String,
    pub loading: bool,
    pub error: Option<String>,
}

impl Default for RemoteBootstrapModalState {
    fn default() -> Self {
        Self {
            open: false,
            host_id: None,
            parent_dir: "~/workspace".to_string(),
            folder_name: String::new(),
            git_clone_url: String::new(),
            loading: false,
            error: None,
        }
    }
}

impl RemoteBootstrapModalState {
    pub fn open_for_host(&mut self, host_id: Option<Uuid>) {
        self.open = true;
        self.host_id = host_id;
        if self.parent_dir.trim().is_empty() {
            self.parent_dir = "~/workspace".to_string();
        }
        self.folder_name.clear();
        self.git_clone_url.clear();
        self.loading = false;
        self.error = None;
    }

    pub fn reset(&mut self) {
        self.open = false;
        self.loading = false;
        self.error = None;
        self.folder_name.clear();
        self.git_clone_url.clear();
    }
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
    pub git_diff_loading: bool,
    pub remote_diff_seq: u64,
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
    pub app_is_active: bool,
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
    pub terminal_search_open: bool,
    pub terminal_search_query: String,
    pub terminal_search_index: usize,
    pub voice_base_url: String,
    pub voice_model: String,
    pub voice_api_key: String,
    pub voice_recording: bool,
    pub voice_transcribing: bool,
    pub webhook_settings: NotificationWebhookSettings,
    pub webhook_loading: bool,
    pub webhook_loaded_ok: bool,
    pub webhook_dirty: bool,
    pub port_preview: crate::port_preview::PortPreviewState,
    pub bootstrap_modal: RemoteBootstrapModalState,
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
            git_diff_loading: false,
            remote_diff_seq: 0,
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
            app_is_active: true,
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
            terminal_search_open: false,
            terminal_search_query: String::new(),
            terminal_search_index: 0,
            voice_base_url: DesktopPreferences::default().voice_base_url,
            voice_model: DesktopPreferences::default().voice_model,
            voice_api_key: String::new(),
            voice_recording: false,
            voice_transcribing: false,
            webhook_settings: NotificationWebhookSettings::default(),
            webhook_loading: false,
            webhook_loaded_ok: false,
            webhook_dirty: false,
            port_preview: crate::port_preview::PortPreviewState::default(),
            bootstrap_modal: RemoteBootstrapModalState::default(),
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

    /// Resolves `(device_id, cwd)` ONLY when the currently focused terminal session (or active
    /// space when no session is focused) targets a remote host — preventing split-pane local focus
    /// from being overwritten by a remote `GitDiffResponse` (`Bug #7`).
    pub fn effective_remote_diff_target(&self) -> Option<(Uuid, String)> {
        if let Some(sid) = self.focused_session_id() {
            if let Some(sess) = self.session_by_id(sid) {
                let raw_hid = sess.remote_host_id.as_deref()?;
                let hid = Uuid::parse_str(raw_hid).ok()?;
                let sp_path = self
                    .spaces
                    .iter()
                    .find(|sp| sp.id == sess.space_id)
                    .map(|sp| sp.path.clone())
                    .unwrap_or_else(|| ".".to_string());
                let eff_cwd = if !sess.cwd.trim().is_empty() && sess.cwd != "." {
                    sess.cwd.clone()
                } else {
                    sp_path
                };
                return Some((hid, eff_cwd));
            }
        }
        let sp = self.active_space().filter(|s| s.is_remote)?;
        let hid = Uuid::parse_str(&sp.machine_id).ok()?;
        Some((hid, sp.path.clone()))
    }

    /// Returns the next alive session in `NeedsYou` state using a circular scan over `self.sessions`
    /// starting at `(cur_idx + 1) % len`, so approving `S2` in `[S1, S2, S3]` advances to `S3`
    /// instead of wrapping backwards to `S1` (`Bug #8`).
    pub fn next_needs_you_session_id(&self) -> Option<String> {
        let len = self.sessions.len();
        if len == 0 {
            return None;
        }
        let start_idx = self
            .focused_session_id()
            .and_then(|cur_id| self.sessions.iter().position(|s| s.id == cur_id))
            .map(|idx| (idx + 1) % len)
            .unwrap_or(0);
        for offset in 0..len {
            let s = &self.sessions[(start_idx + offset) % len];
            if s.alive && s.effective_state() == AgentLifecycleState::NeedsYou {
                return Some(s.id.clone());
            }
        }
        None
    }

    /// Applies session selection with split-pane deduplication (switching `focused_pane` if the
    /// target session is already open in the opposite pane) and `LayoutMode::Terminals` grid
    /// visibility auto-switch when the target session is beyond the 4-tile grid (`Bug #8`).
    pub fn apply_select_session(&mut self, session_id: &str) -> Option<(bool, bool)> {
        let (sess_idx, space_id, is_local_sess) = self
            .sessions
            .iter()
            .enumerate()
            .find(|(_, s)| s.id == session_id)
            .map(|(idx, s)| (idx, s.space_id.clone(), s.remote_host_id.is_none()))?;

        let space_changed = self.active_space_id.as_deref() != Some(space_id.as_str());
        self.active_space_id = Some(space_id.clone());
        if space_changed {
            self.current_browse_rel.clear();
            self.selected_file_rel = None;
            self.selected_file_content.clear();
            self.selected_change_path = None;
        }
        if is_local_sess {
            self.remote_diff_seq = self.remote_diff_seq.wrapping_add(1);
            self.git_diff_loading = false;
        }

        let mut layout_changed = false;
        if self.layout.split_terminals {
            if self.active_session_id.as_deref() == Some(session_id) {
                self.focused_pane = SplitPane::Primary;
            } else if self.split_session_id.as_deref() == Some(session_id) {
                self.focused_pane = SplitPane::Secondary;
            } else if self.focused_pane == SplitPane::Secondary {
                self.split_session_id = Some(session_id.to_string());
                if space_changed {
                    let primary_in_space = self
                        .active_session_id
                        .as_deref()
                        .and_then(|id| self.session_by_id(id))
                        .map(|s| s.space_id == space_id)
                        .unwrap_or(false);
                    if !primary_in_space {
                        self.active_session_id = self
                            .sessions
                            .iter()
                            .find(|s| s.space_id == space_id && s.id != session_id)
                            .map(|s| s.id.clone());
                    }
                }
            } else {
                self.active_session_id = Some(session_id.to_string());
                if space_changed {
                    let secondary_in_space = self
                        .split_session_id
                        .as_deref()
                        .and_then(|id| self.session_by_id(id))
                        .map(|s| s.space_id == space_id)
                        .unwrap_or(false);
                    if !secondary_in_space {
                        self.split_session_id = self
                            .sessions
                            .iter()
                            .find(|s| s.space_id == space_id && s.id != session_id)
                            .map(|s| s.id.clone());
                    }
                }
            }
        } else {
            self.active_session_id = Some(session_id.to_string());
        }

        if self.layout.layout == LayoutMode::Terminals && sess_idx >= 4 {
            self.layout.layout = LayoutMode::Workbench;
            layout_changed = true;
        }

        self.input_target = ActiveInputTarget::TerminalDirect;
        Some((space_changed, layout_changed))
    }

    /// Clears transient Settings inputs (inline rename + plaintext password buffers) so they never
    /// leak across tab switches, modal closes, Escape, or sign-out.
    pub fn clear_settings_ephemeral_inputs(&mut self) {
        self.renaming_host_id = None;
        self.rename_host_input.clear();
        self.pw_change_expanded = false;
        self.pw_current_input.clear();
        self.pw_new_input.clear();
        self.input_selected_all = false;
        if matches!(
            self.input_target,
            ActiveInputTarget::RenameDeviceInput
                | ActiveInputTarget::ChangePwCurrent
                | ActiveInputTarget::ChangePwNew
                | ActiveInputTarget::AuthServerUrl
                | ActiveInputTarget::AuthAccount
                | ActiveInputTarget::AuthPassword
                | ActiveInputTarget::VoiceBaseUrl
                | ActiveInputTarget::VoiceModel
                | ActiveInputTarget::VoiceApiKey
                | ActiveInputTarget::WebhookBarkUrl
                | ActiveInputTarget::WebhookTelegramToken
                | ActiveInputTarget::WebhookTelegramChatId
                | ActiveInputTarget::WebhookFeishuUrl
                | ActiveInputTarget::WebhookDiscordUrl
                | ActiveInputTarget::WebhookGenericUrl
        ) {
            self.input_target = ActiveInputTarget::TerminalDirect;
        }
    }

    pub fn record_session_osc_status(&mut self, session_id: &str, osc: Osc7501Status) {
        let title = if let Some(sess) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            sess.osc_status = Some(osc.clone());
            sess.title.clone()
        } else {
            return;
        };
        let active_id = self.focused_session_id().map(str::to_string);
        if let Some(payload) = self.notify_tracker.observe(
            session_id,
            &title,
            &osc,
            active_id.as_deref(),
            self.app_is_active,
            self.locale,
            Instant::now(),
        ) {
            if self.desktop_notifications {
                dispatch_native_notification(payload);
            }
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
            kind_tag: "AGENT",
            label: tr(self.locale, "Launch Grok Agent", "启动 Grok Agent").to_string(),
            detail: "agent · grok".to_string(),
            action: PaletteAction::LaunchAgent("grok"),
        });
        items.push(PaletteItem {
            kind_tag: "AGENT",
            label: tr(
                self.locale,
                "Launch Antigravity (AGY) Agent",
                "启动 Antigravity (AGY) Agent",
            )
            .to_string(),
            detail: "agent · agy".to_string(),
            action: PaletteAction::LaunchAgent("agy"),
        });
        items.push(PaletteItem {
            kind_tag: "AGENT",
            label: tr(self.locale, "Launch Gemini CLI Agent", "启动 Gemini CLI Agent").to_string(),
            detail: "agent · gemini".to_string(),
            action: PaletteAction::LaunchAgent("gemini"),
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Add Workspace Folder...", "添加本地工作区...").to_string(),
            detail: "Finder picker".to_string(),
            action: PaletteAction::AddSpaceFolder,
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(
                self.locale,
                "New Remote Workspace / Git Clone…",
                "新建远端工作区 / Git Clone…",
            )
            .to_string(),
            detail: "E2EE · mkdir / git clone".to_string(),
            action: PaletteAction::OpenRemoteBootstrapModal,
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
            label: tr(
                self.locale,
                "Toggle Localhost Port Preview Panel",
                "打开本地/远端端口加密预览侧栏",
            )
            .to_string(),
            detail: "127.0.0.1:PORT · E2EE".to_string(),
            action: PaletteAction::OpenDrawer(RightDrawerTab::Preview),
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Connect Remote Machine (Mesh)", "打开远端节点 (Mesh)").to_string(),
            detail: "E2EE P2P / WSS".to_string(),
            action: PaletteAction::OpenDrawer(RightDrawerTab::Mesh),
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(self.locale, "Find in Terminal…", "终端内搜索…").to_string(),
            detail: "Cmd+F · search".to_string(),
            action: PaletteAction::OpenTerminalSearch,
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(
                self.locale,
                "Export Session Log (.md)",
                "导出当前会话日志 (.md)",
            )
            .to_string(),
            detail: "Cmd+Shift+E · markdown export".to_string(),
            action: PaletteAction::ExportActiveSession,
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(
                self.locale,
                "Jump to Next Needs-You Session",
                "跳转到下一个待确认 (Needs-You) 会话",
            )
            .to_string(),
            detail: "Cmd+J · Needs Attention".to_string(),
            action: PaletteAction::JumpNextNeedsYou,
        });
        items.push(PaletteItem {
            kind_tag: "ACTION",
            label: tr(
                self.locale,
                "Voice Dictation (Start / Stop)",
                "语音听写（开始 / 停止录音）",
            )
            .to_string(),
            detail: "Whisper · mic".to_string(),
            action: PaletteAction::ToggleVoiceDictation,
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
    pty_synced_gen: Mutex<HashMap<String, (u64, bool, u64)>>,
    inspector_seq: Arc<AtomicU64>,
    voice_ctrl: Arc<crate::voice_dock::DesktopVoiceController>,
    herdr_shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl Drop for CodaGpuWorkspaceView {
    fn drop(&mut self) {
        let _ = self.herdr_shutdown_tx.send(true);
        self.mesh_hub.shutdown();
        if let Ok(mut mgr) = self.pty_mgr.lock() {
            mgr.shutdown_all();
        }
    }
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
        init_state.voice_base_url = prefs.voice_base_url.clone();
        init_state.voice_model = prefs.voice_model.clone();
        init_state.voice_api_key = prefs.voice_api_key.clone();
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
        let (herdr_shutdown_tx, herdr_shutdown_rx) = tokio::sync::watch::channel(false);

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
            voice_ctrl: crate::voice_dock::DesktopVoiceController::new(),
            herdr_shutdown_tx,
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

        spawn_herdr_discovery(
            Arc::downgrade(&state),
            Arc::downgrade(&pty_mgr),
            Arc::downgrade(&ui_dirty),
            herdr_shutdown_rx,
        );
        mesh_hub.ensure_embedded_host();
        mesh_hub.start_account_sync();
        if state.lock().map(|g| g.is_logged_in).unwrap_or(false) {
            mesh_hub.fetch_notification_webhooks();
        }

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

    pub(crate) fn update_prefs_async<F>(f: F)
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
                ActiveInputTarget::TerminalSearch => {
                    (!g.terminal_search_query.is_empty())
                        .then(|| (g.terminal_search_query.clone(), false))
                }
                ActiveInputTarget::VoiceBaseUrl => {
                    (!g.voice_base_url.is_empty()).then(|| (g.voice_base_url.clone(), false))
                }
                ActiveInputTarget::VoiceModel => {
                    (!g.voice_model.is_empty()).then(|| (g.voice_model.clone(), false))
                }
                ActiveInputTarget::PreviewPort => {
                    (!g.port_preview.port_input.is_empty())
                        .then(|| (g.port_preview.port_input.clone(), false))
                }
                ActiveInputTarget::PreviewPath => {
                    (!g.port_preview.path_input.is_empty())
                        .then(|| (g.port_preview.path_input.clone(), false))
                }
                ActiveInputTarget::WebhookBarkUrl => {
                    (!g.webhook_settings.bark_url.is_empty())
                        .then(|| (g.webhook_settings.bark_url.clone(), false))
                }
                ActiveInputTarget::WebhookTelegramChatId => {
                    (!g.webhook_settings.telegram_chat_id.is_empty())
                        .then(|| (g.webhook_settings.telegram_chat_id.clone(), false))
                }
                ActiveInputTarget::WebhookFeishuUrl => {
                    (!g.webhook_settings.feishu_webhook_url.is_empty())
                        .then(|| (g.webhook_settings.feishu_webhook_url.clone(), false))
                }
                ActiveInputTarget::WebhookDiscordUrl => {
                    (!g.webhook_settings.discord_webhook_url.is_empty())
                        .then(|| (g.webhook_settings.discord_webhook_url.clone(), false))
                }
                ActiveInputTarget::AuthPassword
                | ActiveInputTarget::ChangePwCurrent
                | ActiveInputTarget::ChangePwNew
                | ActiveInputTarget::VoiceApiKey
                | ActiveInputTarget::WebhookTelegramToken => None,
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
        let (target, palette_open, settings_open, bootstrap_open, focused_id) = self
            .state
            .lock()
            .map(|g| {
                (
                    g.input_target,
                    g.palette_open,
                    g.settings_open,
                    g.bootstrap_modal.open,
                    g.focused_session_id().map(|s| s.to_string()),
                )
            })
            .unwrap_or((ActiveInputTarget::TerminalDirect, false, false, false, None));

        let clip_text = cx
            .read_from_clipboard()
            .and_then(|i| i.text())
            .filter(|s| !s.is_empty());

        if !palette_open
            && !settings_open
            && !bootstrap_open
            && target == ActiveInputTarget::TerminalDirect
        {
            let Some(id) = focused_id else {
                return;
            };
            let maybe_finder_image = clip_text
                .as_deref()
                .map(crate::helpers::looks_like_copied_image_filename)
                .unwrap_or(false);
            if let Some(txt) = clip_text.filter(|_| !maybe_finder_image) {
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
                    let safe_txt = crate::helpers::sanitize_bracketed_paste_text(&txt);
                    let mut bytes = Vec::with_capacity(safe_txt.len() + 12);
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(safe_txt.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                    self.send_bytes_to_session(&id, &bytes);
                } else {
                    self.send_bytes_to_session(&id, txt.as_bytes());
                }
                cx.notify();
            } else {
                // Either clipboard holds no text OR looks like a Finder-copied image filename —
                // check `«class furl»` / `«class PNGf»` with bounded 5s timeout & fallback to text!
                let fallback_txt = cx
                    .read_from_clipboard()
                    .and_then(|i| i.text())
                    .filter(|s| !s.is_empty());
                self.paste_clipboard_image_into_session(&id, fallback_txt);
                cx.notify();
            }
            return;
        }

        if let Some(txt) = clip_text {
            let payload = if palette_open
                || matches!(
                    target,
                    ActiveInputTarget::CommandPalette
                        | ActiveInputTarget::TerminalSearch
                        | ActiveInputTarget::AuthServerUrl
                        | ActiveInputTarget::AuthAccount
                        | ActiveInputTarget::RenameDeviceInput
                        | ActiveInputTarget::VoiceBaseUrl
                        | ActiveInputTarget::VoiceModel
                        | ActiveInputTarget::PreviewPort
                        | ActiveInputTarget::PreviewPath
                        | ActiveInputTarget::WebhookBarkUrl
                        | ActiveInputTarget::WebhookTelegramChatId
                        | ActiveInputTarget::WebhookFeishuUrl
                        | ActiveInputTarget::WebhookDiscordUrl
                        | ActiveInputTarget::WebhookGenericUrl
                        | ActiveInputTarget::BootstrapParentDir
                        | ActiveInputTarget::BootstrapFolderName
                        | ActiveInputTarget::BootstrapGitCloneUrl
                ) {
                txt.trim().to_string()
            } else if matches!(
                target,
                ActiveInputTarget::AuthPassword
                    | ActiveInputTarget::ChangePwCurrent
                    | ActiveInputTarget::ChangePwNew
                    | ActiveInputTarget::VoiceApiKey
                    | ActiveInputTarget::WebhookTelegramToken
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

    fn paste_clipboard_image_into_session(&self, session_id: &str, fallback_text: Option<String>) {
        let remote_info = self.state.lock().ok().and_then(|g| {
            let s = g.session_by_id(session_id)?;
            let hid = Uuid::parse_str(s.remote_host_id.as_deref()?).ok()?;
            let term = s.hc_addr.clone()?;
            Some((hid, term))
        });
        let session_id_owned = session_id.to_string();
        let state_bg = Arc::clone(&self.state);
        let pty_mgr_bg = Arc::clone(&self.pty_mgr);
        let mesh_hub_bg = Arc::clone(&self.mesh_hub);
        let ui_dirty_bg = Arc::clone(&self.ui_dirty);

        std::thread::spawn(move || {
            let extracted = crate::helpers::extract_clipboard_image_or_file();
            match extracted {
                Ok(crate::helpers::ClipboardImagePayload::LocalImageFile(file_path)) => {
                    if let Some((host_id, term_addr)) = remote_info {
                        match std::fs::read(&file_path) {
                            Ok(raw_bytes) => {
                                let ext = file_path
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .filter(|e| !e.is_empty())
                                    .unwrap_or("png");
                                let fname = format!(
                                    "clipboard-{}.{ext}",
                                    &Uuid::new_v4().simple().to_string()[..12]
                                );
                                mesh_hub_bg.upload_clipboard_png_and_paste(
                                    host_id,
                                    session_id_owned,
                                    term_addr,
                                    fname,
                                    raw_bytes,
                                );
                            }
                            Err(e) => {
                                if let Ok(mut g) = state_bg.lock() {
                                    g.status_notice =
                                        Some(format!("Failed reading copied image file: {e}"));
                                }
                                ui_dirty_bg.store(true, Ordering::Release);
                            }
                        }
                    } else {
                        let pasted = format_pasted_terminal_path(&file_path.to_string_lossy());
                        if let Ok(mgr) = pty_mgr_bg.lock() {
                            let _ = mgr.write_input(&session_id_owned, pasted.as_bytes());
                        }
                        if let Ok(mut g) = state_bg.lock() {
                            if let Some(s) =
                                g.sessions.iter_mut().find(|s| s.id == session_id_owned)
                            {
                                s.scroll_offset = 0;
                            }
                            g.status_notice = Some(format!(
                                "{}: {}",
                                tr(g.locale, "Pasted image path", "已粘贴图片文件路径"),
                                file_path.display()
                            ));
                        }
                        ui_dirty_bg.store(true, Ordering::Release);
                    }
                }
                Ok(crate::helpers::ClipboardImagePayload::PngBytes(png_bytes)) => {
                    if let Some((host_id, term_addr)) = remote_info {
                        let fname = format!(
                            "clipboard-{}.png",
                            &Uuid::new_v4().simple().to_string()[..12]
                        );
                        mesh_hub_bg.upload_clipboard_png_and_paste(
                            host_id,
                            session_id_owned,
                            term_addr,
                            fname,
                            png_bytes,
                        );
                    } else {
                        match crate::helpers::save_clipboard_png_to_tmp(&png_bytes) {
                            Ok(saved_path) => {
                                let pasted =
                                    format_pasted_terminal_path(&saved_path.to_string_lossy());
                                if let Ok(mgr) = pty_mgr_bg.lock() {
                                    let _ = mgr.write_input(&session_id_owned, pasted.as_bytes());
                                }
                                if let Ok(mut g) = state_bg.lock() {
                                    if let Some(s) =
                                        g.sessions.iter_mut().find(|s| s.id == session_id_owned)
                                    {
                                        s.scroll_offset = 0;
                                    }
                                    g.status_notice = Some(format!(
                                        "{}: {}",
                                        tr(g.locale, "Pasted screenshot", "已粘贴本地截图"),
                                        saved_path.display()
                                    ));
                                }
                            }
                            Err(err) => {
                                if let Ok(mut g) = state_bg.lock() {
                                    g.status_notice = Some(err);
                                }
                            }
                        }
                        ui_dirty_bg.store(true, Ordering::Release);
                    }
                }
                Err(err) => {
                    if let Some(txt) = fallback_text {
                        let bracketed = pty_mgr_bg
                            .lock()
                            .map(|m| m.terminal_modes(&session_id_owned).1)
                            .unwrap_or(false);
                        let payload = if bracketed {
                            let safe_txt = crate::helpers::sanitize_bracketed_paste_text(&txt);
                            let mut b = Vec::with_capacity(safe_txt.len() + 12);
                            b.extend_from_slice(b"\x1b[200~");
                            b.extend_from_slice(safe_txt.as_bytes());
                            b.extend_from_slice(b"\x1b[201~");
                            b
                        } else {
                            txt.into_bytes()
                        };
                        if let Some((host_id, term_addr)) = remote_info {
                            mesh_hub_bg.send_remote_pty_input(host_id, &term_addr, payload);
                        } else if let Ok(mgr) = pty_mgr_bg.lock() {
                            let _ = mgr.write_input(&session_id_owned, &payload);
                        }
                        if let Ok(mut g) = state_bg.lock() {
                            if let Some(s) =
                                g.sessions.iter_mut().find(|s| s.id == session_id_owned)
                            {
                                s.scroll_offset = 0;
                            }
                        }
                    } else if !err.contains("already in progress") {
                        if let Ok(mut g) = state_bg.lock() {
                            g.status_notice = Some(
                                tr(
                                    g.locale,
                                    "Clipboard is empty (copy text or a screenshot first)",
                                    "剪贴板为空（请先复制文本或截图）",
                                )
                                .to_string(),
                            );
                        }
                    }
                    ui_dirty_bg.store(true, Ordering::Release);
                }
            }
        });
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
            ActiveInputTarget::TerminalSearch => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.terminal_search_query.clear();
                        g.input_selected_all = false;
                    }
                    g.terminal_search_query.push_str(text);
                    g.terminal_search_index = 0;
                }
                self.jump_to_current_search_match();
            }
            ActiveInputTarget::VoiceBaseUrl => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.voice_base_url.clear();
                        g.input_selected_all = false;
                    }
                    g.voice_base_url.push_str(text);
                }
            }
            ActiveInputTarget::VoiceModel => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.voice_model.clear();
                        g.input_selected_all = false;
                    }
                    g.voice_model.push_str(text);
                }
            }
            ActiveInputTarget::VoiceApiKey => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.voice_api_key.clear();
                        g.input_selected_all = false;
                    }
                    g.voice_api_key.push_str(text);
                }
            }
            ActiveInputTarget::PreviewPort => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.port_preview.port_input.clear();
                        g.input_selected_all = false;
                    }
                    for ch in text.chars() {
                        if ch.is_ascii_digit() && g.port_preview.port_input.len() < 5 {
                            g.port_preview.port_input.push(ch);
                        }
                    }
                }
            }
            ActiveInputTarget::PreviewPath => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.port_preview.path_input.clear();
                        g.input_selected_all = false;
                    }
                    g.port_preview.path_input.push_str(text);
                }
            }
            ActiveInputTarget::WebhookBarkUrl => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.webhook_settings.bark_url.clear();
                        g.input_selected_all = false;
                    }
                    g.webhook_settings.bark_url.push_str(text);
                    g.webhook_dirty = true;
                }
            }
            ActiveInputTarget::WebhookTelegramToken => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.webhook_settings.telegram_bot_token.clear();
                        g.input_selected_all = false;
                    }
                    g.webhook_settings.telegram_bot_token.push_str(text);
                    g.webhook_dirty = true;
                }
            }
            ActiveInputTarget::WebhookTelegramChatId => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.webhook_settings.telegram_chat_id.clear();
                        g.input_selected_all = false;
                    }
                    g.webhook_settings.telegram_chat_id.push_str(text);
                    g.webhook_dirty = true;
                }
            }
            ActiveInputTarget::WebhookFeishuUrl => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.webhook_settings.feishu_webhook_url.clear();
                        g.input_selected_all = false;
                    }
                    g.webhook_settings.feishu_webhook_url.push_str(text);
                    g.webhook_dirty = true;
                }
            }
            ActiveInputTarget::WebhookDiscordUrl => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.webhook_settings.discord_webhook_url.clear();
                        g.input_selected_all = false;
                    }
                    g.webhook_settings.discord_webhook_url.push_str(text);
                    g.webhook_dirty = true;
                }
            }
            ActiveInputTarget::WebhookGenericUrl => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.webhook_settings.generic_webhook_url.clear();
                        g.input_selected_all = false;
                    }
                    g.webhook_settings.generic_webhook_url.push_str(text);
                    g.webhook_dirty = true;
                }
            }
            ActiveInputTarget::BootstrapParentDir => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.bootstrap_modal.parent_dir.clear();
                        g.input_selected_all = false;
                    }
                    g.bootstrap_modal.parent_dir.push_str(text);
                    g.bootstrap_modal.error = None;
                }
            }
            ActiveInputTarget::BootstrapFolderName => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.bootstrap_modal.folder_name.clear();
                        g.input_selected_all = false;
                    }
                    g.bootstrap_modal.folder_name.push_str(text);
                    g.bootstrap_modal.error = None;
                }
            }
            ActiveInputTarget::BootstrapGitCloneUrl => {
                if let Ok(mut g) = self.state.lock() {
                    if g.input_selected_all {
                        g.bootstrap_modal.git_clone_url.clear();
                        g.input_selected_all = false;
                    }
                    g.bootstrap_modal.git_clone_url.push_str(text);
                    g.bootstrap_modal.error = None;
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
                                ActiveInputTarget::TerminalSearch => {
                                    g.terminal_search_query.clear();
                                    g.terminal_search_index = 0;
                                }
                                ActiveInputTarget::VoiceBaseUrl => g.voice_base_url.clear(),
                                ActiveInputTarget::VoiceModel => g.voice_model.clear(),
                                ActiveInputTarget::VoiceApiKey => g.voice_api_key.clear(),
                                ActiveInputTarget::PreviewPort => g.port_preview.port_input.clear(),
                                ActiveInputTarget::PreviewPath => g.port_preview.path_input.clear(),
                                ActiveInputTarget::WebhookBarkUrl => {
                                    g.webhook_settings.bark_url.clear();
                                    g.webhook_dirty = true;
                                }
                                ActiveInputTarget::WebhookTelegramToken => {
                                    g.webhook_settings.telegram_bot_token.clear();
                                    g.webhook_dirty = true;
                                }
                                ActiveInputTarget::WebhookTelegramChatId => {
                                    g.webhook_settings.telegram_chat_id.clear();
                                    g.webhook_dirty = true;
                                }
                                ActiveInputTarget::WebhookFeishuUrl => {
                                    g.webhook_settings.feishu_webhook_url.clear();
                                    g.webhook_dirty = true;
                                }
                                ActiveInputTarget::WebhookDiscordUrl => {
                                    g.webhook_settings.discord_webhook_url.clear();
                                    g.webhook_dirty = true;
                                }
                                ActiveInputTarget::WebhookGenericUrl => {
                                    g.webhook_settings.generic_webhook_url.clear();
                                    g.webhook_dirty = true;
                                }
                                ActiveInputTarget::BootstrapParentDir => {
                                    g.bootstrap_modal.parent_dir.clear();
                                    g.bootstrap_modal.error = None;
                                }
                                ActiveInputTarget::BootstrapFolderName => {
                                    g.bootstrap_modal.folder_name.clear();
                                    g.bootstrap_modal.error = None;
                                }
                                ActiveInputTarget::BootstrapGitCloneUrl => {
                                    g.bootstrap_modal.git_clone_url.clear();
                                    g.bootstrap_modal.error = None;
                                }
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
                "f" => {
                    self.toggle_terminal_search();
                    cx.notify();
                    cx.stop_propagation();
                }
                "j" => {
                    self.jump_to_next_needs_you_session();
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
                    g.clear_settings_ephemeral_inputs();
                }
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }

        let bootstrap_open = self
            .state
            .lock()
            .map(|g| g.bootstrap_modal.open)
            .unwrap_or(false);
        if bootstrap_open && target == ActiveInputTarget::TerminalDirect {
            if key == "escape" {
                if let Ok(mut g) = self.state.lock() {
                    if !g.bootstrap_modal.loading {
                        g.bootstrap_modal.reset();
                    }
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
                        g.pw_change_expanded = false;
                        g.pw_current_input.clear();
                        g.pw_new_input.clear();
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
                        g.pw_change_expanded = false;
                        g.pw_current_input.clear();
                        g.pw_new_input.clear();
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::TerminalSearch => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.terminal_search_query.clear();
                            g.input_selected_all = false;
                        } else {
                            g.terminal_search_query.pop();
                        }
                        g.terminal_search_index = 0;
                    }
                    self.jump_to_current_search_match();
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.step_terminal_search(!mods.shift);
                    cx.notify();
                    cx.stop_propagation();
                }
                "up" => {
                    self.step_terminal_search(false);
                    cx.notify();
                    cx.stop_propagation();
                }
                "down" => {
                    self.step_terminal_search(true);
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.terminal_search_open = false;
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                _ => {}
            },
            ActiveInputTarget::VoiceBaseUrl => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.voice_base_url.clear();
                            g.input_selected_all = false;
                        } else {
                            g.voice_base_url.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "tab" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::VoiceModel;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.save_voice_preferences();
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
            ActiveInputTarget::VoiceModel => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.voice_model.clear();
                            g.input_selected_all = false;
                        } else {
                            g.voice_model.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "tab" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::VoiceApiKey;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.save_voice_preferences();
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
            ActiveInputTarget::VoiceApiKey => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.voice_api_key.clear();
                            g.input_selected_all = false;
                        } else {
                            g.voice_api_key.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.save_voice_preferences();
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
            ActiveInputTarget::PreviewPort => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.port_preview.port_input.clear();
                            g.input_selected_all = false;
                        } else {
                            g.port_preview.port_input.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "tab" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = ActiveInputTarget::PreviewPath;
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.trigger_active_port_preview();
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
            ActiveInputTarget::PreviewPath => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        if g.input_selected_all {
                            g.port_preview.path_input.clear();
                            g.input_selected_all = false;
                        } else {
                            g.port_preview.path_input.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.trigger_active_port_preview();
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
            ActiveInputTarget::WebhookBarkUrl
            | ActiveInputTarget::WebhookTelegramToken
            | ActiveInputTarget::WebhookTelegramChatId
            | ActiveInputTarget::WebhookFeishuUrl
            | ActiveInputTarget::WebhookDiscordUrl
            | ActiveInputTarget::WebhookGenericUrl => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        let sel_all = g.input_selected_all;
                        g.input_selected_all = false;
                        g.webhook_dirty = true;
                        let field = match target {
                            ActiveInputTarget::WebhookBarkUrl => &mut g.webhook_settings.bark_url,
                            ActiveInputTarget::WebhookTelegramToken => {
                                &mut g.webhook_settings.telegram_bot_token
                            }
                            ActiveInputTarget::WebhookTelegramChatId => {
                                &mut g.webhook_settings.telegram_chat_id
                            }
                            ActiveInputTarget::WebhookFeishuUrl => {
                                &mut g.webhook_settings.feishu_webhook_url
                            }
                            ActiveInputTarget::WebhookDiscordUrl => {
                                &mut g.webhook_settings.discord_webhook_url
                            }
                            _ => &mut g.webhook_settings.generic_webhook_url,
                        };
                        if sel_all {
                            field.clear();
                        } else {
                            field.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "tab" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = match target {
                            ActiveInputTarget::WebhookBarkUrl => {
                                ActiveInputTarget::WebhookTelegramToken
                            }
                            ActiveInputTarget::WebhookTelegramToken => {
                                ActiveInputTarget::WebhookTelegramChatId
                            }
                            ActiveInputTarget::WebhookTelegramChatId => {
                                ActiveInputTarget::WebhookFeishuUrl
                            }
                            ActiveInputTarget::WebhookFeishuUrl => {
                                ActiveInputTarget::WebhookDiscordUrl
                            }
                            ActiveInputTarget::WebhookDiscordUrl => {
                                ActiveInputTarget::WebhookGenericUrl
                            }
                            _ => ActiveInputTarget::WebhookBarkUrl,
                        };
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.save_notification_webhooks();
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
            ActiveInputTarget::BootstrapParentDir
            | ActiveInputTarget::BootstrapGitCloneUrl
            | ActiveInputTarget::BootstrapFolderName => match key {
                "backspace" => {
                    if let Ok(mut g) = self.state.lock() {
                        let sel_all = g.input_selected_all;
                        g.input_selected_all = false;
                        g.bootstrap_modal.error = None;
                        let field = match target {
                            ActiveInputTarget::BootstrapParentDir => {
                                &mut g.bootstrap_modal.parent_dir
                            }
                            ActiveInputTarget::BootstrapGitCloneUrl => {
                                &mut g.bootstrap_modal.git_clone_url
                            }
                            _ => &mut g.bootstrap_modal.folder_name,
                        };
                        if sel_all {
                            field.clear();
                        } else {
                            field.pop();
                        }
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "tab" => {
                    if let Ok(mut g) = self.state.lock() {
                        g.input_selected_all = false;
                        g.input_target = match target {
                            ActiveInputTarget::BootstrapParentDir => {
                                ActiveInputTarget::BootstrapGitCloneUrl
                            }
                            ActiveInputTarget::BootstrapGitCloneUrl => {
                                ActiveInputTarget::BootstrapFolderName
                            }
                            _ => ActiveInputTarget::BootstrapParentDir,
                        };
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
                "enter" | "return" => {
                    self.submit_remote_bootstrap_workspace();
                    cx.notify();
                    cx.stop_propagation();
                }
                "escape" => {
                    if let Ok(mut g) = self.state.lock() {
                        if !g.bootstrap_modal.loading {
                            g.bootstrap_modal.reset();
                            g.input_selected_all = false;
                            g.input_target = ActiveInputTarget::TerminalDirect;
                        }
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
        let mut osc_updates: Vec<(String, Option<Osc7501Status>)> = Vec::new();
        for sess in &mut g.sessions {
            if let Some(snap) = snaps.get(&sess.id) {
                let prev = synced.get(&sess.id).copied();
                if prev == Some((snap.generation, snap.alive, snap.osc_seq)) {
                    continue;
                }
                let prev_osc_seq = prev.map(|p| p.2).unwrap_or(0);
                synced.insert(sess.id.clone(), (snap.generation, snap.alive, snap.osc_seq));
                let old_total = sess.styled_lines.len();
                let new_total = snap.styled_lines.len();
                // #41 + V3-31: Keep scrollback viewport pinned while reading history, and clamp if output shrinks.
                if snap.in_alt_screen {
                    sess.scroll_offset = 0;
                } else if sess.scroll_offset > 0 && new_total > old_total {
                    sess.scroll_offset = (sess.scroll_offset + (new_total - old_total))
                        .min(new_total.saturating_sub(1))
                        .min(1200);
                } else if sess.scroll_offset > 0 {
                    sess.scroll_offset = sess.scroll_offset.min(new_total.saturating_sub(1));
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
                // V3-10: Only propagate OSC 7501 when a genuinely new OSC sequence arrived (osc_seq changed),
                // preventing sticky NeedsYou state from resurrecting after approval or herdr_live_event.
                if snap.osc_seq != prev_osc_seq {
                    osc_updates.push((sess.id.clone(), snap.osc_status.clone()));
                }
            }
        }
        for (sid, osc_opt) in osc_updates {
            if let Some(osc) = osc_opt {
                g.record_session_osc_status(&sid, osc);
            } else if let Some(sess) = g.sessions.iter_mut().find(|s| s.id == sid) {
                sess.osc_status = None;
            }
        }
    }

    fn optimistically_clear_needs_you(&self, session_id: &str) {
        let should_clear = self
            .state
            .lock()
            .ok()
            .and_then(|g| {
                g.sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .and_then(|s| s.osc_status.as_ref())
                    .map(|o| o.state == AgentLifecycleState::NeedsYou)
            })
            .unwrap_or(false);
        if !should_clear {
            return;
        }
        let next = Osc7501Status {
            state: AgentLifecycleState::Working,
            task: None,
            updated_at: Instant::now(),
        };
        if let Ok(mgr) = self.pty_mgr.lock() {
            mgr.set_session_osc_status(session_id, Some(next.clone()));
        }
        if let Ok(mut g) = self.state.lock() {
            g.record_session_osc_status(session_id, next);
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn send_bytes_to_session(&self, session_id: &str, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.optimistically_clear_needs_you(session_id);
        }
        self.send_bytes_to_session_raw(session_id, bytes);
    }

    fn send_bytes_to_session_raw(&self, session_id: &str, bytes: &[u8]) {
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
        self.optimistically_clear_needs_you(session_id);
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

    pub(crate) fn trigger_refresh_git_diff(&self) {
        let remote_target = self
            .state
            .lock()
            .ok()
            .and_then(|g| g.effective_remote_diff_target());
        if let Some((device_id, cwd)) = remote_target {
            self.mesh_hub.request_remote_git_diff(device_id, cwd);
        } else {
            self.refresh_active_space_inspector();
        }
    }

    fn refresh_inspector_arcs(
        state: &Arc<Mutex<CodaDesktopState>>,
        mesh_hub: &Arc<RemoteMeshHub>,
        inspector_seq: &Arc<AtomicU64>,
        ui_dirty: &Arc<AtomicBool>,
    ) {
        let (
            space_id,
            space_path_str,
            remote_host_id,
            remote_diff_cwd,
            browse_rel,
            selected_change,
            selected_file_rel,
        ) = {
            let Ok(mut g) = state.lock() else {
                return;
            };
            if let Some((hid, eff_cwd)) = g.effective_remote_diff_target() {
                let (sp_id, sp_path) = g
                    .active_space()
                    .filter(|sp| sp.is_remote && Uuid::parse_str(&sp.machine_id).ok() == Some(hid))
                    .map(|sp| (sp.id.clone(), sp.path.clone()))
                    .unwrap_or_else(|| (String::new(), ".".to_string()));
                (
                    sp_id,
                    sp_path,
                    Some(hid),
                    eff_cwd,
                    g.current_browse_rel.clone(),
                    g.selected_change_path.as_ref().map(|p| format!("file:{p}")),
                    g.selected_file_rel.clone(),
                )
            } else {
                // Invalidate any in-flight remote GitDiffRequest so a late response cannot
                // clobber the local space or focused local split-pane snapshot.
                g.remote_diff_seq = g.remote_diff_seq.wrapping_add(1);
                g.git_diff_loading = false;

                let local_target = g
                    .focused_session_id()
                    .and_then(|sid| g.session_by_id(sid))
                    .filter(|s| s.remote_host_id.is_none())
                    .and_then(|s| {
                        g.spaces
                            .iter()
                            .find(|sp| !sp.is_remote && sp.id == s.space_id)
                            .map(|sp| (sp.id.clone(), sp.path.clone()))
                            .or_else(|| {
                                (!s.cwd.trim().is_empty() && s.cwd != ".")
                                    .then(|| (s.space_id.clone(), s.cwd.clone()))
                            })
                    })
                    .or_else(|| {
                        g.active_space()
                            .filter(|sp| !sp.is_remote)
                            .map(|sp| (sp.id.clone(), sp.path.clone()))
                    });
                let Some((sp_id, sp_path)) = local_target else {
                    return;
                };
                (
                    sp_id,
                    sp_path,
                    None,
                    String::new(),
                    g.current_browse_rel.clone(),
                    g.selected_change_path.as_ref().map(|p| format!("file:{p}")),
                    g.selected_file_rel.clone(),
                )
            }
        };

        if let Some(host_id) = remote_host_id {
            mesh_hub.request_remote_git_diff(host_id, remote_diff_cwd.clone());
            let dir_root = if space_path_str != "." && !space_path_str.is_empty() {
                &space_path_str
            } else {
                &remote_diff_cwd
            };
            if dir_root != "." && !space_id.is_empty() {
                mesh_hub.refresh_remote_directory(host_id, &space_id, dir_root, &browse_rel);
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
                let still_targeting_local_space = g.effective_remote_diff_target().is_none()
                    && (g.active_space_id.as_deref() == Some(space_id.as_str())
                        || g.focused_session_id()
                            .and_then(|sid| g.session_by_id(sid))
                            .map(|s| s.remote_host_id.is_none() && s.space_id == space_id)
                            .unwrap_or(false));
                if still_targeting_local_space {
                    g.repo_snapshot = snap;
                    if g.active_space_id.as_deref() == Some(space_id.as_str()) {
                        g.dir_entries = entries;
                        if let Some(preview) = file_preview {
                            g.selected_file_content = preview;
                        }
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
                                                    g.notify_tracker.seed_baseline(
                                                        &id,
                                                        AgentLifecycleState::Idle,
                                                    );
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
            g.notify_tracker
                .seed_baseline(&id, AgentLifecycleState::Idle);
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
        let (before_active, before_split) = self
            .state
            .lock()
            .ok()
            .map(|g| (g.active_session_id.clone(), g.split_session_id.clone()))
            .unwrap_or((None, None));
        self.spawn_terminal_in_active_space(Some(agent_cmd));
        if let Ok(mut g) = self.state.lock() {
            let spawned_id = if g.split_session_id != before_split {
                g.split_session_id.clone()
            } else if g.active_session_id != before_active {
                g.active_session_id.clone()
            } else {
                None
            };
            if let Some(id) = spawned_id {
                if let Some(sess) = g.sessions.iter_mut().find(|s| s.id == id) {
                    sess.title = format!("{space_name} · {agent_cmd}");
                }
            }
        }
    }

    fn select_session(&self, session_id: &str) {
        let Some((_space_changed, layout_changed)) = (match self.state.lock() {
            Ok(mut g) => g.apply_select_session(session_id),
            Err(_) => return,
        }) else {
            return;
        };
        if layout_changed {
            self.persist_layout_config();
        }
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
            g.remote_diff_seq = g.remote_diff_seq.wrapping_add(1);
            g.git_diff_loading = false;
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
        self.mesh_hub.stop_all_tunnels_quiet();
        if let Ok(mut g) = self.state.lock() {
            g.sessions.retain(|s| s.id != session_id);
            g.notify_tracker.prune_session(session_id);
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
        self.mesh_hub.stop_all_tunnels_quiet();

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

    pub(crate) fn add_workspace_folder(&self) {
        let remote_hid = self.state.lock().ok().and_then(|g| {
            g.active_space()
                .filter(|sp| sp.is_remote)
                .and_then(|sp| Uuid::parse_str(&sp.machine_id).ok())
                .or_else(|| Self::resolve_active_remote_host_id(&g))
        });
        if let Some(hid) = remote_hid {
            self.open_remote_bootstrap_modal(Some(hid));
        } else {
            self.add_local_workspace_folder();
        }
    }

    pub(crate) fn add_local_workspace_folder(&self) {
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

    pub(crate) fn open_remote_bootstrap_modal(&self, preferred_host: Option<Uuid>) {
        if let Ok(mut g) = self.state.lock() {
            let resolved_host = preferred_host
                .or_else(|| Self::resolve_active_remote_host_id(&g))
                .or_else(|| g.remote_hosts.iter().find(|h| h.is_online).map(|h| h.id))
                .or_else(|| g.remote_hosts.first().map(|h| h.id));
            g.palette_open = false;
            g.palette_query.clear();
            g.palette_selected = 0;
            g.settings_open = false;
            g.clear_settings_ephemeral_inputs();
            g.bootstrap_modal.open = true;
            g.bootstrap_modal.host_id = resolved_host;
            if g.bootstrap_modal.parent_dir.trim().is_empty() {
                g.bootstrap_modal.parent_dir = "~/workspace".to_string();
            }
            g.bootstrap_modal.folder_name.clear();
            g.bootstrap_modal.git_clone_url.clear();
            g.bootstrap_modal.loading = false;
            g.bootstrap_modal.error = None;
            g.input_selected_all = false;
            g.input_target = ActiveInputTarget::BootstrapParentDir;
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    pub(crate) fn submit_remote_bootstrap_workspace(&self) {
        let params = {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            if g.bootstrap_modal.loading {
                return;
            }
            let loc = g.locale;
            let Some(device_id) = g.bootstrap_modal.host_id else {
                g.bootstrap_modal.error = Some(
                    tr(
                        loc,
                        "Select an online remote host first.",
                        "请先选择一个在线远端主机。",
                    )
                    .to_string(),
                );
                self.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let parent_raw = g.bootstrap_modal.parent_dir.trim().to_string();
            let parent_dir = if parent_raw.is_empty() {
                "~/workspace".to_string()
            } else {
                parent_raw
            };
            let git_url_raw = g.bootstrap_modal.git_clone_url.trim().to_string();
            let git_clone_url = (!git_url_raw.is_empty()).then_some(git_url_raw);
            let folder_raw = g.bootstrap_modal.folder_name.trim().to_string();
            let eff_folder = if !folder_raw.is_empty() {
                folder_raw
            } else if let Some(ref url) = git_clone_url {
                crate::helpers::derive_folder_name_from_git_url(url).unwrap_or_default()
            } else {
                String::new()
            };
            if eff_folder.is_empty()
                || eff_folder == "."
                || eff_folder == ".."
                || eff_folder.contains('/')
                || eff_folder.contains('\\')
            {
                g.bootstrap_modal.error = Some(
                    tr(
                        loc,
                        "Enter a single folder name (no slashes) or a valid Git repository URL.",
                        "请输入合法的单层目录名称（不含斜杠）或有效的 Git 仓库地址。",
                    )
                    .to_string(),
                );
                self.ui_dirty.store(true, Ordering::Release);
                return;
            }
            g.bootstrap_modal.parent_dir = parent_dir.clone();
            g.bootstrap_modal.folder_name = eff_folder.clone();
            g.bootstrap_modal.loading = true;
            g.bootstrap_modal.error = None;
            g.input_selected_all = false;
            (device_id, parent_dir, eff_folder, git_clone_url)
        };
        self.mesh_hub
            .bootstrap_remote_workspace(params.0, params.1, params.2, params.3);
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn execute_palette_action(&self, action: PaletteAction) {
        if let Ok(mut g) = self.state.lock() {
            g.palette_open = false;
            g.palette_query.clear();
            g.palette_selected = 0;
        }
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
            PaletteAction::AddSpaceFolder => {
                self.add_workspace_folder();
                if self
                    .state
                    .lock()
                    .map(|g| g.bootstrap_modal.open)
                    .unwrap_or(false)
                {
                    return;
                }
            }
            PaletteAction::OpenRemoteBootstrapModal => {
                self.open_remote_bootstrap_modal(None);
                return;
            }
            PaletteAction::OpenDrawer(tab) => {
                if let Ok(mut g) = self.state.lock() {
                    g.right_drawer = Some(tab);
                }
                if tab == RightDrawerTab::Changes {
                    self.trigger_refresh_git_diff();
                } else if tab == RightDrawerTab::Files {
                    self.refresh_active_space_inspector();
                }
            }
            PaletteAction::OpenSettings => {
                let should_fetch = if let Ok(mut g) = self.state.lock() {
                    g.bootstrap_modal.open = false;
                    g.settings_open = true;
                    g.is_logged_in && !g.webhook_dirty && !g.webhook_loading
                } else {
                    false
                };
                if should_fetch {
                    self.reload_notification_webhooks();
                }
            }
            PaletteAction::ToggleLocale => self.toggle_locale(),
            PaletteAction::OpenTerminalSearch => {
                if let Ok(mut g) = self.state.lock() {
                    g.bootstrap_modal.open = false;
                    g.settings_open = false;
                    g.terminal_search_open = true;
                    g.input_selected_all = !g.terminal_search_query.is_empty();
                    g.input_target = ActiveInputTarget::TerminalSearch;
                }
                return;
            }
            PaletteAction::ExportActiveSession => self.export_focused_session_markdown(),
            PaletteAction::ToggleVoiceDictation => self.toggle_voice_recording(),
            PaletteAction::JumpNextNeedsYou => self.jump_to_next_needs_you_session(),
        }
        if let Ok(mut g) = self.state.lock() {
            if !g.bootstrap_modal.open {
                g.input_target = ActiveInputTarget::TerminalDirect;
            }
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
        let mut should_fetch_wh = false;
        if let Ok(mut g) = self.state.lock() {
            g.settings_open = !g.settings_open;
            if g.settings_open {
                g.palette_open = false;
                g.bootstrap_modal.open = false;
                should_fetch_wh = g.is_logged_in && !g.webhook_dirty && !g.webhook_loading;
            } else {
                g.clear_settings_ephemeral_inputs();
            }
            g.input_target = ActiveInputTarget::TerminalDirect;
        }
        if should_fetch_wh {
            self.reload_notification_webhooks();
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    pub fn jump_to_next_needs_you_session(&self) {
        let next_id = self.state.lock().ok().and_then(|mut g| {
            g.palette_open = false;
            g.settings_open = false;
            g.bootstrap_modal.open = false;
            g.clear_settings_ephemeral_inputs();
            let loc = g.locale;
            match g.next_needs_you_session_id() {
                Some(id) => Some(id),
                None => {
                    g.status_notice = Some(
                        tr(
                            loc,
                            "No sessions currently need your attention",
                            "当前没有等待确认的会话",
                        )
                        .to_string(),
                    );
                    None
                }
            }
        });
        if let Some(id) = next_id {
            self.select_session(&id);
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    pub(crate) fn reload_notification_webhooks(&self) {
        self.mesh_hub.fetch_notification_webhooks();
    }

    pub(crate) fn save_notification_webhooks(&self) {
        let outcome = match self.state.lock() {
            Ok(mut g) => prepare_webhook_save(&mut g),
            Err(_) => return,
        };
        match outcome {
            Ok(settings) => {
                self.mesh_hub.save_notification_webhooks(settings);
            }
            Err(should_reload) => {
                if should_reload {
                    self.reload_notification_webhooks();
                }
                self.ui_dirty.store(true, Ordering::Release);
            }
        }
    }

    pub(crate) fn test_notification_webhooks(&self) {
        self.mesh_hub.test_notification_webhooks();
    }

    pub(crate) fn prune_offline_web_devices(&self) {
        self.mesh_hub.prune_offline_web_devices();
    }

    fn resolve_active_remote_host_id(g: &CodaDesktopState) -> Option<Uuid> {
        g.effective_remote_diff_target().map(|(hid, _)| hid)
    }

    fn toggle_terminal_search(&self) {
        let should_jump = if let Ok(mut g) = self.state.lock() {
            if g.settings_open {
                return;
            }
            g.palette_open = false;
            if g.terminal_search_open && g.input_target == ActiveInputTarget::TerminalSearch {
                g.terminal_search_open = false;
                g.input_selected_all = false;
                g.input_target = ActiveInputTarget::TerminalDirect;
                false
            } else {
                g.terminal_search_open = true;
                g.input_selected_all = !g.terminal_search_query.is_empty();
                g.input_target = ActiveInputTarget::TerminalSearch;
                !g.terminal_search_query.trim().is_empty()
            }
        } else {
            false
        };
        if should_jump {
            self.jump_to_current_search_match();
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn step_terminal_search(&self, forward: bool) {
        if let Ok(mut g) = self.state.lock() {
            let Some(fid) = g.focused_session_id().map(|s| s.to_string()) else {
                return;
            };
            let matches = g
                .session_by_id(&fid)
                .map(|s| {
                    crate::search_export::find_terminal_matches(
                        &s.styled_lines,
                        &g.terminal_search_query,
                    )
                })
                .unwrap_or_default();
            if matches.is_empty() {
                g.terminal_search_index = 0;
                return;
            }
            let len = matches.len();
            let cur = g.terminal_search_index % len;
            g.terminal_search_index = if forward {
                (cur + 1) % len
            } else {
                (cur + len - 1) % len
            };
        }
        self.jump_to_current_search_match();
    }

    fn jump_to_current_search_match(&self) {
        let pane_rows = self
            .visible_rows_by_pane
            .lock()
            .ok()
            .and_then(|m| {
                let pane = self
                    .state
                    .lock()
                    .ok()
                    .map(|g| g.focused_pane)
                    .unwrap_or(SplitPane::Primary);
                m.get(&pane).copied()
            })
            .unwrap_or(36);

        if let Ok(mut g) = self.state.lock() {
            let Some(fid) = g.focused_session_id().map(|s| s.to_string()) else {
                return;
            };
            let query = g.terminal_search_query.clone();
            let idx = g.terminal_search_index;
            if let Some(sess) = g.sessions.iter_mut().find(|s| s.id == fid) {
                let matches =
                    crate::search_export::find_terminal_matches(&sess.styled_lines, &query);
                if let Some(m) = matches.get(idx % matches.len().max(1)) {
                    let total = sess.styled_lines.len();
                    let max_off = total.saturating_sub(pane_rows);
                    let desired_end = (m.line_idx + pane_rows / 2 + 1).min(total);
                    sess.scroll_offset = total.saturating_sub(desired_end).min(max_off);
                }
            }
        }
    }

    fn export_focused_session_markdown(&self) {
        let snapshot = self.state.lock().ok().and_then(|g| {
            let id = g.focused_session_id()?;
            let s = g.session_by_id(id)?;
            Some((g.locale, s.title.clone(), s.cwd.clone(), s.styled_lines.clone()))
        });
        let Some((loc, title, cwd, styled)) = snapshot else {
            return;
        };
        match crate::search_export::export_session_markdown_file(&title, &cwd, &styled) {
            Ok(path) => {
                if let Ok(mut g) = self.state.lock() {
                    g.status_notice = Some(format!(
                        "{}: {}",
                        tr(loc, "Exported session log", "已导出会话日志"),
                        display_compact_path(&path.to_string_lossy())
                    ));
                }
            }
            Err(err) => {
                if let Ok(mut g) = self.state.lock() {
                    g.status_notice = Some(format!(
                        "{}: {err}",
                        tr(loc, "Export failed", "导出失败")
                    ));
                }
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    pub(crate) fn trigger_active_port_preview(&self) {
        let parsed = {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            match crate::port_preview::parse_preview_port_and_path(
                &g.port_preview.port_input,
                &g.port_preview.path_input,
            ) {
                Ok((port, path)) => {
                    g.port_preview.loading = true;
                    g.port_preview.path_input = path.clone();
                    g.input_selected_all = false;
                    g.input_target = ActiveInputTarget::TerminalDirect;
                    let remote_hid = Self::resolve_active_remote_host_id(&g);
                    Some((port, path, remote_hid))
                }
                Err(err) => {
                    g.port_preview.loading = false;
                    g.port_preview.status_code = Some(400);
                    g.port_preview.content_type = "text/plain".to_string();
                    g.port_preview.body_preview = err;
                    None
                }
            }
        };
        let Some((port, path, remote_hid)) = parsed else {
            self.ui_dirty.store(true, Ordering::Release);
            return;
        };
        if let Some(hid) = remote_hid {
            self.mesh_hub
                .spawn_remote_port_preview_snapshot(hid, port, path);
        } else {
            let state_bg = Arc::clone(&self.state);
            let ui_dirty_bg = Arc::clone(&self.ui_dirty);
            std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                let (sc, ct, body) =
                    rt.block_on(crate::port_preview::fetch_local_port_preview(port, &path));
                if let Ok(mut g) = state_bg.lock() {
                    g.port_preview.loading = false;
                    g.port_preview.status_code = Some(sc);
                    g.port_preview.content_type = ct;
                    g.port_preview.body_preview = body;
                }
                ui_dirty_bg.store(true, Ordering::Release);
            });
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    pub(crate) fn open_live_port_tunnel(&self, open_in_browser: bool) {
        let parsed = {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            match crate::port_preview::parse_preview_port_and_path(
                &g.port_preview.port_input,
                &g.port_preview.path_input,
            ) {
                Ok((port, path)) => {
                    g.port_preview.path_input = path.clone();
                    g.input_selected_all = false;
                    g.input_target = ActiveInputTarget::TerminalDirect;
                    Some((Self::resolve_active_remote_host_id(&g), port, path))
                }
                Err(err) => {
                    g.status_notice = Some(err);
                    None
                }
            }
        };
        if let Some((device_id, remote_port, initial_path)) = parsed {
            self.mesh_hub.open_live_loopback_tunnel(
                device_id,
                remote_port,
                initial_path,
                open_in_browser,
            );
        } else {
            self.ui_dirty.store(true, Ordering::Release);
        }
    }

    pub(crate) fn stop_port_tunnel(&self, key_opt: Option<crate::port_preview::TunnelTargetKey>) {
        self.mesh_hub.stop_live_loopback_tunnel(key_opt);
    }

    pub(crate) fn save_voice_preferences(&self) {
        let (loc, base_url, model, api_key) = match self.state.lock() {
            Ok(g) => (
                g.locale,
                g.voice_base_url.trim().to_string(),
                g.voice_model.trim().to_string(),
                g.voice_api_key.trim().to_string(),
            ),
            Err(_) => return,
        };
        if let Err(err) = crate::voice_dock::validated_transcription_endpoint(&base_url) {
            if let Ok(mut g) = self.state.lock() {
                g.status_notice = Some(err);
            }
            self.ui_dirty.store(true, Ordering::Release);
            return;
        }
        let eff_url = if base_url.is_empty() {
            "https://api.openai.com/v1".to_string()
        } else {
            base_url
        };
        let eff_model = if model.is_empty() {
            "whisper-1".to_string()
        } else {
            model
        };
        if let Ok(mut g) = self.state.lock() {
            g.voice_base_url = eff_url.clone();
            g.voice_model = eff_model.clone();
            g.voice_api_key = api_key.clone();
            g.input_selected_all = false;
            g.input_target = ActiveInputTarget::TerminalDirect;
            g.status_notice = Some(
                tr(
                    loc,
                    "Saved Whisper voice configuration",
                    "已保存 Whisper 语音听写配置",
                )
                .to_string(),
            );
        }
        Self::update_prefs_async(move |prefs| {
            prefs.voice_base_url = eff_url;
            prefs.voice_model = eff_model;
            prefs.voice_api_key = api_key;
        });
        self.ui_dirty.store(true, Ordering::Release);
    }

    fn toggle_voice_recording(&self) {
        if self.voice_ctrl.is_transcribing.load(Ordering::Acquire) {
            return;
        }
        if self.voice_ctrl.is_recording.load(Ordering::Acquire) {
            let Some(audio_path) = self.voice_ctrl.stop_recording_to_path() else {
                return;
            };
            let (loc, base_url, model, api_key, target_sid) = match self.state.lock() {
                Ok(mut g) => {
                    g.voice_recording = false;
                    g.voice_transcribing = true;
                    g.status_notice = Some(
                        tr(g.locale, "Transcribing voice…", "正在进行 Whisper 语音转写…")
                            .to_string(),
                    );
                    (
                        g.locale,
                        g.voice_base_url.clone(),
                        g.voice_model.clone(),
                        g.voice_api_key.clone(),
                        g.focused_session_id().map(|s| s.to_string()),
                    )
                }
                Err(_) => return,
            };
            self.voice_ctrl.is_transcribing.store(true, Ordering::Release);
            let voice_ctrl_bg = Arc::clone(&self.voice_ctrl);
            let state_bg = Arc::clone(&self.state);
            let pty_mgr_bg = Arc::clone(&self.pty_mgr);
            let mesh_hub_bg = Arc::clone(&self.mesh_hub);
            let ui_dirty_bg = Arc::clone(&self.ui_dirty);
            std::thread::spawn(move || {
                let res = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())
                    .and_then(|rt| {
                        rt.block_on(crate::voice_dock::transcribe_audio_file(
                            &audio_path,
                            &base_url,
                            &api_key,
                            &model,
                        ))
                    });
                let _ = std::fs::remove_file(&audio_path);
                voice_ctrl_bg.is_transcribing.store(false, Ordering::Release);

                match res {
                    Ok(transcript) if !transcript.is_empty() => {
                        let sid = target_sid.or_else(|| {
                            state_bg
                                .lock()
                                .ok()
                                .and_then(|g| g.focused_session_id().map(|s| s.to_string()))
                        });
                        if let Some(id) = sid {
                            let bracketed = pty_mgr_bg
                                .lock()
                                .map(|m| m.terminal_modes(&id).1)
                                .unwrap_or(false);
                            let safe_txt =
                                crate::helpers::sanitize_bracketed_paste_text(&transcript);
                            let payload = if bracketed {
                                let mut b = Vec::with_capacity(safe_txt.len() + 12);
                                b.extend_from_slice(b"\x1b[200~");
                                b.extend_from_slice(safe_txt.as_bytes());
                                b.extend_from_slice(b"\x1b[201~");
                                b
                            } else {
                                safe_txt.into_bytes()
                            };
                            let remote_info = state_bg.lock().ok().and_then(|g| {
                                g.session_by_id(&id).and_then(|s| {
                                    Some((
                                        Uuid::parse_str(s.remote_host_id.as_deref()?).ok()?,
                                        s.hc_addr.clone()?,
                                    ))
                                })
                            });
                            if let Some((hid, addr)) = remote_info {
                                mesh_hub_bg.send_remote_pty_input(hid, &addr, payload);
                            } else if let Ok(mgr) = pty_mgr_bg.lock() {
                                let _ = mgr.write_input(&id, &payload);
                            }
                        }
                        if let Ok(mut g) = state_bg.lock() {
                            g.voice_transcribing = false;
                            g.status_notice = Some(
                                tr(
                                    loc,
                                    "Inserted voice transcript into terminal",
                                    "已将语音转写文本填入当前终端",
                                )
                                .to_string(),
                            );
                        }
                    }
                    Ok(_) => {
                        if let Ok(mut g) = state_bg.lock() {
                            g.voice_transcribing = false;
                            g.status_notice = Some(
                                tr(loc, "No speech detected", "未识别到有效语音内容").to_string(),
                            );
                        }
                    }
                    Err(err) => {
                        if let Ok(mut g) = state_bg.lock() {
                            g.voice_transcribing = false;
                            g.status_notice = Some(err);
                        }
                    }
                }
                ui_dirty_bg.store(true, Ordering::Release);
            });
            self.ui_dirty.store(true, Ordering::Release);
        } else {
            let (loc, base_url, api_key) = match self.state.lock() {
                Ok(g) => (g.locale, g.voice_base_url.clone(), g.voice_api_key.clone()),
                Err(_) => return,
            };
            match crate::voice_dock::resolve_voice_dictation_mode(&base_url, &api_key) {
                Err(err) => {
                    if let Ok(mut g) = self.state.lock() {
                        g.voice_recording = false;
                        g.status_notice = Some(err);
                    }
                    self.ui_dirty.store(true, Ordering::Release);
                    return;
                }
                Ok(crate::voice_dock::VoiceDictationMode::MacosSystemDictation) => {
                    if let Ok(mut g) = self.state.lock() {
                        g.palette_open = false;
                        g.settings_open = false;
                        g.clear_settings_ephemeral_inputs();
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                    let invoked = crate::voice_dock::trigger_macos_system_dictation();
                    if let Ok(mut g) = self.state.lock() {
                        g.voice_recording = false;
                        g.status_notice = Some(
                            crate::voice_dock::macos_dictation_fallback_notice(
                                invoked,
                                loc == UiLocale::Zh,
                            )
                            .to_string(),
                        );
                    }
                    self.ui_dirty.store(true, Ordering::Release);
                    return;
                }
                Ok(crate::voice_dock::VoiceDictationMode::MissingApiKey) => {
                    if let Ok(mut g) = self.state.lock() {
                        g.voice_recording = false;
                        g.status_notice = Some(
                            tr(
                                loc,
                                "Configure a Whisper API Key or http://127.0.0.1 endpoint in Settings",
                                "请在设置中填写 Whisper API Key 或本地 127.0.0.1 地址",
                            )
                            .to_string(),
                        );
                    }
                    self.ui_dirty.store(true, Ordering::Release);
                    return;
                }
                Ok(crate::voice_dock::VoiceDictationMode::WhisperApi { .. }) => {}
            }

            match self.voice_ctrl.start_recording() {
                Ok(()) => {
                    if let Ok(mut g) = self.state.lock() {
                        g.voice_recording = true;
                        g.status_notice = Some(
                            tr(
                                g.locale,
                                "Recording voice… click Stop to transcribe",
                                "正在录音… 再次点击停止并转写到终端",
                            )
                            .to_string(),
                        );
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = self.state.lock() {
                        g.voice_recording = false;
                        g.status_notice = Some(err);
                    }
                }
            }
            self.ui_dirty.store(true, Ordering::Release);
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let win_active = window.is_window_active();
        let st = match self.state.lock() {
            Ok(mut g) => {
                g.app_is_active = win_active;
                g.clone()
            }
            Err(_) => CodaDesktopState::default(),
        };

        let needs_you_count = st
            .sessions
            .iter()
            .filter(|s| s.alive && s.effective_state() == AgentLifecycleState::NeedsYou)
            .count();
        crate::voice_dock::update_macos_dock_badge(needs_you_count);

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
            && !st.bootstrap_modal.open
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
            .on_action(cx.listener(|this, _: &ToggleSettings, _, cx| {
                this.toggle_settings_modal();
                cx.notify();
            }))
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
                        g.bootstrap_modal.open = false;
                        g.clear_settings_ephemeral_inputs();
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
                            let last_col: usize = sess
                                .styled_lines
                                .get(last_line)
                                .map(|l| {
                                    l.text
                                        .chars()
                                        .map(|ch| crate::pty::char_display_width(ch).max(1))
                                        .sum()
                                })
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
            .on_action(cx.listener(|this, _: &ToggleTerminalSearch, _, cx| {
                this.toggle_terminal_search();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExportActiveSession, _, cx| {
                this.export_focused_session_markdown();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleVoiceDictation, _, cx| {
                this.toggle_voice_recording();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &JumpNextNeedsYou, _, cx| {
                this.jump_to_next_needs_you_session();
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
            .when(st.bootstrap_modal.open, |el| {
                el.child(self.render_remote_bootstrap_modal_overlay(&st, cx))
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

        let needs_you_sessions: Vec<&TerminalSessionEntry> = st
            .sessions
            .iter()
            .filter(|s| s.alive && s.effective_state() == AgentLifecycleState::NeedsYou)
            .collect();

        // Group Spaces by machine_id / machine_label preserving discovery order,
        // and include online remote hosts even before their first workspace is created.
        let mut machine_groups: Vec<(String, String, bool, Vec<&SpaceEntry>)> = Vec::new();
        for sp in &st.spaces {
            if let Some(grp) = machine_groups.iter_mut().find(|g| {
                if sp.is_remote {
                    g.0 == sp.machine_id
                } else {
                    !g.2 && g.1 == sp.machine_label
                }
            }) {
                grp.3.push(sp);
            } else {
                machine_groups.push((
                    sp.machine_id.clone(),
                    sp.machine_label.clone(),
                    sp.is_remote,
                    vec![sp],
                ));
            }
        }
        for host in &st.remote_hosts {
            if !host.is_online {
                continue;
            }
            let hid_str = host.id.to_string();
            if !machine_groups.iter().any(|g| g.2 && g.0 == hid_str) {
                machine_groups.push((hid_str, host.name.clone(), true, Vec::new()));
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
                    .when(!needs_you_sessions.is_empty(), |el| {
                        let count = needs_you_sessions.len();
                        el.child(
                            div()
                                .id("sidebar-needs-attention-group")
                                .p(px(7.0))
                                .rounded(px(7.0))
                                .bg(rgba(0xf59e0b1a))
                                .border_1()
                                .border_color(rgba(0xf59e0b66))
                                .flex()
                                .flex_col()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(5.0))
                                                .child(
                                                    div()
                                                        .w(px(6.0))
                                                        .h(px(6.0))
                                                        .rounded(px(99.0))
                                                        .bg(rgb(palette::NEEDS_YOU_AMBER)),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.5))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(palette::NEEDS_YOU_AMBER))
                                                        .child(format!(
                                                            "{} ({count})",
                                                            tr(
                                                                loc,
                                                                "NEEDS ATTENTION",
                                                                "待你确认 · NEEDS ATTENTION"
                                                            )
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id("btn-sidebar-jump-needs-you")
                                                .occlude()
                                                .px(px(5.0))
                                                .py(px(1.0))
                                                .rounded(px(4.0))
                                                .bg(rgba(0xf59e0b2e))
                                                .border_1()
                                                .border_color(rgba(0xf59e0b77))
                                                .cursor_pointer()
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(palette::NEEDS_YOU_AMBER))
                                                .child("⌘J")
                                                .on_click(cx.listener(
                                                    |this, _: &ClickEvent, _, cx| {
                                                        this.jump_to_next_needs_you_session();
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                )
                                .children(needs_you_sessions.into_iter().map(|sess| {
                                    let sid = sess.id.clone();
                                    let is_focused = st.focused_session_id() == Some(sess.id.as_str());
                                    let sp_meta = st
                                        .spaces
                                        .iter()
                                        .find(|s| s.id == sess.space_id)
                                        .map(|s| format!("{} · {}", s.machine_label, s.name))
                                        .unwrap_or_else(|| "local".to_string());
                                    div()
                                        .id(SharedString::from(format!("needs-attn-row-{}", sess.id)))
                                        .occlude()
                                        .px(px(7.0))
                                        .py(px(5.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(if is_focused {
                                            palette::HOVER_BG
                                        } else {
                                            palette::SURFACE_BG
                                        }))
                                        .border_1()
                                        .border_color(rgb(if is_focused {
                                            palette::NEEDS_YOU_AMBER
                                        } else {
                                            palette::BORDER
                                        }))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                                .truncate()
                                                .child(sess.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(rgb(palette::NEEDS_YOU_AMBER))
                                                .truncate()
                                                .child(format!(
                                                    "{sp_meta} · {}",
                                                    sess.preview_snippet()
                                                )),
                                        )
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.select_session(&sid);
                                            cx.notify();
                                        }))
                                })),
                        )
                    })
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
                            .children(machine_groups.into_iter().map(
                                |(machine_id, machine, is_remote, list)| {
                                    let host_uuid_opt = is_remote
                                        .then(|| Uuid::parse_str(&machine_id).ok())
                                        .flatten();
                                    let add_btn_id = if is_remote {
                                        format!("btn-add-remote-space-{machine_id}")
                                    } else {
                                        "btn-add-local-space".to_string()
                                    };
                                    let is_empty_remote = is_remote && list.is_empty();
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .px(px(6.0))
                                                .pt(px(4.0))
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap(px(4.0))
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .text_size(px(10.5))
                                                        .text_color(rgb(palette::TEXT_DIM))
                                                        .truncate()
                                                        .child(if is_remote {
                                                            machine
                                                        } else {
                                                            format!(
                                                                "{} · {machine}",
                                                                tr(loc, "This Mac", "本机")
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .id(SharedString::from(add_btn_id))
                                                        .occlude()
                                                        .flex_shrink_0()
                                                        .px(px(4.0))
                                                        .py(px(0.5))
                                                        .rounded(px(3.0))
                                                        .hover(|h| {
                                                            h.bg(rgb(palette::HOVER_BG))
                                                                .text_color(rgb(
                                                                    palette::TEXT_PRIMARY,
                                                                ))
                                                        })
                                                        .cursor_pointer()
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(palette::TEXT_MUTED))
                                                        .child("+")
                                                        .on_click(cx.listener(
                                                            move |this, _: &ClickEvent, _, cx| {
                                                                if is_remote {
                                                                    this.open_remote_bootstrap_modal(
                                                                        host_uuid_opt,
                                                                    );
                                                                } else {
                                                                    this.add_local_workspace_folder();
                                                                }
                                                                cx.notify();
                                                            },
                                                        )),
                                                ),
                                        )
                                        .when(is_empty_remote, |grp| {
                                            grp.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "empty-remote-bootstrap-{machine_id}"
                                                    )))
                                                    .occlude()
                                                    .px(px(8.0))
                                                    .py(px(4.0))
                                                    .rounded(px(5.0))
                                                    .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                                    .cursor_pointer()
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(accent))
                                                    .child(tr(
                                                        loc,
                                                        "+ Create folder / Git clone…",
                                                        "+ 新建目录 / Git 克隆…",
                                                    ))
                                                    .on_click(cx.listener(
                                                        move |this, _: &ClickEvent, _, cx| {
                                                            this.open_remote_bootstrap_modal(
                                                                host_uuid_opt,
                                                            );
                                                            cx.notify();
                                                        },
                                                    )),
                                            )
                                        })
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
                                },
                            )),
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
                    )
                    .child({
                        let v_rec = st.voice_recording;
                        let v_tx = st.voice_transcribing;
                        let v_label = if v_rec {
                            tr(loc, "● Stop", "● 停止")
                        } else if v_tx {
                            tr(loc, "🎙 …", "🎙 转写…")
                        } else {
                            tr(loc, "🎙 Voice", "🎙 语音")
                        };
                        div()
                            .id("btn-voice-dictation")
                            .occlude()
                            .h(px(22.0))
                            .px(px(7.0))
                            .rounded(px(5.0))
                            .bg(rgb(if v_rec {
                                palette::HOVER_BG
                            } else {
                                palette::SURFACE_BG
                            }))
                            .border_1()
                            .border_color(rgb(if v_rec {
                                palette::ERROR_RED
                            } else if v_tx {
                                accent
                            } else {
                                palette::BORDER
                            }))
                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(10.5))
                            .text_color(rgb(if v_rec {
                                palette::ERROR_RED
                            } else if v_tx {
                                accent
                            } else {
                                palette::TEXT_MUTED
                            }))
                            .child(v_label)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.toggle_voice_recording();
                                cx.notify();
                            }))
                    }),
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
                        RightDrawerTab::Preview,
                        tr(loc, "Port", "端口"),
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
                if opened {
                    if tab == RightDrawerTab::Changes {
                        this.trigger_refresh_git_diff();
                    } else if tab == RightDrawerTab::Files {
                        this.refresh_active_space_inspector();
                    }
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
            && !st.bootstrap_modal.open
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
        let needs_you_active = sess.alive
            && sess
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

        let show_search_bar = st.terminal_search_open && is_focused;
        let search_matches = if show_search_bar && !st.terminal_search_query.trim().is_empty() {
            crate::search_export::find_terminal_matches(
                &sess.styled_lines,
                &st.terminal_search_query,
            )
        } else {
            Vec::new()
        };
        let active_match_idx = if search_matches.is_empty() {
            None
        } else {
            Some(st.terminal_search_index % search_matches.len())
        };
        let search_focused = st.input_target == ActiveInputTarget::TerminalSearch;
        let search_count_label = if st.terminal_search_query.trim().is_empty() {
            "0/0".to_string()
        } else if let Some(mi) = active_match_idx {
            format!("{}/{}", mi + 1, search_matches.len())
        } else {
            "0/0".to_string()
        };

        div()
            .id(SharedString::from(format!("term-pane-{pane_slot:?}-{}", sess.id)))
            .size_full()
            .bg(rgb(palette::CANVAS_BG))
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _, cx| {
                // V3-11: terminal_modes returns (in_alt_screen, bracketed_paste, app_cursor_keys).
                let (in_alt, _, app_cursor) = this
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
                    this.send_bytes_to_session_raw(&sid_scroll, &payload);
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
                            if ev.modifiers.platform {
                                let clicked_url = this.state.lock().ok().and_then(|g| {
                                    let line = g
                                        .session_by_id(&sid_sel_down)?
                                        .styled_lines
                                        .get(abs_line)?;
                                    crate::search_export::url_at_display_col(&line.text, col)
                                });
                                if let Some(url) = clicked_url {
                                    if crate::search_export::open_external_url_safely(&url) {
                                        if let Ok(mut g) = this.state.lock() {
                                            g.status_notice = Some(format!(
                                                "{}: {url}",
                                                tr(g.locale, "Opened URL", "已打开链接")
                                            ));
                                        }
                                    }
                                    cx.notify();
                                    return;
                                }
                            }
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
            .when(show_search_bar, |el| {
                let q_display = if st.terminal_search_query.is_empty() {
                    tr(loc, "Find in terminal…", "在终端滚动缓存中搜索…").to_string()
                } else {
                    format!(
                        "{}{}",
                        st.terminal_search_query,
                        if search_focused { "▏" } else { "" }
                    )
                };
                el.child(
                    div()
                        .id(SharedString::from(format!("term-search-bar-{pane_slot:?}")))
                        .occlude()
                        .flex_shrink_0()
                        .px(px(12.0))
                        .py(px(5.0))
                        .bg(rgb(palette::CHROME_BG))
                        .border_b_1()
                        .border_color(rgb(palette::BORDER))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "term-search-input-{pane_slot:?}"
                                )))
                                .occlude()
                                .flex_1()
                                .min_w_0()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(palette::SURFACE_BG))
                                .border_1()
                                .border_color(rgb(if search_focused {
                                    accent
                                } else {
                                    palette::BORDER
                                }))
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .text_color(rgb(if st.terminal_search_query.is_empty() {
                                    palette::TEXT_DIM
                                } else {
                                    palette::TEXT_PRIMARY
                                }))
                                .truncate()
                                .child(format!("🔍 {q_display}"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Ok(mut g) = this.state.lock() {
                                        g.input_selected_all = false;
                                        g.input_target = ActiveInputTarget::TerminalSearch;
                                    }
                                    cx.notify();
                                })),
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
                                        .child(search_count_label),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "term-search-prev-{pane_slot:?}"
                                        )))
                                        .occlude()
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
                                        .child("↑")
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.step_terminal_search(false);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "term-search-next-{pane_slot:?}"
                                        )))
                                        .occlude()
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
                                        .child("↓")
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.step_terminal_search(true);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "term-export-md-{pane_slot:?}"
                                        )))
                                        .occlude()
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .border_1()
                                        .border_color(rgb(palette::BORDER))
                                        .hover(|h| {
                                            h.border_color(rgb(accent))
                                                .text_color(rgb(palette::TEXT_PRIMARY))
                                        })
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .text_color(rgb(palette::TEXT_MUTED))
                                        .child(tr(loc, "Export .md", "导出 .md"))
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.export_focused_session_markdown();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "term-search-close-{pane_slot:?}"
                                        )))
                                        .occlude()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(palette::TEXT_DIM))
                                        .child("×")
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            if let Ok(mut g) = this.state.lock() {
                                                g.terminal_search_open = false;
                                                g.input_target = ActiveInputTarget::TerminalDirect;
                                            }
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
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
                            let line_cols: usize = line
                                .text
                                .chars()
                                .map(|ch| crate::pty::char_display_width(ch).max(1))
                                .sum();
                            let s_col = if abs_idx == r0 { c0 } else { 0 };
                            let e_col = if abs_idx == r1 { c1 } else { line_cols };
                            (s_col < e_col).then_some((s_col, e_col))
                        });
                        let line_matches: Vec<(usize, usize, bool)> = search_matches
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| m.line_idx == abs_idx)
                            .map(|(mi, m)| (m.byte_start, m.byte_end, Some(mi) == active_match_idx))
                            .collect();
                        render_styled_terminal_line(line, line_h, sel_cols, &line_matches)
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
                let is_active = st.focused_session_id() == Some(sess.id.as_str())
                    || st.active_session_id.as_deref() == Some(&sess.id);
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
                                    .map(|line| {
                                        render_styled_terminal_line(line, tile_line_h, None, &[])
                                    })
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
            RightDrawerTab::Preview => (
                tr(loc, "Port Preview", "端口预览").to_string(),
                Some(format!("127.0.0.1:{}", st.port_preview.port_input)),
            ),
            RightDrawerTab::Mesh => (
                tr(loc, "Remote Nodes", "远端节点").to_string(),
                Some(format!("{online_hosts}/{}", st.remote_hosts.len())),
            ),
        };

        let show_back = match tab {
            RightDrawerTab::Files => st.selected_file_rel.is_some(),
            RightDrawerTab::Changes => st.selected_change_path.is_some(),
            RightDrawerTab::Preview | RightDrawerTab::Mesh => false,
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
                                                RightDrawerTab::Preview | RightDrawerTab::Mesh => {}
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
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            if tab == RightDrawerTab::Preview {
                                                this.trigger_active_port_preview();
                                            } else if tab == RightDrawerTab::Changes {
                                                this.trigger_refresh_git_diff();
                                            } else {
                                                this.refresh_active_space_inspector();
                                            }
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
                        let is_remote_diff = Self::resolve_active_remote_host_id(st).is_some();
                        let branch_badge = if snap.branch.is_empty() {
                            "HEAD".to_string()
                        } else {
                            snap.branch.clone()
                        };
                        let el = el.child(
                            div()
                                .p(px(8.0))
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
                                                .px(px(6.0))
                                                .py(px(2.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(palette::CHROME_BG))
                                                .font_family("Menlo")
                                                .text_size(px(10.5))
                                                .text_color(rgb(accent))
                                                .truncate()
                                                .child(format!("⎇ {branch_badge}")),
                                        )
                                        .when(snap.is_git_repo, |row| {
                                            row.child(
                                                div()
                                                    .font_family("Menlo")
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(palette::DONE_GREEN))
                                                    .child(format!("+{}", snap.diff_added)),
                                            )
                                            .child(
                                                div()
                                                    .font_family("Menlo")
                                                    .text_size(px(10.5))
                                                    .text_color(rgb(palette::ERROR_RED))
                                                    .child(format!("-{}", snap.diff_deleted)),
                                            )
                                        })
                                        .when(is_remote_diff, |row| {
                                            row.child(
                                                div()
                                                    .px(px(5.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgba(0x38bdf822))
                                                    .text_size(px(9.5))
                                                    .text_color(rgb(palette::WORKING_SKY))
                                                    .child("E2EE"),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .id("btn-refresh-git-diff")
                                        .occlude()
                                        .flex_shrink_0()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(palette::CHROME_BG))
                                        .border_1()
                                        .border_color(rgb(if st.git_diff_loading {
                                            accent
                                        } else {
                                            palette::BORDER_STRONG
                                        }))
                                        .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                        .cursor_pointer()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(palette::TEXT_PRIMARY))
                                        .child(if st.git_diff_loading {
                                            tr(loc, "Refreshing…", "刷新中…")
                                        } else {
                                            tr(loc, "↻ Refresh Diff", "↻ 刷新 Diff")
                                        })
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.trigger_refresh_git_diff();
                                            cx.notify();
                                        })),
                                ),
                        );

                        let el = el.when_some(snap.repo_root.as_ref(), |el, root_str| {
                            el.child(
                                div()
                                    .px(px(6.0))
                                    .font_family("Menlo")
                                    .text_size(px(10.0))
                                    .text_color(rgb(palette::TEXT_DIM))
                                    .truncate()
                                    .child(display_compact_path(root_str)),
                            )
                        });

                        if st.git_diff_loading && snap.changed_files.is_empty() && !snap.is_git_repo && snap.error.is_none() {
                            el.child(
                                div()
                                    .p(px(10.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .text_size(px(11.5))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(tr(
                                        loc,
                                        "Fetching Git diff over E2EE…",
                                        "正在通过 E2EE 获取远端 Git Diff…",
                                    )),
                            )
                        } else if let Some(ref err_msg) = snap.error {
                            el.child(
                                div()
                                    .p(px(10.0))
                                    .rounded(px(6.0))
                                    .bg(rgba(0xf871711a))
                                    .border_1()
                                    .border_color(rgb(palette::ERROR_RED))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette::ERROR_RED))
                                            .child(tr(
                                                loc,
                                                "Git status / diff error",
                                                "Git 状态 / Diff 读取失败",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .font_family("Menlo")
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child(err_msg.clone()),
                                    ),
                            )
                        } else if !snap.is_git_repo {
                            let detail = snap.diff_text.trim();
                            el.child(
                                div()
                                    .p(px(10.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(palette::SURFACE_BG))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(palette::TEXT_MUTED))
                                            .child(tr(
                                                loc,
                                                "Not a Git repository",
                                                "当前目录不是 Git 仓库",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(rgb(palette::TEXT_DIM))
                                            .child(if detail.is_empty() {
                                                tr(
                                                    loc,
                                                    "Open a Git repository or click Refresh Diff after `git init`.",
                                                    "切换到 Git 仓库或在执行 `git init` 后点击刷新 Diff。",
                                                )
                                                .to_string()
                                            } else {
                                                detail.to_string()
                                            }),
                                    ),
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
                            let all_selected = st.selected_change_path.is_none();
                            let total_files = snap.changed_files.len();
                            let diff_lines: Vec<String> = extract_file_diff_lines(
                                &snap.diff_text,
                                st.selected_change_path.as_deref(),
                            )
                            .into_iter()
                            .map(str::to_string)
                            .collect();

                            el.child(
                                // Clickable file filter pills (`All` + each `status_short` file)
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(5.0))
                                    .child(
                                        div()
                                            .id("change-pill-all")
                                            .occlude()
                                            .px(px(7.0))
                                            .py(px(3.0))
                                            .rounded(px(99.0))
                                            .bg(rgb(if all_selected {
                                                palette::HOVER_BG
                                            } else {
                                                palette::SURFACE_BG
                                            }))
                                            .border_1()
                                            .border_color(rgb(if all_selected {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .cursor_pointer()
                                            .font_family("Menlo")
                                            .text_size(px(10.5))
                                            .text_color(rgb(if all_selected {
                                                palette::TEXT_PRIMARY
                                            } else {
                                                palette::TEXT_MUTED
                                            }))
                                            .child(format!(
                                                "{} ({total_files})",
                                                tr(loc, "All", "全部")
                                            ))
                                            .on_click(cx.listener(
                                                move |this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.selected_change_path = None;
                                                    }
                                                    if !is_remote_diff {
                                                        this.refresh_active_space_inspector();
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                    )
                                    .children(snap.changed_files.iter().take(48).map(|f| {
                                        let path_rel = f.path.clone();
                                        let is_sel = st.selected_change_path.as_deref()
                                            == Some(f.path.as_str());
                                        let badge_color = match f.status.as_str() {
                                            "A" | "??" => palette::DONE_GREEN,
                                            "D" => palette::ERROR_RED,
                                            "R" | "C" => palette::WORKING_SKY,
                                            _ => palette::NEEDS_YOU_AMBER,
                                        };
                                        let short_label = Path::new(&f.path)
                                            .file_name()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or(&f.path)
                                            .to_string();
                                        div()
                                            .id(SharedString::from(format!(
                                                "change-file-{}",
                                                f.path
                                            )))
                                            .occlude()
                                            .px(px(7.0))
                                            .py(px(3.0))
                                            .rounded(px(99.0))
                                            .bg(rgb(if is_sel {
                                                palette::HOVER_BG
                                            } else {
                                                palette::SURFACE_BG
                                            }))
                                            .border_1()
                                            .border_color(rgb(if is_sel {
                                                accent
                                            } else {
                                                palette::BORDER
                                            }))
                                            .hover(|h| h.bg(rgb(palette::HOVER_BG)))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .gap(px(5.0))
                                            .font_family("Menlo")
                                            .text_size(px(10.5))
                                            .child(
                                                div()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(badge_color))
                                                    .child(f.status.clone()),
                                            )
                                            .child(
                                                div()
                                                    .max_w(px(210.0))
                                                    .truncate()
                                                    .text_color(rgb(if is_sel {
                                                        palette::TEXT_PRIMARY
                                                    } else {
                                                        palette::TEXT_MUTED
                                                    }))
                                                    .child(short_label),
                                            )
                                            .on_click(cx.listener(
                                                move |this, _: &ClickEvent, _, cx| {
                                                    if let Ok(mut g) = this.state.lock() {
                                                        g.selected_change_path =
                                                            if g.selected_change_path.as_deref()
                                                                == Some(path_rel.as_str())
                                                            {
                                                                None
                                                            } else {
                                                                Some(path_rel.clone())
                                                            };
                                                    }
                                                    if !is_remote_diff {
                                                        this.refresh_active_space_inspector();
                                                    }
                                                    cx.notify();
                                                },
                                            ))
                                    })),
                            )
                            .child(div().h(px(1.0)).w_full().bg(rgb(palette::BORDER)))
                            .when(diff_lines.is_empty(), |d| {
                                d.child(
                                    div()
                                        .p(px(8.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(palette::SURFACE_BG))
                                        .text_size(px(11.0))
                                        .text_color(rgb(palette::TEXT_DIM))
                                        .child(tr(
                                            loc,
                                            "Untracked or binary file — open in Files tab to preview full contents",
                                            "未跟踪或二进制文件 — 可在“文件”侧栏查看内容",
                                        )),
                                )
                            })
                            .when(!diff_lines.is_empty(), |d| {
                                d.child(
                                    div()
                                        .p(px(8.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(palette::CANVAS_BG))
                                        .border_1()
                                        .border_color(rgb(palette::BORDER))
                                        .font_family("Menlo")
                                        .text_size(px(11.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(1.5))
                                        .children(diff_lines.into_iter().map(|line| {
                                            let kind = classify_diff_line(&line);
                                            let (fg, bg_opt, is_bold) = match kind {
                                                DiffLineKind::Header => {
                                                    (palette::TEXT_PRIMARY, None, true)
                                                }
                                                DiffLineKind::Hunk => {
                                                    (0x22d3ee, Some(0x22d3ee14), false)
                                                }
                                                DiffLineKind::Addition => {
                                                    (palette::DONE_GREEN, Some(0x22c55e14), false)
                                                }
                                                DiffLineKind::Deletion => {
                                                    (palette::ERROR_RED, Some(0xf8717114), false)
                                                }
                                                DiffLineKind::Context => {
                                                    (palette::TEXT_MUTED, None, false)
                                                }
                                            };
                                            div()
                                                .px(px(4.0))
                                                .rounded(px(2.0))
                                                .when_some(bg_opt, |row, bg_rgba| {
                                                    row.bg(rgba(bg_rgba))
                                                })
                                                .when(is_bold, |row| {
                                                    row.font_weight(FontWeight::BOLD)
                                                })
                                                .text_color(rgb(fg))
                                                .truncate()
                                                .child(if line.is_empty() {
                                                    " ".to_string()
                                                } else {
                                                    line
                                                })
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
                    .when(tab == RightDrawerTab::Preview, |el| {
                        self.render_port_preview_drawer_body(st, el, cx)
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
            let ota_host = host.clone();
            let can_ota = host.is_online
                && host
                    .health
                    .as_ref()
                    .map(|h| h.ota_supported)
                    .unwrap_or(true);
            let meta_line = match host.health.as_ref() {
                Some(hh) => crate::search_export::format_host_health_compact(hh),
                None => format!("{} · {}", host.platform, host.role),
            };
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
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgb(palette::TEXT_DIM))
                                .truncate()
                                .child(meta_line),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .when(can_ota, |el| {
                            el.child(
                                div()
                                    .id(SharedString::from(format!("mesh-ota-{}", host.id)))
                                    .occlude()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(palette::CHROME_BG))
                                    .border_1()
                                    .border_color(rgb(palette::BORDER))
                                    .hover(|h| h.border_color(rgb(st.layout.accent_color_u32())))
                                    .text_size(px(10.0))
                                    .text_color(rgb(palette::TEXT_MUTED))
                                    .child(tr(loc, "Upgrade", "升级"))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.mesh_hub.upgrade_remote_daemon(ota_host.clone());
                                        cx.notify();
                                    })),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(st.layout.accent_color_u32()))
                                .child(tr(loc, "Open", "进入")),
                        ),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.mesh_hub.probe_host(host_clone.clone());
                    cx.notify();
                }))
        }))
    }

}


fn render_styled_terminal_line(
    line: &StyledTerminalLine,
    line_height_px: f32,
    selection_cols: Option<(usize, usize)>,
    search_matches: &[(usize, usize, bool)],
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
            // V3-30: Map display columns (width 2 for CJK/Emoji, 1 for narrow) to UTF-8 byte bounds
            // so highlight aligns identically with `extract_selected_terminal_text`.
            let mut col = 0usize;
            let mut b_start: Option<usize> = None;
            let mut b_end: usize = 0;
            for (byte_idx, ch) in line.text.char_indices() {
                let w = crate::pty::char_display_width(ch).max(1);
                if col + w > col_start && col < col_end {
                    if b_start.is_none() {
                        b_start = Some(byte_idx);
                    }
                    b_end = byte_idx + ch.len_utf8();
                }
                col += w;
            }
            if let Some(bs) = b_start {
                if bs < b_end {
                    highlights.push((
                        bs..b_end,
                        HighlightStyle {
                            background_color: Some(Hsla::from(rgba(0x38bdf855))),
                            ..Default::default()
                        },
                    ));
                }
            }
        }
    }

    for &(bs, be, is_active) in search_matches {
        if bs < be && be <= line.text.len() {
            highlights.push((
                bs..be,
                HighlightStyle {
                    color: is_active.then(|| Hsla::from(rgb(0x09090b))),
                    background_color: Some(Hsla::from(if is_active {
                        rgba(0xf59e0bee)
                    } else {
                        rgba(0xf59e0b55)
                    })),
                    font_weight: is_active.then_some(FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            ));
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
            g.notify_tracker
                .seed_baseline(&id, AgentLifecycleState::Idle);
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
                    let initial_state = osc
                        .as_ref()
                        .map(|o| o.state)
                        .unwrap_or(AgentLifecycleState::Idle);
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
                    g.notify_tracker.seed_baseline(&id, initial_state);
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

pub(crate) fn prepare_webhook_save(
    g: &mut CodaDesktopState,
) -> Result<NotificationWebhookSettings, bool> {
    g.input_selected_all = false;
    g.input_target = ActiveInputTarget::TerminalDirect;
    let loc = g.locale;
    if !g.is_logged_in {
        g.status_notice = Some(
            tr(
                loc,
                "Sign in first to configure notification webhooks",
                "请先登录账号后再保存推送配置",
            )
            .to_string(),
        );
        return Err(false);
    }
    if g.webhook_loading || !g.webhook_loaded_ok {
        let should_reload = !g.webhook_loading;
        g.status_notice = Some(
            tr(
                loc,
                "Cloud webhook configuration has not finished loading; reloading first so saved URLs are not overwritten.",
                "云端现有 Webhook 配置尚未加载成功，已阻止盲写覆盖并正在重新拉取。",
            )
            .to_string(),
        );
        return Err(should_reload);
    }
    g.webhook_settings.bark_url = g.webhook_settings.bark_url.trim().to_string();
    g.webhook_settings.telegram_bot_token =
        g.webhook_settings.telegram_bot_token.trim().to_string();
    g.webhook_settings.telegram_chat_id = g.webhook_settings.telegram_chat_id.trim().to_string();
    g.webhook_settings.feishu_webhook_url =
        g.webhook_settings.feishu_webhook_url.trim().to_string();
    g.webhook_settings.discord_webhook_url =
        g.webhook_settings.discord_webhook_url.trim().to_string();
    g.webhook_settings.generic_webhook_url =
        g.webhook_settings.generic_webhook_url.trim().to_string();
    Ok(g.webhook_settings.clone())
}

fn spawn_herdr_discovery(
    weak_state: std::sync::Weak<Mutex<CodaDesktopState>>,
    weak_pty: std::sync::Weak<Mutex<LocalPtyManager>>,
    weak_dirty: std::sync::Weak<AtomicBool>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            if *shutdown_rx.borrow() {
                return;
            }
            if weak_state.strong_count() == 0 || weak_pty.strong_count() == 0 {
                return;
            }

            let hc_bin = match discover_hc_binary() {
                Some(p) => p,
                None => match ensure_or_install_hc_binary().await {
                    Ok(p) => p,
                    Err(_) => {
                        if !*shutdown_rx.borrow() {
                            if let (Some(state_bg), Some(pty_mgr), Some(ui_dirty)) = (
                                weak_state.upgrade(),
                                weak_pty.upgrade(),
                                weak_dirty.upgrade(),
                            ) {
                                spawn_local_fallback_shell(&state_bg, &pty_mgr, &ui_dirty);
                            }
                        }
                        return;
                    }
                },
            };

            if *shutdown_rx.borrow() {
                return;
            }

            let Ok(hc) = HerdrClient::connect_with_bin(hc_bin.clone()).await else {
                if !*shutdown_rx.borrow() {
                    if let (Some(state_bg), Some(pty_mgr), Some(ui_dirty)) = (
                        weak_state.upgrade(),
                        weak_pty.upgrade(),
                        weak_dirty.upgrade(),
                    ) {
                        spawn_local_fallback_shell(&state_bg, &pty_mgr, &ui_dirty);
                    }
                }
                return;
            };
            let ver = hc.info().version.clone();
            {
                let (Some(state_bg), Some(pty_mgr), Some(ui_dirty)) = (
                    weak_state.upgrade(),
                    weak_pty.upgrade(),
                    weak_dirty.upgrade(),
                ) else {
                    return;
                };
                if let Ok(mut g) = state_bg.lock() {
                    g.hc_available = true;
                    g.hc_version = Some(ver);
                    g.hc_path = Some(hc_bin.clone());
                }
                reconcile_local_herdr_sessions(&hc, &hc_bin, &state_bg, &pty_mgr, &ui_dirty).await;
            }

            // Continuously subscribe to `herdr-core` `events.subscribe` so terminals created or
            // updated by iOS/iPadOS on this Mac appear & attach live in GPUI Desktop.
            // Hold only `Weak` references across waits and abort immediately on window close.
            loop {
                if *shutdown_rx.borrow()
                    || weak_state.strong_count() == 0
                    || weak_pty.strong_count() == 0
                {
                    break;
                }
                if let Ok(mut ev_rx) = hc.subscribe_live_events().await {
                    loop {
                        let ev_opt = tokio::select! {
                            _ = shutdown_rx.changed() => {
                                if *shutdown_rx.borrow() {
                                    return;
                                }
                                continue;
                            }
                            ev = ev_rx.recv() => ev,
                        };
                        let Some(ev) = ev_opt else {
                            break;
                        };
                        let (Some(state_bg), Some(pty_mgr), Some(ui_dirty)) = (
                            weak_state.upgrade(),
                            weak_pty.upgrade(),
                            weak_dirty.upgrade(),
                        ) else {
                            return;
                        };
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
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
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
            if let Ok(mut g) = this.state.lock() {
                g.app_is_active = win.is_window_active();
            }
            cx.observe_window_activation(win, |this, win, _cx| {
                if let Ok(mut g) = this.state.lock() {
                    g.app_is_active = win.is_window_active();
                }
            })
            .detach();
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
            KeyBinding::new("cmd-f", ToggleTerminalSearch, None),
            KeyBinding::new("cmd-j", JumpNextNeedsYou, None),
            KeyBinding::new("cmd-shift-e", ExportActiveSession, None),
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
                    MenuItem::action("Find in Terminal…", ToggleTerminalSearch),
                    MenuItem::action("Select All", SelectAllAction),
                ],
            },
            Menu {
                name: "Shell".into(),
                items: vec![
                    MenuItem::action("New Window", NewWindow),
                    MenuItem::action("New Terminal", NewTerminal),
                    MenuItem::action("Toggle Split Terminals", ToggleSplit),
                    MenuItem::action("Export Session Log (.md)", ExportActiveSession),
                    MenuItem::action("Voice Dictation (Start / Stop)", ToggleVoiceDictation),
                    MenuItem::separator(),
                    MenuItem::action("Close Terminal", CloseActiveTerminal),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Command Palette…", ToggleCommandPalette),
                    MenuItem::action("Jump to Next Needs-You Session", JumpNextNeedsYou),
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

        let _initial_view = open_main_workbench_window(cx);

        // V3-15: Active windows handle `ToggleSettings` directly in `CodaGpuWorkspaceView::render`.
        // This app-level fallback only triggers when all windows are closed (Dock / menu bar reopen).
        cx.on_action(move |_: &ToggleSettings, cx| {
            if cx.windows().is_empty() {
                let reopened = open_main_workbench_window(cx);
                reopened.update(cx, |this, cx| {
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

        state.palette_query = "agent".to_string();
        let agent_items = state.palette_items();
        for expected in ["claude", "codex", "grok", "agy", "gemini"] {
            assert!(
                agent_items
                    .iter()
                    .any(|item| matches!(item.action, PaletteAction::LaunchAgent(cmd) if cmd == expected)),
                "'+ Agent' Command Palette filter must include LaunchAgent({expected:?})"
            );
        }

        state.palette_query = "search".to_string();
        assert!(
            state
                .palette_items()
                .iter()
                .any(|item| matches!(item.action, PaletteAction::OpenTerminalSearch)),
            "Command Palette must expose OpenTerminalSearch"
        );

        state.palette_query = "export".to_string();
        assert!(
            state
                .palette_items()
                .iter()
                .any(|item| matches!(item.action, PaletteAction::ExportActiveSession)),
            "Command Palette must expose ExportActiveSession"
        );

        state.palette_query = "needs-you".to_string();
        assert!(
            state
                .palette_items()
                .iter()
                .any(|item| matches!(item.action, PaletteAction::JumpNextNeedsYou)),
            "Command Palette must expose JumpNextNeedsYou"
        );
    }

    #[test]
    fn test_needs_you_attention_inbox_cycling() {
        let mut st = CodaDesktopState::default();
        assert_eq!(st.next_needs_you_session_id(), None);

        let now = Instant::now();
        let make_sess = |id: &str, state: AgentLifecycleState, alive: bool| TerminalSessionEntry {
            id: id.to_string(),
            title: id.to_string(),
            space_id: "sp-1".to_string(),
            cwd: "/tmp".to_string(),
            started_at: now,
            last_output_at: now,
            last_byte_sig: 0,
            lines: Vec::new(),
            styled_lines: Vec::new(),
            scroll_offset: 0,
            alive,
            attached_hc: true,
            hc_addr: Some(id.to_string()),
            remote_host_id: None,
            osc_status: Some(Osc7501Status {
                state,
                task: Some("Approve command".to_string()),
                updated_at: now,
            }),
        };

        st.sessions.push(make_sess("s1-working", AgentLifecycleState::Working, true));
        st.sessions.push(make_sess("s2-needs", AgentLifecycleState::NeedsYou, true));
        st.sessions.push(make_sess("s3-dead", AgentLifecycleState::NeedsYou, false));
        st.sessions.push(make_sess("s4-remote-needs", AgentLifecycleState::NeedsYou, true));

        st.active_session_id = Some("s1-working".to_string());
        assert_eq!(st.next_needs_you_session_id().as_deref(), Some("s2-needs"));

        st.active_session_id = Some("s2-needs".to_string());
        assert_eq!(
            st.next_needs_you_session_id().as_deref(),
            Some("s4-remote-needs")
        );

        st.active_session_id = Some("s4-remote-needs".to_string());
        assert_eq!(st.next_needs_you_session_id().as_deref(), Some("s2-needs"));
    }

    #[tokio::test]
    async fn test_webhook_notifications_and_prune_stale_rest_endpoints() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}");

        tokio::spawn(async move {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(
                    req.contains("Authorization: Bearer test-session-tok")
                        || req.contains("authorization: Bearer test-session-tok"),
                    "missing bearer token: {req}"
                );
                let body_json = if req.starts_with("GET /api/v1/me/notifications ") {
                    r#"{"bark_url":"https://api.day.app/k1","telegram_bot_token":"123:ABC","telegram_chat_id":"999","feishu_webhook_url":"","discord_webhook_url":"","generic_webhook_url":"","notify_on_needs_you":true,"notify_on_finished":false}"#
                } else if req.starts_with("PUT /api/v1/me/notifications ") {
                    assert!(req.contains("discord.com/api/webhooks/test"));
                    assert!(req.contains("hooks.example.com/coda"));
                    r#"{"bark_url":"https://api.day.app/k1","telegram_bot_token":"123:ABC","telegram_chat_id":"999","feishu_webhook_url":"https://open.feishu.cn/hook","discord_webhook_url":"https://discord.com/api/webhooks/test","generic_webhook_url":"https://hooks.example.com/coda","notify_on_needs_you":true,"notify_on_finished":true}"#
                } else if req.starts_with("POST /api/v1/me/notifications/test ") {
                    r#"{"ok":true,"delivered_channels":["bark","discord"],"errors":[]}"#
                } else if req.starts_with("POST /api/v1/devices/prune-stale ") {
                    assert!(req.contains("\"max_idle_days\":0"));
                    assert!(
                        req.contains("\"exclude_device_id\":\"11111111-2222-3333-4444-555555555555\""),
                        "expected exclude_device_id in prune body: {req}"
                    );
                    r#"{"ok":true,"pruned_count":4}"#
                } else {
                    r#"{"error":"unexpected route"}"#
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body_json.len(),
                    body_json
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });

        let loaded = crate::remote_mesh::api_get_notification_webhooks(&base_url, "test-session-tok")
            .await
            .unwrap();
        assert_eq!(loaded.bark_url, "https://api.day.app/k1");
        assert_eq!(loaded.telegram_chat_id, "999");
        assert!(!loaded.notify_on_finished);

        let mut updated_req = loaded.clone();
        updated_req.feishu_webhook_url = "https://open.feishu.cn/hook".to_string();
        updated_req.discord_webhook_url = "https://discord.com/api/webhooks/test".to_string();
        updated_req.generic_webhook_url = "https://hooks.example.com/coda".to_string();
        updated_req.notify_on_finished = true;
        let saved = crate::remote_mesh::api_put_notification_webhooks(
            &base_url,
            "test-session-tok",
            &updated_req,
        )
        .await
        .unwrap();
        assert_eq!(saved.discord_webhook_url, "https://discord.com/api/webhooks/test");
        assert_eq!(saved.generic_webhook_url, "https://hooks.example.com/coda");
        assert!(saved.notify_on_finished);

        let (delivered, errs) =
            crate::remote_mesh::api_test_notification_webhooks(&base_url, "test-session-tok")
                .await
                .unwrap();
        assert_eq!(delivered, 2);
        assert!(errs.is_empty());

        let exclude_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let pruned = crate::remote_mesh::api_prune_stale_web_devices(
            &base_url,
            "test-session-tok",
            0,
            Some(exclude_id),
        )
        .await
        .unwrap();
        assert_eq!(pruned, 4);
    }

    #[test]
    fn test_cmd_j_circular_scan_split_pane_smart_focus_and_grid_auto_switch() {
        let mut st = CodaDesktopState::default();
        let now = Instant::now();
        let make_sess = |id: &str, state: AgentLifecycleState| TerminalSessionEntry {
            id: id.to_string(),
            title: id.to_string(),
            space_id: "sp-1".to_string(),
            cwd: "/tmp".to_string(),
            started_at: now,
            last_output_at: now,
            last_byte_sig: 0,
            lines: Vec::new(),
            styled_lines: Vec::new(),
            scroll_offset: 0,
            alive: true,
            attached_hc: true,
            hc_addr: Some(id.to_string()),
            remote_host_id: None,
            osc_status: Some(Osc7501Status {
                state,
                task: Some("Approve".to_string()),
                updated_at: now,
            }),
        };

        // [S1(NeedsYou), S2(Working after user approved it), S3(NeedsYou)]:
        // Pressing Cmd+J while S2 is focused must advance circularly to S3, NOT jump backwards to S1!
        st.sessions.push(make_sess("s1", AgentLifecycleState::NeedsYou));
        st.sessions.push(make_sess("s2", AgentLifecycleState::Working));
        st.sessions.push(make_sess("s3", AgentLifecycleState::NeedsYou));
        st.active_session_id = Some("s2".to_string());
        assert_eq!(
            st.next_needs_you_session_id().as_deref(),
            Some("s3"),
            "Cmd+J from approved S2 must advance forward to S3 before wrapping to S1"
        );

        // Split-pane smart focus: if S3 is already open in Secondary while Primary is focused on S1,
        // selecting S3 switches focused_pane to Secondary instead of duplicating S3 in both panes.
        st.layout.split_terminals = true;
        st.focused_pane = SplitPane::Primary;
        st.active_session_id = Some("s1".to_string());
        st.split_session_id = Some("s3".to_string());
        let (space_changed, layout_changed) = st
            .apply_select_session("s3")
            .expect("s3 should be selectable");
        assert!(space_changed);
        assert!(!layout_changed);
        assert_eq!(st.focused_pane, SplitPane::Secondary);
        assert_eq!(st.active_session_id.as_deref(), Some("s1"));
        assert_eq!(st.split_session_id.as_deref(), Some("s3"));

        // Grid mode auto-switch: selecting session at index >= 4 in LayoutMode::Terminals switches to Workbench.
        st.layout.split_terminals = false;
        st.layout.layout = LayoutMode::Terminals;
        st.sessions.push(make_sess("s4", AgentLifecycleState::Idle));
        st.sessions.push(make_sess("s5", AgentLifecycleState::NeedsYou));
        let (same_space_changed, switched_layout) = st
            .apply_select_session("s5")
            .expect("s5 should be selectable");
        assert!(!same_space_changed);
        assert!(switched_layout);
        assert_eq!(st.layout.layout, LayoutMode::Workbench);
        assert_eq!(st.active_session_id.as_deref(), Some("s5"));
    }

    #[test]
    fn test_remote_git_diff_monotonic_seq_cwd_and_split_local_guard() {
        let mut st = CodaDesktopState::default();
        let host_a = Uuid::new_v4();
        let host_b = Uuid::new_v4();
        st.spaces.push(SpaceEntry {
            id: format!("remote:{host_a}:/srv/app"),
            name: "app".to_string(),
            path: "/srv/app".to_string(),
            machine_id: host_a.to_string(),
            machine_label: "linux-a".to_string(),
            is_remote: true,
            branch: "main".to_string(),
            dirty_files: 0,
            ahead: 0,
            behind: 0,
            avatar_color: 0x38bdf8,
        });
        st.active_space_id = Some(format!("remote:{host_a}:/srv/app"));
        st.remote_diff_seq = 5;
        st.git_diff_loading = true;

        let make_payload = |cwd: &str, is_git: bool, err: Option<&str>| {
            crate::remote_mesh::RemoteGitDiffPayload {
                is_git_repo: is_git,
                repo_root: is_git.then(|| cwd.to_string()),
                branch: "feat/v1".to_string(),
                status_short: "M  src/main.rs\n".to_string(),
                diff_unified: "+new line\n-old line\n".to_string(),
                error: err.map(str::to_string),
            }
        };

        // Stale seq (4 != 5) must be ignored
        assert!(!crate::remote_mesh::apply_remote_git_diff_payload_to_state(
            &mut st,
            4,
            host_a,
            None,
            "/srv/app",
            make_payload("/srv/app", true, None)
        ));
        assert!(st.git_diff_loading);

        // Wrong host or wrong cwd must be ignored
        assert!(!crate::remote_mesh::apply_remote_git_diff_payload_to_state(
            &mut st,
            5,
            host_b,
            None,
            "/srv/app",
            make_payload("/srv/app", true, None)
        ));
        assert!(!crate::remote_mesh::apply_remote_git_diff_payload_to_state(
            &mut st,
            5,
            host_a,
            None,
            "/srv/other",
            make_payload("/srv/other", true, None)
        ));

        // Matching (seq=5, host_a, "/srv/app") applies cleanly
        assert!(crate::remote_mesh::apply_remote_git_diff_payload_to_state(
            &mut st,
            5,
            host_a,
            None,
            "/srv/app",
            make_payload("/srv/app", true, None)
        ));
        assert!(!st.git_diff_loading);
        assert!(st.repo_snapshot.is_git_repo);
        assert_eq!(st.repo_snapshot.branch, "feat/v1");
        assert_eq!(st.repo_snapshot.repo_root.as_deref(), Some("/srv/app"));
        assert_eq!(st.repo_snapshot.changed_files.len(), 1);

        // If split-pane Secondary is focused on a LOCAL session while active_space is remote,
        // effective_remote_diff_target() is None and remote responses cannot clobber local snapshot!
        let now = Instant::now();
        st.sessions.push(TerminalSessionEntry {
            id: "local-1".to_string(),
            title: "local".to_string(),
            space_id: "local:/Users/dev/local-repo".to_string(),
            cwd: "/Users/dev/local-repo".to_string(),
            started_at: now,
            last_output_at: now,
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
        st.layout.split_terminals = true;
        st.focused_pane = SplitPane::Secondary;
        st.split_session_id = Some("local-1".to_string());
        st.repo_snapshot.branch = "local-branch".to_string();
        st.remote_diff_seq = 6;
        assert_eq!(st.effective_remote_diff_target(), None);
        assert!(!crate::remote_mesh::apply_remote_git_diff_payload_to_state(
            &mut st,
            6,
            host_a,
            None,
            "/srv/app",
            make_payload("/srv/app", true, None)
        ));
        assert_eq!(st.repo_snapshot.branch, "local-branch");

        // Non-git / error reporting when focused back on remote
        st.layout.split_terminals = false;
        st.focused_pane = SplitPane::Primary;
        st.remote_diff_seq = 7;
        assert!(crate::remote_mesh::apply_remote_git_diff_payload_to_state(
            &mut st,
            7,
            host_a,
            None,
            "/srv/app",
            make_payload("/srv/app", false, Some("fatal: dubious ownership"))
        ));
        assert!(!st.repo_snapshot.is_git_repo);
        assert_eq!(
            st.repo_snapshot.error.as_deref(),
            Some("fatal: dubious ownership")
        );
    }

    #[test]
    fn test_webhook_load_save_guard_prevents_blind_overwrite() {
        let mut st = CodaDesktopState::default();
        assert_eq!(prepare_webhook_save(&mut st), Err(false));

        st.is_logged_in = true;
        st.webhook_loaded_ok = false;
        st.webhook_loading = false;
        // Must block PUT and request reload (Err(true)) so server URLs are not wiped
        assert_eq!(prepare_webhook_save(&mut st), Err(true));
        assert!(st.status_notice.is_some());

        st.webhook_loading = true;
        assert_eq!(prepare_webhook_save(&mut st), Err(false));

        st.webhook_loading = false;
        st.webhook_loaded_ok = true;
        st.webhook_settings.generic_webhook_url = "  https://hooks.example.org/notify  ".to_string();
        let dto = prepare_webhook_save(&mut st).expect("loaded webhook settings should allow save");
        assert_eq!(dto.generic_webhook_url, "https://hooks.example.org/notify");
    }

    #[test]
    fn test_remote_mesh_hub_weak_tasks_and_clean_drop() {
        struct OverrideReset;
        impl Drop for OverrideReset {
            fn drop(&mut self) {
                set_config_root_override(None);
            }
        }
        let tmp_dir = std::env::temp_dir().join(format!("coda-hub-drop-test-{}", Uuid::new_v4()));
        set_config_root_override(Some(tmp_dir.clone()));
        let _reset = OverrideReset;

        let state = Arc::new(Mutex::new(CodaDesktopState::default()));
        let dirty = Arc::new(AtomicBool::new(false));
        let pty_mgr = Arc::new(Mutex::new(LocalPtyManager::new(Arc::clone(&dirty))));
        let hub = RemoteMeshHub::new(state, pty_mgr, dirty);
        hub.start_account_sync();

        // Background tasks inside RemoteMeshHub MUST hold Weak<RemoteMeshHub>, never strong Arcs!
        assert_eq!(
            Arc::strong_count(&hub),
            1,
            "RemoteMeshHub background tasks must hold Weak<RemoteMeshHub> so Drop is reachable on window close"
        );
        let weak_hub = Arc::downgrade(&hub);
        hub.shutdown();
        drop(hub);
        assert!(
            weak_hub.upgrade().is_none(),
            "dropping view's Arc<RemoteMeshHub> must immediately drop RemoteMeshHub"
        );
        let _ = std::fs::remove_dir_all(&tmp_dir);
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
