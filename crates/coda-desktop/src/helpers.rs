//! Pure utility, Keychain-backed daemon config IO, shell path escaping, clipboard PNG helpers,
//! and Git diff parsing helpers for `coda-desktop`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use coda_daemon::DaemonConfig;
use uuid::Uuid;

use crate::config::{load_secret_from_keychain, save_secret_in_keychain};
use crate::git_inspector::ChangedFileEntry;
use crate::pty::{AgentLifecycleState, Osc7501Status};

pub const MAX_CLIPBOARD_PNG_BYTES: usize = 8 * 1024 * 1024;
const PNG_MAGIC_HEADER: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const CLIPBOARD_EXTRACT_TIMEOUT: Duration = Duration::from_secs(5);
static CLIPBOARD_EXTRACT_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
struct CachedDesktopDaemonConfig {
    path: PathBuf,
    mtime: Option<SystemTime>,
    cfg: DaemonConfig,
}

static DESKTOP_DAEMON_CONFIG_CACHE: OnceLock<Mutex<Option<CachedDesktopDaemonConfig>>> =
    OnceLock::new();

fn daemon_config_cache() -> &'static Mutex<Option<CachedDesktopDaemonConfig>> {
    DESKTOP_DAEMON_CONFIG_CACHE.get_or_init(|| Mutex::new(None))
}

pub(crate) fn invalidate_desktop_daemon_config_cache() {
    if let Ok(mut g) = daemon_config_cache().lock() {
        *g = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Header,
    Hunk,
    Addition,
    Deletion,
    Context,
}

/// Loads `DaemonConfig` from `path` with an in-memory `(path, mtime)` + Keychain cache so UI render
/// frames and concurrent Tokio proxy workers NEVER spawn blocking `/usr/bin/security` subprocesses,
/// while keeping `session_token` and `daemon_relay_ticket` intact in `0o600` `daemon.json` so embedded
/// and standalone `coda-daemon` never lose auth (`Bug #9`).
pub(crate) fn load_desktop_daemon_config(path: &Path) -> anyhow::Result<DaemonConfig> {
    let mtime = match std::fs::metadata(path) {
        Ok(m) => m.modified().ok(),
        Err(e) => {
            if let Ok(mut cache) = daemon_config_cache().lock() {
                if cache.as_ref().map(|c| c.path == path).unwrap_or(false) {
                    *cache = None;
                }
            }
            return Err(e.into());
        }
    };

    if let Ok(cache) = daemon_config_cache().lock() {
        if let Some(ref hit) = *cache {
            if hit.path == path && hit.mtime == mtime {
                return Ok(hit.cfg.clone());
            }
        }
    }

    let mut cfg = DaemonConfig::load(path)?;
    let mut final_mtime = mtime;

    if cfg.session_token.is_none() {
        if let Some(kc_tok) = load_secret_from_keychain("session_token") {
            cfg.session_token = Some(kc_tok);
            // Non-destructively heal previously scrubbed `daemon.json` (preserving `daemon_relay_ticket`)
            // so `start_embedded_host_with_handle` and standalone `coda-daemon` retain auth.
            if cfg.save(path).is_ok() {
                final_mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
            }
        }
    } else if let Some(ref tok) = cfg.session_token {
        // Mirror into Keychain cache when running inside Coda.app, WITHOUT stripping `session_token`
        // from `0o600` `daemon.json`.
        let _ = save_secret_in_keychain("session_token", tok);
    }

    if let Ok(mut cache) = daemon_config_cache().lock() {
        *cache = Some(CachedDesktopDaemonConfig {
            path: path.to_path_buf(),
            mtime: final_mtime,
            cfg: cfg.clone(),
        });
    }
    Ok(cfg)
}

/// Saves `DaemonConfig` to `path` (`0o600`), preserving any active `daemon_relay_ticket` for the
/// same device/server, mirroring `session_token` into Keychain, and refreshing the in-memory cache.
pub(crate) fn save_desktop_daemon_config(cfg: &DaemonConfig, path: &Path) -> anyhow::Result<()> {
    let mut merged = cfg.clone();
    if merged.daemon_relay_ticket.trim().is_empty() {
        if let Ok(existing) = DaemonConfig::load(path) {
            if existing.device_id == merged.device_id
                && existing.server_url.trim().trim_end_matches('/')
                    == merged.server_url.trim().trim_end_matches('/')
                && !existing.daemon_relay_ticket.trim().is_empty()
            {
                merged.daemon_relay_ticket = existing.daemon_relay_ticket;
            }
        }
    }
    if let Some(ref tok) = merged.session_token {
        let _ = save_secret_in_keychain("session_token", tok);
    }
    merged.save(path)?;
    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
    if let Ok(mut cache) = daemon_config_cache().lock() {
        *cache = Some(CachedDesktopDaemonConfig {
            path: path.to_path_buf(),
            mtime,
            cfg: merged,
        });
    }
    Ok(())
}

pub(crate) fn utf16_slice(s: &str, range_utf16: std::ops::Range<usize>) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    let start = range_utf16.start.min(units.len());
    let end = range_utf16.end.min(units.len()).max(start);
    String::from_utf16_lossy(&units[start..end])
}

