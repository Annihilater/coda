//! Pure utility, Keychain-backed daemon config IO, and parsing helpers for `coda-desktop`.

use std::path::Path;
use std::time::Instant;

use coda_daemon::{active_daemon_lock_pid, DaemonConfig};

use crate::config::{load_secret_from_keychain, save_secret_in_keychain};
use crate::git_inspector::ChangedFileEntry;
use crate::pty::{AgentLifecycleState, Osc7501Status};

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

pub(crate) fn parse_git_status_short(status_short: &str) -> Vec<ChangedFileEntry> {
    status_short
        .lines()
        .filter_map(|l| {
            let status = l.get(..2)?.trim().to_string();
            let path = l.get(3..)?.trim().to_string();
            (!path.is_empty()).then_some(ChangedFileEntry { status, path })
        })
        .collect()
}

pub(crate) fn extract_file_diff_lines<'a>(
    diff_text: &'a str,
    selected_path: Option<&str>,
) -> Vec<&'a str> {
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
