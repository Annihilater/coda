//! Non-blocking host health telemetry & zero-downtime E2EE OTA self-upgrade for `coda-daemon`.
//!
//! Because interactive shells and coding agents (`claude`, `codex`, `grok`, `agy`, `gemini`) run
//! decoupled inside `herdr-core` (`outlive_parent: true`), upgrading and `exec()`-restarting
//! `coda-daemon` preserves all active agent sessions without dropping a single process.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use coda_protocol::HostHealthSnapshot;
use sha2::{Digest, Sha256};

const MAX_OTA_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;

/// Resolves the on-disk path of the current executable, stripping any Linux `" (deleted)"` suffix
/// if a previous in-place `rename()` already replaced the running inode.
pub fn resolve_clean_exe_path() -> Result<PathBuf> {
    let raw = std::env::current_exe().context("failed resolving current_exe")?;
    if let Some(s) = raw.to_str() {
        if let Some(stripped) = s.strip_suffix(" (deleted)") {
            return Ok(PathBuf::from(stripped));
        }
    }
    Ok(raw)
}

/// Returns `true` when the current process is a standalone `coda-daemon` binary (not embedded inside
/// a macOS `.app` bundle or unit-test runner).
pub fn is_standalone_daemon_binary(exe_path: &Path) -> bool {
    let full = exe_path.to_string_lossy();
    if full.contains(".app/Contents/MacOS/") {
        return false;
    }
    let Some(fname) = exe_path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    fname == "coda-daemon" || fname == "coda-daemon.exe"
}

/// Validates an optional semantic release tag (e.g. `"v0.7.0"` or `"0.7.0"`), normalizing it to `"vX.Y.Z"`.
pub fn normalize_release_tag(raw: Option<&str>) -> Result<Option<String>> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if trimmed.len() > 36 {
        bail!("target_version exceeds maximum length");
    }
    let ver = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let mut parts = ver.splitn(2, '-');
    let core = parts.next().unwrap_or("");
    let nums: Vec<&str> = core.split('.').collect();
    if nums.len() != 3
        || nums
            .iter()
            .any(|n| n.is_empty() || n.len() > 6 || !n.chars().all(|c| c.is_ascii_digit()))
    {
        bail!("invalid target_version `{trimmed}` (expected semver like v0.7.0)");
    }
    if let Some(pre) = parts.next() {
        if pre.is_empty()
            || !pre
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            bail!("invalid prerelease suffix in `{trimmed}`");
        }
    }
    Ok(Some(format!("v{ver}")))
}

/// Collects non-blocking host CPU, load, memory, and uptime telemetry in < 1ms without spawning subprocesses.
pub fn collect_host_health_snapshot(
    daemon_started_at: Instant,
    herdr_version: Option<String>,
) -> HostHealthSnapshot {
    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);

    let load_avg_1m = {
        let mut avg = [0.0_f64; 1];
        let rc = unsafe { libc::getloadavg(avg.as_mut_ptr(), 1) };
        if rc == 1 && avg[0].is_finite() && avg[0] >= 0.0 {
            Some((avg[0] * 100.0).round() / 100.0)
        } else {
            None
        }
    };

    let (mem_used_mb, mem_total_mb) = sample_memory_mb();
    let os_uptime_secs = sample_os_uptime_secs();
    let ota_supported = resolve_clean_exe_path()
        .ok()
        .map(|p| is_standalone_daemon_binary(&p))
        .unwrap_or(false);

    HostHealthSnapshot {
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_cores,
        load_avg_1m,
        mem_used_mb,
        mem_total_mb,
        os_uptime_secs,
        daemon_uptime_secs: daemon_started_at.elapsed().as_secs(),
        herdr_version,
        ota_supported,
    }
}

#[cfg(target_os = "linux")]
fn sample_memory_mb() -> (Option<u64>, Option<u64>) {
    let Ok(txt) = std::fs::read_to_string("/proc/meminfo") else {
        return (None, None);
    };
    let mut total_kb: Option<u64> = None;
    let mut avail_kb: Option<u64> = None;
    for line in txt.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok());
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok());
        }
        if total_kb.is_some() && avail_kb.is_some() {
            break;
        }
    }
    match (total_kb, avail_kb) {
        (Some(t), Some(a)) if t > 0 => {
            let total_mb = t / 1024;
            let used_mb = t.saturating_sub(a) / 1024;
            (Some(used_mb), Some(total_mb))
        }
        (Some(t), None) if t > 0 => (None, Some(t / 1024)),
        _ => (None, None),
    }
}