pub(crate) fn herdr_status_to_osc(
    status: &Option<coda_protocol::HerdrAgentStatus>,
) -> Option<Osc7501Status> {
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

pub(crate) fn normalize_newlines_for_vt(text: &str) -> Vec<u8> {
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

pub(crate) fn path_within_root(path: &str, root: &str) -> bool {
    let p = path.trim_end_matches('/');
    let r = root.trim_end_matches('/');
    !p.is_empty()
        && !r.is_empty()
        && (p == r || (p.len() > r.len() && p.starts_with(r) && p.as_bytes()[r.len()] == b'/'))
}

pub fn parse_git_status_short(status_short: &str) -> Vec<ChangedFileEntry> {
    status_short
        .lines()
        .filter_map(|l| {
            if l.starts_with("## ") || l.len() < 4 {
                return None;
            }
            let raw_code = l.get(..2)?.trim();
            let rest = l.get(3..)?.trim();
            if rest.is_empty() {
                return None;
            }
            let target_raw = rest.rsplit(" -> ").next().unwrap_or(rest).trim();
            let path = if target_raw.starts_with('"')
                && target_raw.ends_with('"')
                && target_raw.len() >= 2
            {
                target_raw[1..target_raw.len() - 1].to_string()
            } else {
                target_raw.to_string()
            };
            if path.is_empty() {
                return None;
            }
            let status = if raw_code.is_empty() {
                "M".to_string()
            } else {
                raw_code.to_string()
            };
            Some(ChangedFileEntry { status, path })
        })
        .collect()
}

pub fn classify_diff_line(line: &str) -> DiffLineKind {
    if line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("dissimilarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("Binary files ")
    {
        DiffLineKind::Header
    } else if line.starts_with("@@") {
        DiffLineKind::Hunk
    } else if line.starts_with('+') {
        DiffLineKind::Addition
    } else if line.starts_with('-') {
        DiffLineKind::Deletion
    } else {
        DiffLineKind::Context
    }
}

fn diff_git_header_matches_path(header_line: &str, target: &str) -> bool {
    let b_suffix = format!(" b/{target}");
    let b_quoted_suffix = format!(" \"b/{target}\"");
    let a_prefix = format!("diff --git a/{target} ");
    let a_quoted_prefix = format!("diff --git \"a/{target}\" ");
    header_line.ends_with(&b_suffix)
        || header_line.ends_with(&b_quoted_suffix)
        || header_line.starts_with(&a_prefix)
        || header_line.starts_with(&a_quoted_prefix)
}

pub fn extract_file_diff_lines<'a>(
    diff_text: &'a str,
    selected_path: Option<&str>,
) -> Vec<&'a str> {
    const MAX_DIFF_RENDER_LINES: usize = 400;
    if let Some(target) = selected_path {
        if diff_text.contains("diff --git ") {
            let mut in_block = false;
            let mut filtered = Vec::new();
            for line in diff_text.lines() {
                if line.starts_with("diff --git ") {
                    in_block = diff_git_header_matches_path(line, target);
                }
                if in_block {
                    filtered.push(line);
                    if filtered.len() >= MAX_DIFF_RENDER_LINES {
                        break;
                    }
                }
            }
            return filtered;
        }
    }
    diff_text.lines().take(MAX_DIFF_RENDER_LINES).collect()
}

/// Escapes a filesystem path for safe direct insertion into an interactive POSIX shell or agent TUI.
pub fn shell_escape_terminal_path(path: &str) -> String {
    if !path.is_empty()
        && !path.starts_with('-')
        && path.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | ':')
        })
    {
        return path.to_string();
    }
    let mut out = String::with_capacity(path.len() + 8);
    out.push('\'');
    for ch in path.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Formats a pasted file path with POSIX shell escaping and a trailing space for terminal input.
