//! Runtime environment normalization, persisted desktop preferences, i18n strings,
//! and real local Git repository discovery for `coda-desktop`.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};

static CONFIG_DIR_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);
static ATOMIC_WRITE_SEQ: AtomicU64 = AtomicU64::new(1);
static ATOMIC_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Sets or clears an explicit configuration root directory override (used by `--self-test` and unit tests).
pub fn set_config_root_override(dir: Option<PathBuf>) {
    if let Ok(mut g) = CONFIG_DIR_OVERRIDE.write() {
        *g = dir;
    }
}

/// Writes `bytes` atomically to `path` with explicit Unix file permissions (`mode`), `sync_all`, and rename.
pub fn write_file_atomic_mode(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    let _guard = ATOMIC_WRITE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("atomic write lock poisoned"))?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("target path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // #8: Always enforce owner-only 0700 on ~/.config/coda so private keys & sessions are never group/world listable.
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    let seq = ATOMIC_WRITE_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("coda.tmp");
    let tmp_path = parent.join(format!(".{file_name}.tmp.{pid}.{seq}"));

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    let write_res: anyhow::Result<()> = (|| {
        let mut f = opts.open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    Ok(())
}

/// Supported UI locales for the Coda desktop workbench. Defaults to English (`En`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UiLocale {
    #[default]
    En,
    Zh,
}

impl UiLocale {
    pub fn toggle(self) -> Self {
        match self {
            UiLocale::En => UiLocale::Zh,
            UiLocale::Zh => UiLocale::En,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            UiLocale::En => "EN",
            UiLocale::Zh => "中文",
        }
    }

    pub fn label(self) -> &'static str {
        self.code()
    }
}

/// Returns `~/.config/coda` (or the test/self-test override directory if set).
pub fn config_root_dir() -> Option<PathBuf> {
    if let Ok(g) = CONFIG_DIR_OVERRIDE.read() {
        if let Some(dir) = g.as_ref() {
            return Some(dir.clone());
        }
    }
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".config/coda"))
}

/// Returns the localized string matching `locale`.
pub fn tr<'a>(locale: UiLocale, en: &'a str, zh: &'a str) -> &'a str {
    match locale {
        UiLocale::En => en,
        UiLocale::Zh => zh,
    }
}

/// Computes an augmented `PATH` string suitable for child processes launched from a macOS
/// `.app` bundle (where launchd only passes `/usr/bin:/bin:/usr/sbin:/sbin`), covering Homebrew,
/// Cargo, Bun, NVM, Volta, FNM, PNPM, Mise, ASDF, Nix, Pyenv, and Go without unsafe `set_var`.
pub fn augmented_gui_path_string() -> String {
    coda_herdr::build_augmented_path()
}

/// Retained for API compatibility; child spawns use `augmented_gui_path_string()` directly so no
/// unsafe global `std::env::set_var("PATH", ...)` mutation occurs after threads start.
pub fn ensure_macos_gui_path() {
    let _ = augmented_gui_path_string();
}

/// Returns true when executing inside a packaged macOS `.app` bundle.
pub fn is_macos_app_bundle() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Persists a sensitive token/secret in the macOS Keychain when running inside `Coda.app` (`#8`).
pub fn save_secret_in_keychain(account: &str, secret: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        if !is_macos_app_bundle() {
            return false;
        }
        std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                "dev.coda.desktop",
                "-a",
                account,
                "-w",
                secret,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (account, secret);
        false
    }
}

/// Loads a sensitive token/secret from the macOS Keychain when running inside `Coda.app` (`#8`).
pub fn load_secret_from_keychain(account: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if !is_macos_app_bundle() {
            return None;
        }
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "dev.coda.desktop",
                "-a",
                account,
                "-w",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        None
    }
}

