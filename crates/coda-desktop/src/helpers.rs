//! Pure utility, Keychain-backed daemon config IO, shell path escaping, clipboard PNG helpers,
//! and Git diff parsing helpers for `coda-desktop`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use coda_daemon::{active_daemon_lock_pid, DaemonConfig};
use uuid::Uuid;

use crate::config::{load_secret_from_keychain, save_secret_in_keychain};
use crate::git_inspector::ChangedFileEntry;
use crate::pty::{AgentLifecycleState, Osc7501Status};

pub const MAX_CLIPBOARD_PNG_BYTES: usize = 8 * 1024 * 1024;
const PNG_MAGIC_HEADER: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Header,
    Hunk,
    Addition,
    Deletion,
    Context,
}

pub(crate) fn load_desktop_daemon_config(path: &Path) -> anyhow::Result<DaemonConfig> {
    let mut cfg = DaemonConfig::load(path)?;
    if cfg.session_token.is_none() {
        cfg.session_token = load_secret_from_keychain("session_token");
    } else if let Some(ref tok) = cfg.session_token {
        // Migrate existing plaintext disk token into macOS Keychain when running inside Coda.app
        // without an external standalone coda-daemon holding the lock.
        let external_owns = active_daemon_lock_pid(path)
            .map(|pid| pid != std::process::id())
            .unwrap_or(false);
        if !external_owns && save_secret_in_keychain("session_token", tok) {
            let mut scrubbed = cfg.clone();
            scrubbed.session_token = None;
            let _ = scrubbed.save(path);
        }
    }
    Ok(cfg)
}

pub(crate) fn save_desktop_daemon_config(cfg: &DaemonConfig, path: &Path) -> anyhow::Result<()> {
    let external_owns = active_daemon_lock_pid(path)
        .map(|pid| pid != std::process::id())
        .unwrap_or(false);
    if let Some(ref tok) = cfg.session_token {
        let stored_in_keychain = save_secret_in_keychain("session_token", tok);
        if stored_in_keychain && !external_owns {
            let mut scrubbed = cfg.clone();
            scrubbed.session_token = None;
            return scrubbed.save(path);
        }
    }
    cfg.save(path)
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

/// Extracts an image from the system clipboard as PNG bytes (using `osascript` `«class PNGf»` on macOS).
pub fn extract_clipboard_png_bytes() -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    {
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
        let out = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .arg(&scratch)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("Failed invoking osascript for clipboard image: {e}"))?;
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
        Ok(bytes)
    }
    #[cfg(target_os = "linux")]
    {
        for (bin, args) in [
            ("wl-paste", &["--no-newline", "--type", "image/png"][..]),
            ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"][..]),
        ] {
            if let Ok(out) = std::process::Command::new(bin)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
            {
                if out.status.success() && is_valid_png_bytes(&out.stdout) {
                    return Ok(out.stdout);
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