pub fn format_pasted_terminal_path(path: &str) -> String {
    format!("{} ", shell_escape_terminal_path(path))
}

/// Iteratively strips forged bracketed-paste start/end escape sequences until fixed-point.
pub fn sanitize_bracketed_paste_text(text: &str) -> String {
    let mut cur = text.to_string();
    loop {
        let next = cur.replace("\x1b[200~", "").replace("\x1b[201~", "");
        if next.len() == cur.len() {
            return next;
        }
        cur = next;
    }
}

/// Derives a safe default folder name from a Git clone URL (e.g. `https://github.com/org/repo.git` -> `"repo"`).
pub fn derive_folder_name_from_git_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let tail = trimmed
        .rsplit(['/', ':'])
        .next()?
        .trim_end_matches(".git")
        .trim();
    if tail.is_empty()
        || tail == "."
        || tail == ".."
        || tail.starts_with('-')
        || !tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        None
    } else {
        Some(tail.to_string())
    }
}

/// Returns true if `path` has a supported image file extension.
pub fn is_supported_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff" | "tif" | "heic"
            )
        })
        .unwrap_or(false)
}

/// Detects when macOS Finder file-copy populates NSPasteboard text with a bare image filename
/// (`"Screenshot 1.png"`) or `file://` URL, which would otherwise shadow image/file paste.
pub fn looks_like_copied_image_filename(text: &str) -> bool {
    if text.contains('\n') || text.contains('\r') {
        return false;
    }
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 512 {
        return false;
    }
    if let Some(rest) = trimmed.strip_prefix("file://") {
        return is_supported_image_extension(Path::new(rest));
    }
    !trimmed.contains('/') && !trimmed.contains('\\') && is_supported_image_extension(Path::new(trimmed))
}

#[derive(Debug, Clone)]
pub enum ClipboardImagePayload {
    PngBytes(Vec<u8>),
    LocalImageFile(PathBuf),
}

struct ClipboardExtractInFlightGuard;

impl ClipboardExtractInFlightGuard {
    fn try_acquire() -> Option<Self> {
        CLIPBOARD_EXTRACT_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for ClipboardExtractInFlightGuard {
    fn drop(&mut self) {
        CLIPBOARD_EXTRACT_IN_FLIGHT.store(false, Ordering::Release);
    }
}

fn run_command_with_timeout(
    mut cmd: std::process::Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    use std::io::Read;

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed spawning clipboard helper: {e}"))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut out) = stdout_pipe {
            let _ = out.take((MAX_CLIPBOARD_PNG_BYTES + 1024) as u64).read_to_end(&mut buf);
        }
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut err) = stderr_pipe {
            let _ = err.take(4096).read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_handle.join().unwrap_or_default();
                let stderr = stderr_handle.join().unwrap_or_default();
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Clipboard image extraction timed out after 5s".into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Clipboard helper wait failed: {e}"));
            }
        }
    }
}

/// Validates that `bytes` starts with the 8-byte PNG signature and fits within the 8 MiB E2EE upload bound.
pub fn is_valid_png_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= PNG_MAGIC_HEADER.len()
        && bytes.len() <= MAX_CLIPBOARD_PNG_BYTES
        && bytes.starts_with(PNG_MAGIC_HEADER)
}

/// Saves validated PNG bytes to `/tmp/coda-paste-<uuid>.png` with owner-only `0o600` permissions.
pub fn save_clipboard_png_to_tmp(png_bytes: &[u8]) -> Result<PathBuf, String> {
    if png_bytes.len() > MAX_CLIPBOARD_PNG_BYTES {
        return Err("Clipboard image exceeds 8 MiB limit".into());
    }
    if !is_valid_png_bytes(png_bytes) {
        return Err("Clipboard payload is not a valid PNG image".into());
    }
    let base_dir = if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    let out_path = base_dir.join(format!("coda-paste-{}.png", Uuid::new_v4()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&out_path)
        .map_err(|e| format!("Failed creating {}: {e}", out_path.display()))?;
    if let Err(e) = file.write_all(png_bytes).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(&out_path);
        return Err(format!("Failed writing clipboard PNG: {e}"));
    }
    Ok(out_path)
}