/// Deletes a sensitive token/secret from the macOS Keychain (`#8`).
pub fn delete_secret_from_keychain(account: &str) {
    #[cfg(target_os = "macos")]
    {
        if !is_macos_app_bundle() {
            return;
        }
        let _ = std::process::Command::new("/usr/bin/security")
            .args([
                "delete-generic-password",
                "-s",
                "dev.coda.desktop",
                "-a",
                account,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
    }
}

fn default_allow_remote_control() -> bool {
    true
}

fn default_desktop_notifications() -> bool {
    true
}

fn default_voice_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_voice_model() -> String {
    "whisper-1".to_string()
}

/// Persisted user desktop preferences stored at `~/.config/coda/desktop.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopPreferences {
    #[serde(default)]
    pub locale: UiLocale,
    #[serde(default = "default_server_url")]
    pub server_url: String,
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    #[serde(default)]
    pub selected_repo_path: Option<String>,
    #[serde(default)]
    pub dismissed_spaces: Vec<String>,
    /// #8 / #31: Explicit user toggle controlling whether this Mac accepts incoming remote control.
    #[serde(default = "default_allow_remote_control")]
    pub allow_remote_control: bool,
    /// Native macOS OSC 7501 agent notifications (`needs_you`, `error`, background `done`).
    #[serde(default = "default_desktop_notifications")]
    pub desktop_notifications: bool,
    #[serde(default = "default_voice_base_url")]
    pub voice_base_url: String,
    #[serde(default = "default_voice_model")]
    pub voice_model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub voice_api_key: String,
}

fn default_server_url() -> String {
    std::env::var("CODA_SERVER_URL").unwrap_or_else(|_| "https://codahub.dev".into())
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            locale: UiLocale::En,
            server_url: default_server_url(),
            workspace_roots: Vec::new(),
            selected_repo_path: None,
            dismissed_spaces: Vec::new(),
            allow_remote_control: true,
            desktop_notifications: true,
            voice_base_url: default_voice_base_url(),
            voice_model: default_voice_model(),
            voice_api_key: String::new(),
        }
    }
}

impl DesktopPreferences {
    pub fn default_path() -> PathBuf {
        config_root_dir()
            .unwrap_or_else(|| PathBuf::from(".config/coda"))
            .join("desktop.json")
    }

    pub fn load_or_default() -> Self {
        let path = Self::default_path();
        let mut prefs = std::fs::read_to_string(&path)
            .ok()
            .and_then(|txt| serde_json::from_str::<Self>(&txt).ok())
            .unwrap_or_default();
        if let Some(kc_key) = load_secret_from_keychain("desktop-voice-api-key") {
            prefs.voice_api_key = kc_key;
        }
        prefs
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::default_path();
        let mut copy = self.clone();
        if !copy.voice_api_key.is_empty() && save_secret_in_keychain("desktop-voice-api-key", &copy.voice_api_key) {
            copy.voice_api_key.clear();
        } else if copy.voice_api_key.is_empty() {
            delete_secret_from_keychain("desktop-voice-api-key");
        }
        let body = serde_json::to_string_pretty(&copy)?;
        write_file_atomic_mode(&path, body.as_bytes(), 0o600)?;
        Ok(())
    }
}

/// Detects a real, friendly local host name without hardcoding fixture names.
pub fn detect_local_machine_name() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("scutil")
            .args(["--get", "LocalHostName"])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").arg("-s").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "Local-Mac".into()
}