#[cfg(target_os = "macos")]
fn sample_memory_mb() -> (Option<u64>, Option<u64>) {
    let total_bytes = unsafe {
        let mut mem: u64 = 0;
        let mut size = std::mem::size_of::<u64>();
        let rc = libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut mem).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        );
        if rc == 0 && mem > 0 {
            Some(mem)
        } else {
            None
        }
    };
    let Some(total_b) = total_bytes else {
        return (None, None);
    };
    let total_mb = total_b / (1024 * 1024);

    // Query Mach VM64 active + wired + compressed pages (matches Activity Monitor "Memory Used").
    #[repr(C)]
    #[derive(Default)]
    struct VmStatistics64 {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        zero_fill_count: u64,
        reactivations: u64,
        pageins: u64,
        pageouts: u64,
        faults: u64,
        cow_faults: u64,
        lookups: u64,
        hits: u64,
        purges: u64,
        purgeable_count: u32,
        speculative_count: u32,
        decompressions: u64,
        compressions: u64,
        swapins: u64,
        swapouts: u64,
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
    }
    extern "C" {
        fn mach_host_self() -> u32;
        fn host_page_size(host: u32, out_page_size: *mut usize) -> i32;
        fn host_statistics64(
            host: u32,
            flavor: i32,
            host_info_out: *mut i32,
            host_info_out_cnt: *mut u32,
        ) -> i32;
    }
    const HOST_VM_INFO64: i32 = 4;
    let used_mb = unsafe {
        let host = mach_host_self();
        let mut page_size: usize = 0;
        let mut vm = VmStatistics64::default();
        let mut count = (std::mem::size_of::<VmStatistics64>() / std::mem::size_of::<i32>()) as u32;
        if host_page_size(host, &raw mut page_size) == 0
            && page_size > 0
            && host_statistics64(
                host,
                HOST_VM_INFO64,
                (&raw mut vm).cast(),
                &raw mut count,
            ) == 0
        {
            let used_pages = (vm.internal_page_count.saturating_sub(vm.purgeable_count) as u64)
                + (vm.wire_count as u64)
                + (vm.compressor_page_count as u64);
            let mb = (used_pages.saturating_mul(page_size as u64)) / (1024 * 1024);
            Some(mb.min(total_mb))
        } else {
            None
        }
    };
    (used_mb, Some(total_mb))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample_memory_mb() -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(target_os = "linux")]
fn sample_os_uptime_secs() -> Option<u64> {
    let txt = std::fs::read_to_string("/proc/uptime").ok()?;
    let first = txt.split_whitespace().next()?;
    let secs: f64 = first.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then_some(secs as u64)
}

#[cfg(target_os = "macos")]
fn sample_os_uptime_secs() -> Option<u64> {
    unsafe {
        let mut boottime = libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        let mut size = std::mem::size_of::<libc::timeval>();
        let rc = libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            (&raw mut boottime).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        );
        if rc != 0 || boottime.tv_sec <= 0 {
            return None;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        (now >= boottime.tv_sec).then_some((now - boottime.tv_sec) as u64)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample_os_uptime_secs() -> Option<u64> {
    None
}

fn daemon_platform_slug() -> Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        other => bail!("OTA upgrade is not supported on OS `{other}`"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("OTA upgrade is not supported on architecture `{other}`"),
    };
    if os == "macos" && arch != "aarch64" {
        bail!("prebuilt macOS OTA binaries currently target Apple Silicon (aarch64)");
    }
    Ok((os, arch))
}

pub fn verify_sha256_manifest(manifest_text: &str, asset_names: &[&str], actual_bytes: &[u8]) -> Result<bool> {
    let actual_hex = hex::encode(Sha256::digest(actual_bytes));
    for line in manifest_text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let expected_hex = parts[0].trim().to_ascii_lowercase();
        let file_col = parts[1].trim().trim_start_matches('*');
        let base_col = Path::new(file_col)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(file_col);
        if asset_names.contains(&base_col) {
            if expected_hex.len() == 64 && expected_hex == actual_hex {
                return Ok(true);
            }
            bail!(
                "SHA-256 checksum mismatch for `{base_col}` (expected {expected_hex}, got {actual_hex})"
            );
        }
    }
    Ok(false)
}

async fn download_bytes_bounded(http: &reqwest::Client, url: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let mut resp = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?
        .error_for_status()
        .with_context(|| format!("HTTP error from {url}"))?;

    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            bail!("OTA archive from {url} exceeds {max_bytes} bytes");
        }
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len().saturating_add(chunk.len()) > max_bytes {
            bail!("OTA archive stream from {url} exceeded {max_bytes} bytes");
        }
        out.extend_from_slice(&chunk);
    }
    if out.len() < 1024 {
        bail!("downloaded OTA archive from {url} is unexpectedly small ({} bytes)", out.len());
    }
    Ok(out)
}