/// Extracts either a copied Finder image file (`«class furl»`) or raw PNG clipboard bytes (`«class PNGf»`),
/// guarded by a single in-flight `AtomicBool` lock and a strict 5-second subprocess timeout (`Bug #10`).
pub fn extract_clipboard_image_or_file() -> Result<ClipboardImagePayload, String> {
    let Some(_guard) = ClipboardExtractInFlightGuard::try_acquire() else {
        return Err("Clipboard image extraction already in progress".into());
    };
    extract_clipboard_image_or_file_unlocked()
}

fn extract_clipboard_image_or_file_unlocked() -> Result<ClipboardImagePayload, String> {
    #[cfg(target_os = "macos")]
    {
        // 1. Check if the user copied an image file in Finder (`«class furl»`) so Finder's bare
        //    filename string or icon preview never shadows the actual file!
        let file_script = r#"try
    POSIX path of (the clipboard as «class furl»)
on error
    ""
end try"#;
        let mut furl_cmd = std::process::Command::new("/usr/bin/osascript");
        furl_cmd.arg("-e").arg(file_script);
        if let Ok(out) = run_command_with_timeout(furl_cmd, CLIPBOARD_EXTRACT_TIMEOUT) {
            if out.status.success() {
                let candidate = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !candidate.is_empty() {
                    let p = PathBuf::from(&candidate);
                    if let Ok(meta) = std::fs::metadata(&p) {
                        if meta.is_file() && is_supported_image_extension(&p) {
                            if meta.len() > MAX_CLIPBOARD_PNG_BYTES as u64 {
                                return Err("Copied image file exceeds 8 MiB limit".into());
                            }
                            return Ok(ClipboardImagePayload::LocalImageFile(p));
                        }
                    }
                }
            }
        }

        // 2. Fall back to raw clipboard PNG data (`«class PNGf»`, e.g. macOS screenshot).
        let scratch = std::env::temp_dir()
            .join(format!(".coda-clip-extract-{}.png", Uuid::new_v4().simple()));
        let script = r#"on run argv
    set outPath to POSIX file (item 1 of argv)
    set pngData to the clipboard as «class PNGf»
    set fRef to open for access outPath with write permission
    try
        set eof fRef to 0
        write pngData to fRef
        close access fRef
    on error errMsg
        try
            close access fRef
        end try
        error errMsg
    end try
end run"#;
        let mut cmd = std::process::Command::new("/usr/bin/osascript");
        cmd.arg("-e").arg(script).arg(&scratch);
        let out = match run_command_with_timeout(cmd, CLIPBOARD_EXTRACT_TIMEOUT) {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_file(&scratch);
                return Err(e);
            }
        };
        if !out.status.success() {
            let _ = std::fs::remove_file(&scratch);
            return Err("No image found in clipboard".into());
        }
        let read_res = std::fs::read(&scratch);
        let _ = std::fs::remove_file(&scratch);
        let bytes = read_res.map_err(|e| format!("Failed reading extracted clipboard image: {e}"))?;
        if bytes.len() > MAX_CLIPBOARD_PNG_BYTES {
            return Err("Clipboard image exceeds 8 MiB limit".into());
        }
        if !is_valid_png_bytes(&bytes) {
            return Err("Clipboard data is not a valid PNG image".into());
        }
        Ok(ClipboardImagePayload::PngBytes(bytes))
    }
    #[cfg(target_os = "linux")]
    {
        for (bin, args) in [
            ("wl-paste", &["--no-newline", "--type", "image/png"][..]),
            ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"][..]),
        ] {
            let mut cmd = std::process::Command::new(bin);
            cmd.args(args);
            if let Ok(out) = run_command_with_timeout(cmd, CLIPBOARD_EXTRACT_TIMEOUT) {
                if out.status.success() && is_valid_png_bytes(&out.stdout) {
                    return Ok(ClipboardImagePayload::PngBytes(out.stdout));
                }
            }
        }
        Err("No image found in clipboard".into())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("Clipboard image paste unsupported on this platform".into())
    }
}