/// Validates and returns existing Git repository roots from `saved_roots`, or performs
/// a bounded depth-2 discovery across common developer folders under `$HOME` on first run.
pub fn discover_local_git_repositories(saved_roots: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for raw in saved_roots {
        let p = PathBuf::from(raw.trim());
        if p.is_dir() {
            let canonical = p
                .canonicalize()
                .unwrap_or(p)
                .to_string_lossy()
                .to_string();
            if seen.insert(canonical.clone()) {
                found.push(canonical);
            }
        }
    }

    // Also include current_dir if it is a genuine user workspace inside $HOME (not `/` or `$HOME`).
    let home_opt = std::env::var("HOME").ok().map(PathBuf::from);
    if let Ok(cwd) = std::env::current_dir() {
        let is_root = cwd == Path::new("/");
        let is_home = home_opt.as_deref() == Some(cwd.as_path());
        if !is_root && !is_home && cwd.join(".git").exists() {
            let s = cwd
                .canonicalize()
                .unwrap_or(cwd)
                .to_string_lossy()
                .to_string();
            if seen.insert(s.clone()) {
                found.push(s);
            }
        }
    }

    // Only auto-scan $HOME developer directories when user has no saved workspaces yet.
    if found.is_empty() {
        if let Ok(cwd) = std::env::current_dir() {
            if cwd.join(".git").exists() {
                let s = cwd
                    .canonicalize()
                    .unwrap_or(cwd)
                    .to_string_lossy()
                    .to_string();
                if seen.insert(s.clone()) {
                    found.push(s);
                }
            }
        }
        if let Some(home) = home_opt {
            let roots_to_scan = [
                home.join("personal/github"),
                home.join("Developer"),
                home.join("Projects"),
                home.join("Code"),
                home.join("workspace"),
                home.join("github"),
                home.join("personal"),
                home.join("repos"),
                home.join("src"),
            ];
            for base in roots_to_scan {
                if !base.is_dir() {
                    continue;
                }
                scan_dir_for_git(&base, 0, 2, &mut seen, &mut found, 6);
                if found.len() >= 6 {
                    break;
                }
            }
        }
    }

    found.sort();
    found
}

fn scan_dir_for_git(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
    limit: usize,
) {
    if out.len() >= limit {
        return;
    }
    if dir.join(".git").exists() {
        let canonical = dir
            .canonicalize()
            .unwrap_or_else(|_| dir.to_path_buf())
            .to_string_lossy()
            .to_string();
        if seen.insert(canonical.clone()) {
            out.push(canonical);
        }
        return;
    }
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with('.')
                && name_str != "node_modules"
                && name_str != "target"
                && p.is_dir()
            {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    subdirs.sort();
    for sub in subdirs {
        scan_dir_for_git(&sub, depth + 1, max_depth, seen, out, limit);
        if out.len() >= limit {
            break;
        }
    }
}

/// Opens a native folder selection dialog (`choose folder` via AppleScript on macOS)
/// and returns the selected directory path, or `None` if canceled.
pub fn pick_folder_via_native_dialog() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("osascript")
            .args([
                "-e",
                r#"POSIX path of (choose folder with prompt "Select a Git Repository or Project Folder")"#,
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&out.stdout)
            .trim()
            .trim_end_matches('/')
            .to_string();
        if path.is_empty() {
            None
        } else {
            Some(if path.starts_with('/') {
                path
            } else {
                format!("/{path}")
            })
        }
    }
    #[cfg(all(not(target_os = "macos"), target_os = "linux"))]
    {
        let out = std::process::Command::new("zenity")
            .args(["--file-selection", "--directory", "--title=Select Project Folder"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() { None } else { Some(path) }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Formats an absolute filesystem path cleanly using `~` for `$HOME` when possible.
pub fn display_compact_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home_trimmed = home.trim_end_matches('/');
        if !home_trimmed.is_empty() {
            if path == home_trimmed {
                return "~".into();
            }
            if let Some(rest) = path.strip_prefix(home_trimmed) {
                if rest.starts_with('/') {
                    return format!("~{rest}");
                }
            }
        }
    }
    path.to_string()
}

/// Opens an existing canonicalized local directory or file in macOS Finder (`open <path>`) / Linux file manager.
pub fn reveal_in_finder(path: &str) {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed.contains("://") {
        return;
    }
    let Ok(canon) = std::fs::canonicalize(trimmed) else {
        return;
    };
    if !canon.is_dir() && !canon.is_file() {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&canon).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&canon).spawn();
    }
}