/// Downloads, SHA-256 checks (when `SHA256SUMS` is reachable), extracts, smoke-tests (`--version`),
/// and atomically replaces the running `coda-daemon` executable in place.
///
/// Returns `(previous_version, new_version_text)`.
pub async fn perform_daemon_ota_upgrade(
    server_url: &str,
    target_version: Option<&str>,
) -> Result<(String, String)> {
    let prev_ver = env!("CARGO_PKG_VERSION").to_string();
    let exe_path = resolve_clean_exe_path()?;
    if !is_standalone_daemon_binary(&exe_path) {
        bail!(
            "Host runs embedded inside Coda Desktop (`{}`); update Coda.app via DMG/Release instead",
            exe_path.display()
        );
    }
    let normalized_tag = normalize_release_tag(target_version)?;
    let (os, arch) = daemon_platform_slug()?;

    let clean_srv = server_url.trim().trim_end_matches('/');
    let unversioned_asset = format!("coda-daemon-{os}-{arch}.tar.gz");
    let srv_dl_asset = format!("daemon-{os}-{arch}.tar.gz");

    let (candidate_urls, checksum_urls, match_names): (Vec<String>, Vec<String>, Vec<String>) =
        if let Some(ref tag) = normalized_tag {
            let versioned_asset = format!("coda-daemon-{os}-{arch}-{tag}.tar.gz");
            (
                vec![format!(
                    "https://github.com/Annihilater/coda/releases/download/{tag}/{versioned_asset}"
                )],
                vec![format!(
                    "https://github.com/Annihilater/coda/releases/download/{tag}/SHA256SUMS-{tag}.txt"
                )],
                vec![versioned_asset, unversioned_asset],
            )
        } else {
            let mut dl = Vec::new();
            if clean_srv.starts_with("https://")
                || clean_srv.starts_with("http://127.0.0.1")
                || clean_srv.starts_with("http://localhost")
            {
                dl.push(format!("{clean_srv}/download/{srv_dl_asset}"));
            }
            dl.push(format!(
                "https://github.com/Annihilater/coda/releases/latest/download/{unversioned_asset}"
            ));
            (
                dl,
                vec![format!(
                    "https://github.com/Annihilater/coda/releases/latest/download/SHA256SUMS.txt"
                )],
                vec![unversioned_asset],
            )
        };

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(8))
        .user_agent(format!("coda-daemon-ota/{prev_ver}"))
        .build()?;

    let mut archive_bytes: Option<Vec<u8>> = None;
    let mut last_err: Option<anyhow::Error> = None;
    for url in &candidate_urls {
        match download_bytes_bounded(&http, url, MAX_OTA_ARCHIVE_BYTES).await {
            Ok(b) => {
                archive_bytes = Some(b);
                break;
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }
    let archive_bytes = archive_bytes.ok_or_else(|| {
        last_err.unwrap_or_else(|| anyhow!("no valid OTA download URL succeeded"))
    })?;

    for sum_url in &checksum_urls {
        if let Ok(resp) = http.get(sum_url).timeout(Duration::from_secs(8)).send().await {
            if resp.status().is_success() {
                if let Ok(sum_txt) = resp.text().await {
                    let refs: Vec<&str> = match_names.iter().map(String::as_str).collect();
                    let _ = verify_sha256_manifest(&sum_txt, &refs, &archive_bytes)?;
                    break;
                }
            }
        }
    }

    let work_dir = std::env::temp_dir().join(format!(
        "coda-ota-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&work_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&work_dir, std::fs::Permissions::from_mode(0o700));
    }

    let result: Result<(String, String)> = async {
        let tar_path = work_dir.join("daemon.tar.gz");
        std::fs::write(&tar_path, &archive_bytes)?;

        let unpack_dir = work_dir.join("unpack");
        std::fs::create_dir_all(&unpack_dir)?;
        let tar_status = tokio::process::Command::new("tar")
            .args([
                "-xzf",
                tar_path.to_str().unwrap_or_default(),
                "-C",
                unpack_dir.to_str().unwrap_or_default(),
            ])
            .status()
            .await
            .context("failed invoking `tar -xzf` for OTA archive")?;
        if !tar_status.success() {
            bail!("`tar -xzf` exited with {tar_status}");
        }

        let extracted_bin = find_daemon_binary_in_dir(&unpack_dir)
            .ok_or_else(|| anyhow!("`coda-daemon` binary not found inside OTA archive"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&extracted_bin, std::fs::Permissions::from_mode(0o755))?;
        }

        let smoke_out = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(&extracted_bin)
                .arg("--version")
                .output(),
        )
        .await
        .context("timed out running `--version` smoke test on new daemon binary")?
        .context("failed executing new daemon binary `--version`")?;
        let reported_ver = if smoke_out.status.success() {
            String::from_utf8_lossy(&smoke_out.stdout)
                .trim()
                .split_whitespace()
                .last()
                .unwrap_or("updated")
                .trim_start_matches('v')
                .to_string()
        } else {
            // Legacy daemon releases prior to v0.8.0 only supported `--help`.
            let help_out = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::process::Command::new(&extracted_bin)
                    .arg("--help")
                    .output(),
            )
            .await
            .context("timed out running `--help` smoke test on new daemon binary")?
            .context("failed executing new daemon binary `--help`")?;
            if !help_out.status.success() {
                bail!(
                    "new daemon binary failed `--version` ({}) and `--help` ({}) smoke checks",
                    smoke_out.status,
                    help_out.status
                );
            }
            normalized_tag
                .as_deref()
                .unwrap_or("latest")
                .trim_start_matches('v')
                .to_string()
        };

        let exe_parent = exe_path
            .parent()
            .ok_or_else(|| anyhow!("current_exe `{}` has no parent directory", exe_path.display()))?;
        let staged_path = exe_parent.join(format!(
            ".coda-daemon.ota.{}.{}",
            std::process::id(),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        let new_bytes = std::fs::read(&extracted_bin)?;
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o755);
            }
            let mut f = opts.open(&staged_path).with_context(|| {
                format!(
                    "cannot write staged OTA binary in `{}` (check file permissions)",
                    exe_parent.display()
                )
            })?;
            f.write_all(&new_bytes)?;
            f.sync_all()?;
        }
        if let Err(e) = std::fs::rename(&staged_path, &exe_path) {
            let _ = std::fs::remove_file(&staged_path);
            return Err(e).with_context(|| format!("failed replacing `{}`", exe_path.display()));
        }
        Ok((prev_ver, reported_ver))
    }
    .await;

    let _ = std::fs::remove_dir_all(&work_dir);
    result
}