/// Extracts an image from the system clipboard as PNG bytes, guarded by an in-flight `AtomicBool` lock and 5s timeout.
pub fn extract_clipboard_png_bytes() -> Result<Vec<u8>, String> {
    match extract_clipboard_image_or_file()? {
        ClipboardImagePayload::PngBytes(bytes) => Ok(bytes),
        ClipboardImagePayload::LocalImageFile(path) => {
            let bytes =
                std::fs::read(&path).map_err(|e| format!("Failed reading copied image: {e}"))?;
            if is_valid_png_bytes(&bytes) {
                return Ok(bytes);
            }
            #[cfg(target_os = "macos")]
            {
                let scratch = std::env::temp_dir()
                    .join(format!(".coda-clip-sips-{}.png", Uuid::new_v4().simple()));
                let mut cmd = std::process::Command::new("/usr/bin/sips");
                cmd.args(["-s", "format", "png"])
                    .arg(&path)
                    .arg("--out")
                    .arg(&scratch);
                let out = run_command_with_timeout(cmd, CLIPBOARD_EXTRACT_TIMEOUT);
                let read_res = std::fs::read(&scratch);
                let _ = std::fs::remove_file(&scratch);
                if out.map(|o| o.status.success()).unwrap_or(false) {
                    if let Ok(converted) = read_res {
                        if is_valid_png_bytes(&converted) {
                            return Ok(converted);
                        }
                    }
                }
            }
            Err("Clipboard data is not a valid PNG image".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_escape_and_pasted_terminal_path() {
        assert_eq!(
            shell_escape_terminal_path("/tmp/coda-paste-1234abcd.png"),
            "/tmp/coda-paste-1234abcd.png"
        );
        assert_eq!(
            format_pasted_terminal_path("/tmp/coda-paste-1234abcd.png"),
            "/tmp/coda-paste-1234abcd.png "
        );
        assert_eq!(
            shell_escape_terminal_path("/Users/alice/My Screenshots/shot 1.png"),
            "'/Users/alice/My Screenshots/shot 1.png'"
        );
        assert_eq!(
            format_pasted_terminal_path("/tmp/it's $(whoami) `ls`.png"),
            "'/tmp/it'\\''s $(whoami) `ls`.png' "
        );
        assert_eq!(shell_escape_terminal_path("-leading-dash.png"), "'-leading-dash.png'");
        assert_eq!(shell_escape_terminal_path(""), "''");
    }

    #[test]
    fn test_save_clipboard_png_to_tmp_and_validation() {
        let mut valid_png = PNG_MAGIC_HEADER.to_vec();
        valid_png.extend_from_slice(b"IHDR-mock-payload");
        assert!(is_valid_png_bytes(&valid_png));
        assert!(!is_valid_png_bytes(b"not-a-png"));
        assert!(!is_valid_png_bytes(b""));

        let saved = save_clipboard_png_to_tmp(&valid_png).expect("save valid PNG");
        assert!(saved.to_string_lossy().contains("coda-paste-"));
        assert!(saved.to_string_lossy().ends_with(".png"));
        assert_eq!(std::fs::read(&saved).unwrap(), valid_png);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&saved).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_file(&saved);

        assert!(save_clipboard_png_to_tmp(b"JPEG").is_err());
    }

    #[test]
    fn test_clipboard_inflight_guard_timeout_and_finder_filename_detection() {
        assert!(looks_like_copied_image_filename("Screenshot 2026-11-21.png"));
        assert!(looks_like_copied_image_filename("diagram.JPG"));
        assert!(looks_like_copied_image_filename("file:///Users/alice/Desktop/pic.webp"));
        assert!(!looks_like_copied_image_filename("/Users/alice/Desktop/pic.png"));
        assert!(!looks_like_copied_image_filename("cargo test -p coda-desktop"));
        assert!(!looks_like_copied_image_filename("line1.png\nline2.png"));

        assert_eq!(
            sanitize_bracketed_paste_text("hello \x1b[\x1b[201~201~ world \x1b[200~"),
            "hello  world "
        );
        assert_eq!(
            derive_folder_name_from_git_url("https://github.com/Annihilater/coda.git"),
            Some("coda".to_string())
        );
        assert_eq!(
            derive_folder_name_from_git_url("git@github.com:org/my_repo.git/"),
            Some("my_repo".to_string())
        );

        let guard = ClipboardExtractInFlightGuard::try_acquire().expect("acquire first");
        assert!(ClipboardExtractInFlightGuard::try_acquire().is_none());
        assert!(
            extract_clipboard_png_bytes()
                .unwrap_err()
                .contains("already in progress")
        );
        drop(guard);
        assert!(ClipboardExtractInFlightGuard::try_acquire().is_some());

        #[cfg(unix)]
        {
            let mut slow_cmd = std::process::Command::new("/bin/sleep");
            slow_cmd.arg("2");
            let err = run_command_with_timeout(slow_cmd, Duration::from_millis(80)).unwrap_err();
            assert!(err.contains("timed out"));
        }
    }

    #[test]
    fn test_daemon_config_memory_cache_and_non_destructive_token_preservation() {
        let tmp = std::env::temp_dir().join(format!("coda-dcfg-test-{}.json", Uuid::new_v4()));
        let ident = coda_protocol::StoredDeviceIdentity::generate("Mac Studio");
        let dev_id = Uuid::new_v4();
        let initial = DaemonConfig {
            server_url: "https://codahub.dev".to_string(),
            owner_handle: "alice".to_string(),
            owner_email: Some("alice@example.com".to_string()),
            session_token: Some("tok-1".to_string()),
            device_id: dev_id,
            daemon_relay_ticket: "ticket-keep-me".to_string(),
            relays: Vec::new(),
            identity: ident,
        };
        save_desktop_daemon_config(&initial, &tmp).expect("save initial");

        // Disk file MUST keep both session_token and daemon_relay_ticket intact for coda-daemon!
        let raw_on_disk = DaemonConfig::load(&tmp).expect("load raw");
        assert_eq!(raw_on_disk.session_token.as_deref(), Some("tok-1"));
        assert_eq!(raw_on_disk.daemon_relay_ticket, "ticket-keep-me");

        // Saving updated session_token with empty daemon_relay_ticket preserves existing ticket!
        let mut rotated = initial.clone();
        rotated.session_token = Some("tok-2".to_string());
        rotated.daemon_relay_ticket.clear();
        save_desktop_daemon_config(&rotated, &tmp).expect("save rotated");

        let cached = load_desktop_daemon_config(&tmp).expect("load cached");
        assert_eq!(cached.session_token.as_deref(), Some("tok-2"));
        assert_eq!(cached.daemon_relay_ticket, "ticket-keep-me");

        let _ = std::fs::remove_file(&tmp);
        invalidate_desktop_daemon_config_cache();
    }

    #[test]
    fn test_git_status_short_and_diff_filtering_and_classification() {
        let status = " M crates/coda-desktop/src/lib.rs\n?? crates/coda-desktop/src/new_helper.rs\nR  old.rs -> \"new file.rs\"\n";
        let parsed = parse_git_status_short(status);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].status, "M");
        assert_eq!(parsed[0].path, "crates/coda-desktop/src/lib.rs");
        assert_eq!(parsed[1].status, "??");
        assert_eq!(parsed[1].path, "crates/coda-desktop/src/new_helper.rs");
        assert_eq!(parsed[2].status, "R");
        assert_eq!(parsed[2].path, "new file.rs");

        let unified = "\
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 fn a() {}
+fn added_in_a() {}
-fn removed_in_a() {}
";
        let only_a = extract_file_diff_lines(unified, Some("src/a.rs"));
        assert_eq!(only_a.len(), 8);
        assert_eq!(classify_diff_line(only_a[0]), DiffLineKind::Header);
        assert_eq!(classify_diff_line(only_a[2]), DiffLineKind::Header);
        assert_eq!(classify_diff_line(only_a[3]), DiffLineKind::Header);
        assert_eq!(classify_diff_line(only_a[4]), DiffLineKind::Hunk);
        assert_eq!(classify_diff_line(only_a[5]), DiffLineKind::Context);
        assert_eq!(classify_diff_line(only_a[6]), DiffLineKind::Addition);
        assert_eq!(classify_diff_line(only_a[7]), DiffLineKind::Deletion);

        // Selecting an untracked file absent from `git diff` must NOT leak `src/a.rs` hunks.
        let untracked = extract_file_diff_lines(unified, Some("src/new_helper.rs"));
        assert!(untracked.is_empty());
    }
}