/// Top-level workspace layout mode matching `Herdr.app` (`main` workbench and `terminals` grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutMode {
    #[serde(
        alias = "main",
        alias = "code",
        alias = "board",
        alias = "editor",
        alias = "kanban"
    )]
    Workbench,
    #[serde(alias = "terminal")]
    Terminals,
}

impl Default for LayoutMode {
    fn default() -> Self {
        Self::Workbench
    }
}

impl LayoutMode {
    pub fn from_slug(slug: &str) -> Self {
        match slug.trim().to_ascii_lowercase().as_str() {
            "terminals" | "terminal" => Self::Terminals,
            _ => Self::Workbench,
        }
    }

    pub fn as_slug(self) -> &'static str {
        match self {
            Self::Workbench => "workbench",
            Self::Terminals => "terminals",
        }
    }

    pub fn label(self, locale: UiLocale) -> &'static str {
        match self {
            Self::Workbench => tr(locale, "Main", "主工作台"),
            Self::Terminals => tr(locale, "Terminals", "终端平铺"),
        }
    }
}

/// Hot-reloadable UI configuration at `~/.config/coda/layout.json` that both
/// the user and agents running inside Coda can inspect or modify.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MalleableLayoutConfig {
    pub layout: LayoutMode,
    pub sidebar_visible: bool,
    pub sidebar_width_px: f32,
    pub split_terminals: bool,
    pub font_family: String,
    pub terminal_font_size_px: f32,
    pub accent_hex: String,
}

impl Default for MalleableLayoutConfig {
    fn default() -> Self {
        Self {
            layout: LayoutMode::Workbench,
            sidebar_visible: true,
            sidebar_width_px: 236.0,
            split_terminals: false,
            font_family: "Menlo".to_string(),
            terminal_font_size_px: 13.0,
            accent_hex: "#38bdf8".to_string(),
        }
    }
}

impl MalleableLayoutConfig {
    pub fn path() -> Option<PathBuf> {
        config_root_dir().map(|dir| dir.join("layout.json"))
    }

    pub fn load_with_mtime() -> (Self, Option<std::time::SystemTime>) {
        let Some(path) = Self::path() else {
            return (Self::default(), None);
        };
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let cfg = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
            .unwrap_or_default();
        (cfg, mtime)
    }

    pub fn save_default_if_missing() {
        let Some(path) = Self::path() else {
            return;
        };
        if path.exists() {
            return;
        }
        let _ = Self::default().save();
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or_else(|| "cannot resolve config directory".to_string())?;
        let payload = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_file_atomic_mode(&path, payload.as_bytes(), 0o644).map_err(|e| e.to_string())
    }

    pub fn accent_color_u32(&self) -> u32 {
        Self::parse_hex_color(&self.accent_hex).unwrap_or(0x38bdf8)
    }

    pub fn parse_hex_color(hex: &str) -> Option<u32> {
        let trimmed = hex.trim().trim_start_matches('#');
        if trimmed.len() != 6 {
            return None;
        }
        u32::from_str_radix(trimmed, 16).ok()
    }
}

/// Generates `~/.config/coda/AGENTS.md` (if absent) so coding agents running
/// inside Coda terminals know how to reshape the workspace layout and emit OSC 7501 status updates.
pub fn ensure_agents_md_manual() -> Option<PathBuf> {
    let dir = config_root_dir()?;
    let path = dir.join("AGENTS.md");
    if !path.exists() {
        let manual = r##"# Coda Workbench & Agent Integration Guide

Coda watches `~/.config/coda/layout.json` (hot-reloaded every 1s) so you or agents running inside Coda can adapt the workspace layout programmatically.

## 1. Layout Config (`~/.config/coda/layout.json`)

```json
{
  "layout": "workbench",
  "sidebar_visible": true,
  "sidebar_width_px": 236,
  "split_terminals": false,
  "font_family": "Menlo",
  "terminal_font_size_px": 13,
  "accent_hex": "#38bdf8"
}
```

- `"layout"`: `"workbench"` | `"terminals"`
- `"split_terminals"`: `true` enables side-by-side split terminals

## 2. Agent Lifecycle Status via OSC 7501

Terminals running inside Coda parse OSC 7501 escape sequences in stdout to update the Agents inbox:

```sh
printf '\033]7501;state=working;task=Running cargo test\007'
printf '\033]7501;state=blocked;task=Waiting for review\007'
printf '\033]7501;state=done;task=All checks passed\007'
printf '\033]7501;state=idle\007'
```

## 3. Decoupled Session Persistence (`hc`)

When `hc` (`herdr-core`) is installed, Coda attaches sessions via `hc attach` so agent processes continue running even if the desktop window is closed.
"##;
        let _ = write_file_atomic_mode(&path, manual.as_bytes(), 0o644);
    }
    Some(path)
}