fn find_daemon_binary_in_dir(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join("coda-daemon");
    if direct.is_file() {
        return Some(direct);
    }
    let rd = std::fs::read_dir(dir).ok()?;
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_file() && p.file_name().and_then(|s| s.to_str()) == Some("coda-daemon") {
            return Some(p);
        }
        if p.is_dir() {
            let nested = p.join("coda-daemon");
            if nested.is_file() {
                return Some(nested);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_release_tag_and_manifest_verification() {
        assert_eq!(normalize_release_tag(None).unwrap(), None);
        assert_eq!(normalize_release_tag(Some("  ")).unwrap(), None);
        assert_eq!(
            normalize_release_tag(Some("0.7.0")).unwrap(),
            Some("v0.7.0".to_string())
        );
        assert_eq!(
            normalize_release_tag(Some("v0.7.0-rc.1")).unwrap(),
            Some("v0.7.0-rc.1".to_string())
        );
        assert!(normalize_release_tag(Some("../etc/passwd")).is_err());
        assert!(normalize_release_tag(Some("v0.7")).is_err());
        assert!(normalize_release_tag(Some("v0.7.0;rm -rf /")).is_err());

        let payload = b"coda-ota-binary-test-payload";
        let sum = hex::encode(Sha256::digest(payload));
        let manifest = format!(
            "{sum}  coda-daemon-macos-aarch64-v0.7.0.tar.gz\n0000  other.tar.gz\n"
        );
        assert!(
            verify_sha256_manifest(
                &manifest,
                &["coda-daemon-macos-aarch64-v0.7.0.tar.gz"],
                payload
            )
            .unwrap()
        );
        assert!(
            verify_sha256_manifest(
                &manifest,
                &["coda-daemon-macos-aarch64-v0.7.0.tar.gz"],
                b"tampered"
            )
            .is_err()
        );

        assert!(is_standalone_daemon_binary(Path::new("/usr/local/bin/coda-daemon")));
        assert!(!is_standalone_daemon_binary(Path::new(
            "/Applications/Coda.app/Contents/MacOS/coda-desktop"
        )));
    }

    #[test]
    fn test_collect_host_health_snapshot_populates_live_metrics() {
        let snap = collect_host_health_snapshot(Instant::now(), Some("0.1.21".into()));
        assert!(!snap.daemon_version.is_empty());
        assert!(snap.cpu_cores >= 1);
        assert_eq!(snap.herdr_version.as_deref(), Some("0.1.21"));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            assert!(snap.load_avg_1m.is_some());
            assert!(snap.mem_total_mb.unwrap_or(0) > 128);
            assert!(snap.os_uptime_secs.unwrap_or(0) > 0);
        }
    }
}