/// Deterministic vibrant avatar accent color for a workspace name.
pub fn space_avatar_color(name: &str) -> u32 {
    const PALETTE: &[u32] = &[
        0x38bdf8, // sky
        0x22c55e, // emerald
        0xa78bfa, // violet
        0xf59e0b, // amber
        0xf472b6, // pink
        0x2dd4bf, // teal
        0x60a5fa, // blue
        0xfb7185, // rose
    ];
    let hash = name
        .bytes()
        .fold(2166136261u32, |acc, b| acc.wrapping_mul(16777619) ^ (b as u32));
    PALETTE[(hash as usize) % PALETTE.len()]
}

/// Persisted E2EE desktop client identity (`~/.config/coda/desktop_client_identity.json`, mode `0600`)
/// so desktop restarts reuse one registered client device instead of minting endless devices on the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopClientIdentityRecord {
    pub server_url: String,
    pub token_sha256_hex: String,
    pub device_id: uuid::Uuid,
    pub identity: coda_protocol::StoredDeviceIdentity,
}

impl DesktopClientIdentityRecord {
    pub fn default_path() -> PathBuf {
        config_root_dir()
            .unwrap_or_else(|| PathBuf::from(".config/coda"))
            .join("desktop_client_identity.json")
    }

    pub fn token_hash_hex(server_url: &str, token: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(server_url.trim().trim_end_matches('/').as_bytes());
        h.update(b"|");
        h.update(token.trim().as_bytes());
        hex::encode(h.finalize())
    }

    pub fn load_matching(server_url: &str, token: &str) -> Option<(uuid::Uuid, coda_protocol::StoredDeviceIdentity)> {
        let path = Self::default_path();
        let raw = std::fs::read_to_string(&path).ok()?;
        let rec: Self = serde_json::from_str(&raw).ok()?;
        let expected_hash = Self::token_hash_hex(server_url, token);
        let clean_server = server_url.trim().trim_end_matches('/');
        if rec.server_url.trim().trim_end_matches('/') == clean_server
            && rec.token_sha256_hex == expected_hash
        {
            Some((rec.device_id, rec.identity))
        } else {
            None
        }
    }

    pub fn save_for_token(
        server_url: &str,
        token: &str,
        device_id: uuid::Uuid,
        identity: &coda_protocol::StoredDeviceIdentity,
    ) -> anyhow::Result<()> {
        let rec = Self {
            server_url: server_url.trim().trim_end_matches('/').to_string(),
            token_sha256_hex: Self::token_hash_hex(server_url, token),
            device_id,
            identity: identity.clone(),
        };
        let body = serde_json::to_string_pretty(&rec)?;
        write_file_atomic_mode(&Self::default_path(), body.as_bytes(), 0o600)?;
        Ok(())
    }

    pub fn remove_saved() {
        let _ = std::fs::remove_file(Self::default_path());
    }
}

/// Validates that `raw` is a clean `http://` or `https://` URL with no whitespace or control characters.
pub fn is_safe_http_url(raw: &str) -> bool {
    let trimmed = raw.trim();
    (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        && trimmed.len() <= 2048
        && !trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
}
