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
const MAX_SHA256SUMS_BYTES: usize = 512 * 1024;
const DEFAULT_GITHUB_REPO: &str = "Annihilater/coda";

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

fn is_allowed_ota_origin(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.starts_with("https://")
        || trimmed.starts_with("http://127.0.0.1")
        || trimmed.starts_with("http://localhost")
}

fn parse_checksum_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("SHA256 (") {
        if let Some((fname, eq_hex)) = rest.split_once(") =") {
            let hex_s = eq_hex.trim().to_ascii_lowercase();
            let base = Path::new(fname.trim())
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(fname.trim())
                .to_string();
            return Some((hex_s, base));
        }
    }
    let mut parts = trimmed.split_whitespace();
    let expected_hex = parts.next()?.trim().to_ascii_lowercase();
    let file_col = parts.next()?.trim().trim_start_matches('*');
    let base_col = Path::new(file_col)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(file_col)
        .to_string();
    Some((expected_hex, base_col))
}

pub fn verify_sha256_manifest(
    manifest_text: &str,
    asset_names: &[&str],
    actual_bytes: &[u8],
) -> Result<bool> {
    let actual_hex = hex::encode(Sha256::digest(actual_bytes));
    for line in manifest_text.lines() {
        let Some((expected_hex, base_col)) = parse_checksum_line(line) else {
            continue;
        };
        if asset_names.contains(&base_col.as_str()) {
            if expected_hex.len() != 64 || !expected_hex.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("malformed SHA-256 digest in SHA256SUMS for `{base_col}`");
            }
            if expected_hex == actual_hex {
                return Ok(true);
            }
            bail!(
                "SHA-256 checksum mismatch for `{base_col}` (expected {expected_hex}, got {actual_hex})"
            );
        }
    }
    Ok(false)
}

/// Strictly verifies `actual_bytes` against `manifest_text` (fail-closed).
/// Rejects empty/HTML manifests, missing target entries, malformed digests, and hash mismatches.
pub fn verify_sha256_manifest_strict(
    manifest_text: &str,
    asset_names: &[&str],
    actual_bytes: &[u8],
) -> Result<()> {
    let trimmed = manifest_text.trim();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        bail!("SHA256SUMS manifest is empty or HTML");
    }
    if asset_names.is_empty() {
        bail!("no expected asset names provided for SHA256SUMS verification");
    }
    if !verify_sha256_manifest(trimmed, asset_names, actual_bytes)? {
        bail!(
            "OTA aborted (fail-closed): target archive ({}) missing from SHA256SUMS manifest",
            asset_names.join(" / ")
        );
    }
    Ok(())
}

/// Extracts and validates a `vX.Y.Z` tag from a GitHub `/releases/latest` redirect `Location` header.
pub fn extract_release_tag_from_location(location: &str) -> Option<String> {
    let (_, after) = location.split_once("/releases/tag/")?;
    let raw_tag = after
        .split(['?', '#', '/'])
        .next()
        .unwrap_or(after)
        .trim();
    normalize_release_tag(Some(raw_tag)).ok().flatten()
}

/// Extracts and validates a `vX.Y.Z` tag from a resolved `/releases/download/<tag>/<asset>` URL.
pub fn extract_release_tag_from_download_url(url: &str) -> Option<String> {
    let (_, after) = url.split_once("/releases/download/")?;
    let raw_tag = after.split('/').next()?.trim();
    normalize_release_tag(Some(raw_tag)).ok().flatten()
}

fn semver_is_newer(candidate: &str, baseline: &str) -> bool {
    fn parse_key(raw: &str) -> Option<(u64, u64, u64, Option<String>)> {
        let ver = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
        let (core, pre) = match ver.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (ver, None),
        };
        let mut nums = core.split('.');
        let maj = nums.next()?.parse::<u64>().ok()?;
        let min = nums.next()?.parse::<u64>().ok()?;
        let pat = nums.next()?.parse::<u64>().ok()?;
        if nums.next().is_some() {
            return None;
        }
        Some((maj, min, pat, pre))
    }

    let (Some((amaj, amin, apat, apre)), Some((bmaj, bmin, bpat, bpre))) =
        (parse_key(candidate), parse_key(baseline))
    else {
        return false;
    };
    match (amaj, amin, apat).cmp(&(bmaj, bmin, bpat)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (apre, bpre) {
            (None, Some(_)) => true,
            (Some(_), None) | (None, None) => false,
            (Some(a), Some(b)) => a > b,
        },
    }
}

async fn fetch_site_info_tag(http: &reqwest::Client, clean_srv: &str) -> Option<String> {
    if !is_allowed_ota_origin(clean_srv) {
        return None;
    }
    let url = format!("{clean_srv}/api/v1/public/site-info");
    let resp = http.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let txt = resp.text().await.ok()?;
    if txt.trim_start().starts_with('<') {
        return None;
    }
    let val: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let raw_ver = val.get("version").and_then(|v| v.as_str())?;
    normalize_release_tag(Some(raw_ver)).ok().flatten()
}

async fn fetch_github_latest_tag(latest_url: &str, prev_ver: &str) -> Option<String> {
    if !is_allowed_ota_origin(latest_url) {
        return None;
    }
    let no_redir = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(format!("coda-daemon-ota/{prev_ver}"))
        .build()
        .ok()?;

    if let Ok(resp) = no_redir.head(latest_url).send().await {
        if let Some(loc) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        {
            if let Some(tag) = extract_release_tag_from_location(loc) {
                return Some(tag);
            }
        }
    }
    if let Ok(resp) = no_redir.get(latest_url).send().await {
        if let Some(loc) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        {
            if let Some(tag) = extract_release_tag_from_location(loc) {
                return Some(tag);
            }
        }
    }
    None
}

/// Resolves the concrete release tag (`vX.Y.Z`) and whether `clean_srv` matches that tag when
/// `target_version` was not explicitly specified.
pub async fn resolve_target_ota_tag_with_endpoints(
    http: &reqwest::Client,
    clean_srv: &str,
    github_latest_url: Option<&str>,
) -> Result<(String, bool)> {
    let prev_ver = env!("CARGO_PKG_VERSION");
    let site_tag = fetch_site_info_tag(http, clean_srv).await;
    let gh_tag = match github_latest_url {
        Some(url) => fetch_github_latest_tag(url, prev_ver).await,
        None => None,
    };

    match (site_tag, gh_tag) {
        (Some(s), Some(g)) => {
            if s == g {
                Ok((g, true))
            } else if semver_is_newer(&s, &g) {
                Ok((s, true))
            } else {
                Ok((g, false))
            }
        }
        (None, Some(g)) => Ok((g, false)),
        (Some(s), None) => Ok((s, true)),
        (None, None) => bail!(
            "failed resolving latest release tag from `{clean_srv}/api/v1/public/site-info` or GitHub Releases"
        ),
    }
}

/// Builds version-aligned `(candidate_archive_urls, checksum_urls, match_names)` for a concrete `tag`.
pub fn build_ota_plan_for_tag(
    clean_srv: &str,
    github_download_base: &str,
    os: &str,
    arch: &str,
    tag: &str,
    server_matches_tag: bool,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let versioned_asset = format!("coda-daemon-{os}-{arch}-{tag}.tar.gz");
    let unversioned_asset = format!("coda-daemon-{os}-{arch}.tar.gz");
    let srv_dl_asset = format!("daemon-{os}-{arch}.tar.gz");
    let clean_gh = github_download_base.trim().trim_end_matches('/');

    let mut candidate_urls = Vec::new();
    let mut checksum_urls = Vec::new();
    if server_matches_tag && is_allowed_ota_origin(clean_srv) {
        candidate_urls.push(format!("{clean_srv}/download/{srv_dl_asset}"));
        checksum_urls.push(format!("{clean_srv}/download/checksums.txt"));
    }
    if is_allowed_ota_origin(clean_gh) {
        candidate_urls.push(format!("{clean_gh}/{tag}/{versioned_asset}"));
        checksum_urls.push(format!("{clean_gh}/{tag}/SHA256SUMS-{tag}.txt"));
        checksum_urls.push(format!("{clean_gh}/{tag}/SHA256SUMS.txt"));
    }

    (
        candidate_urls,
        checksum_urls,
        vec![versioned_asset, unversioned_asset],
    )
}

async fn download_bytes_bounded(
    http: &reqwest::Client,
    url: &str,
    max_bytes: usize,
) -> Result<(Vec<u8>, String)> {
    let mut resp = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?
        .error_for_status()
        .with_context(|| format!("HTTP error from {url}"))?;

    let final_url = resp.url().as_str().to_string();
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
        bail!(
            "downloaded OTA archive from {url} is unexpectedly small ({} bytes)",
            out.len()
        );
    }
    Ok((out, final_url))
}

async fn download_manifest_bounded(http: &reqwest::Client, url: &str) -> Result<String> {
    let mut resp = http
        .get(url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .with_context(|| format!("GET {url} failed"))?
        .error_for_status()
        .with_context(|| format!("HTTP error from {url}"))?;

    if let Some(len) = resp.content_length() {
        if len > MAX_SHA256SUMS_BYTES as u64 {
            bail!("SHA256SUMS from {url} exceeds {MAX_SHA256SUMS_BYTES} bytes");
        }
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len().saturating_add(chunk.len()) > MAX_SHA256SUMS_BYTES {
            bail!("SHA256SUMS stream from {url} exceeded {MAX_SHA256SUMS_BYTES} bytes");
        }
        out.extend_from_slice(&chunk);
    }
    String::from_utf8(out).with_context(|| format!("SHA256SUMS from {url} is not valid UTF-8"))
}

/// Fetches `SHA256SUMS` from `checksum_urls` and strictly verifies `archive_bytes` (fail-closed).
pub async fn fetch_and_verify_sha256_sums(
    http: &reqwest::Client,
    checksum_urls: &[String],
    match_names: &[String],
    archive_bytes: &[u8],
) -> Result<()> {
    if checksum_urls.is_empty() {
        bail!("OTA aborted (fail-closed): no SHA256SUMS URLs configured");
    }
    let refs: Vec<&str> = match_names.iter().map(String::as_str).collect();
    let mut last_err: Option<String> = None;

    for sum_url in checksum_urls {
        match download_manifest_bounded(http, sum_url).await {
            Ok(sum_txt) => match verify_sha256_manifest_strict(&sum_txt, &refs, archive_bytes) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("SHA-256 checksum mismatch")
                        || msg.contains("malformed SHA-256 digest")
                    {
                        return Err(e);
                    }
                    last_err = Some(format!("{sum_url}: {msg}"));
                }
            },
            Err(e) => {
                last_err = Some(e.to_string());
            }
        }
    }

    bail!(
        "OTA aborted (fail-closed): unable to verify SHA256SUMS for {} ({})",
        match_names.join(" / "),
        last_err.unwrap_or_else(|| "all SHA256SUMS endpoints failed".to_string())
    )
}

/// Resolves the concrete release tag, downloads the daemon archive, strictly verifies `SHA256SUMS`
/// (fail-closed), extracts, smoke-tests (`--version`), and atomically replaces the running
/// `coda-daemon` executable in place.
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
    let gh_dl_base = format!("https://github.com/{DEFAULT_GITHUB_REPO}/releases/download");
    let gh_latest_url = format!("https://github.com/{DEFAULT_GITHUB_REPO}/releases/latest");

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(8))
        .user_agent(format!("coda-daemon-ota/{prev_ver}"))
        .build()?;
    let probe_http = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(format!("coda-daemon-ota/{prev_ver}"))
        .build()?;

    let (mut resolved_tag, server_matches_tag) = if let Some(tag) = normalized_tag {
        let loopback_srv = clean_srv.starts_with("http://127.0.0.1")
            || clean_srv.starts_with("http://localhost");
        let srv_matches = if loopback_srv {
            fetch_site_info_tag(&probe_http, clean_srv).await.as_deref() == Some(tag.as_str())
        } else {
            false
        };
        (tag, srv_matches)
    } else {
        resolve_target_ota_tag_with_endpoints(&probe_http, clean_srv, Some(&gh_latest_url)).await?
    };

    let (candidate_urls, mut checksum_urls, mut match_names) = build_ota_plan_for_tag(
        clean_srv,
        &gh_dl_base,
        os,
        arch,
        &resolved_tag,
        server_matches_tag,
    );

    let mut archive_bytes: Option<Vec<u8>> = None;
    let mut final_dl_url = String::new();
    let mut last_err: Option<anyhow::Error> = None;
    for url in &candidate_urls {
        match download_bytes_bounded(&http, url, MAX_OTA_ARCHIVE_BYTES).await {
            Ok((b, resolved_url)) => {
                archive_bytes = Some(b);
                final_dl_url = resolved_url;
                break;
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }
    let archive_bytes = archive_bytes
        .ok_or_else(|| last_err.unwrap_or_else(|| anyhow!("no valid OTA download URL succeeded")))?;

    if let Some(redirected_tag) = extract_release_tag_from_download_url(&final_dl_url) {
        if redirected_tag != resolved_tag {
            let (_, aligned_sums, aligned_names) = build_ota_plan_for_tag(
                clean_srv,
                &gh_dl_base,
                os,
                arch,
                &redirected_tag,
                server_matches_tag,
            );
            checksum_urls = aligned_sums;
            match_names = aligned_names;
            resolved_tag = redirected_tag;
        }
    }

    fetch_and_verify_sha256_sums(&http, &checksum_urls, &match_names, &archive_bytes).await?;

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
            resolved_tag.trim_start_matches('v').to_string()
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
        let other_sum = hex::encode(Sha256::digest(b"other-archive"));
        let manifest = format!(
            "{sum}  coda-daemon-macos-aarch64-v0.7.0.tar.gz\n{other_sum}  other.tar.gz\n"
        );
        assert!(
            verify_sha256_manifest(
                &manifest,
                &["coda-daemon-macos-aarch64-v0.7.0.tar.gz"],
                payload
            )
            .unwrap()
        );
        verify_sha256_manifest_strict(
            &manifest,
            &["coda-daemon-macos-aarch64-v0.7.0.tar.gz"],
            payload,
        )
        .unwrap();

        // Missing filename in SHA256SUMS returns Ok(false) in low-level parser,
        // and MUST fail closed (Err) in strict verification!
        assert!(
            !verify_sha256_manifest(
                &manifest,
                &["coda-daemon-linux-x86_64-v0.7.0.tar.gz", "coda-daemon-linux-x86_64.tar.gz"],
                payload
            )
            .unwrap()
        );
        let missing_err = verify_sha256_manifest_strict(
            &manifest,
            &["coda-daemon-linux-x86_64-v0.7.0.tar.gz", "coda-daemon-linux-x86_64.tar.gz"],
            payload,
        )
        .unwrap_err()
        .to_string();
        assert!(
            missing_err.contains("fail-closed") && missing_err.contains("missing from SHA256SUMS"),
            "unexpected missing-entry error: {missing_err}"
        );

        // Empty or HTML fallback pages MUST fail closed!
        assert!(
            verify_sha256_manifest_strict(
                "",
                &["coda-daemon-macos-aarch64-v0.7.0.tar.gz"],
                payload
            )
            .is_err()
        );
        assert!(
            verify_sha256_manifest_strict(
                "<!doctype html><html><body>Not Found</body></html>",
                &["coda-daemon-macos-aarch64-v0.7.0.tar.gz"],
                payload
            )
            .is_err()
        );

        // Tampered payload MUST fail closed!
        assert!(
            verify_sha256_manifest(
                &manifest,
                &["coda-daemon-macos-aarch64-v0.7.0.tar.gz"],
                b"tampered"
            )
            .is_err()
        );
        assert!(
            verify_sha256_manifest_strict(
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

    #[tokio::test]
    async fn test_fail_closed_ota_checksum_fetch_and_tag_alignment() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let payload = b"coda-daemon-ota-v1-1-1-archive-bytes";
        let payload_sum = hex::encode(Sha256::digest(payload));
        let other_sum = hex::encode(Sha256::digest(b"unrelated"));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let payload_sum = payload_sum.clone();
                let other_sum = other_sum.clone();
                tokio::spawn(async move {
                    let mut buf = [0_u8; 2048];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                    let resp = match path {
                        "/api/v1/public/site-info" => {
                            let body = r#"{"service":"coda-server","version":"v1.1.0"}"#;
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                        }
                        "/gh-latest" => {
                            "HTTP/1.1 302 Found\r\nLocation: https://github.com/Annihilater/coda/releases/tag/v1.1.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                        }
                        "/sums-missing.txt" => {
                            let body = format!("{other_sum}  coda-server-linux-x86_64-v1.1.1.tar.gz\n");
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                        }
                        "/sums-valid.txt" => {
                            let body = format!(
                                "{payload_sum}  coda-daemon-linux-x86_64-v1.1.1.tar.gz\n{payload_sum}  coda-daemon-linux-x86_64.tar.gz\n"
                            );
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                        }
                        _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found".to_string(),
                    };
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let match_names = vec![
            "coda-daemon-linux-x86_64-v1.1.1.tar.gz".to_string(),
            "coda-daemon-linux-x86_64.tar.gz".to_string(),
        ];

        // 1. Fail-closed when all SHA256SUMS URLs return 404 / network failure
        let err_404 = fetch_and_verify_sha256_sums(
            &http,
            &[format!("{base_url}/does-not-exist")],
            &match_names,
            payload,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err_404.contains("fail-closed"), "got: {err_404}");

        // 2. Fail-closed when SHA256SUMS returns 200 OK without a matching daemon filename
        let err_missing = fetch_and_verify_sha256_sums(
            &http,
            &[format!("{base_url}/sums-missing.txt")],
            &match_names,
            payload,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err_missing.contains("fail-closed") && err_missing.contains("missing from SHA256SUMS"),
            "got: {err_missing}"
        );

        // 3. Succeeds when fallback SHA256SUMS URL contains a valid matching entry, and rejects tampered bytes
        fetch_and_verify_sha256_sums(
            &http,
            &[
                format!("{base_url}/sums-missing.txt"),
                format!("{base_url}/sums-valid.txt"),
            ],
            &match_names,
            payload,
        )
        .await
        .unwrap();

        let err_tampered = fetch_and_verify_sha256_sums(
            &http,
            &[format!("{base_url}/sums-valid.txt")],
            &match_names,
            b"corrupted-archive-bytes",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err_tampered.contains("SHA-256 checksum mismatch"),
            "got: {err_tampered}"
        );

        // 4. Tag resolution when server lags (`v1.1.0`) behind GitHub `/releases/latest` (`v1.1.1`):
        //    must pick `v1.1.1` with `server_matches_tag = false` so archive URL and SHA256SUMS URL both target `v1.1.1`!
        let (resolved_tag, srv_matches) = resolve_target_ota_tag_with_endpoints(
            &http,
            &base_url,
            Some(&format!("{base_url}/gh-latest")),
        )
        .await
        .unwrap();
        assert_eq!(resolved_tag, "v1.1.1");
        assert!(!srv_matches);

        let (dl_urls, sum_urls, names) = build_ota_plan_for_tag(
            &base_url,
            "https://github.com/Annihilater/coda/releases/download",
            "linux",
            "x86_64",
            &resolved_tag,
            srv_matches,
        );
        assert_eq!(
            dl_urls,
            vec!["https://github.com/Annihilater/coda/releases/download/v1.1.1/coda-daemon-linux-x86_64-v1.1.1.tar.gz"]
        );
        assert_eq!(
            sum_urls,
            vec![
                "https://github.com/Annihilater/coda/releases/download/v1.1.1/SHA256SUMS-v1.1.1.txt",
                "https://github.com/Annihilater/coda/releases/download/v1.1.1/SHA256SUMS.txt",
            ]
        );
        assert_eq!(names, match_names);

        // 5. Tag resolution when GitHub is unreachable falls back cleanly to server `site-info` (`v1.1.0`, `srv_matches = true`)
        let (srv_only_tag, srv_only_matches) =
            resolve_target_ota_tag_with_endpoints(&http, &base_url, None)
                .await
                .unwrap();
        assert_eq!(srv_only_tag, "v1.1.0");
        assert!(srv_only_matches);
        let (srv_dl, srv_sums, _) = build_ota_plan_for_tag(
            &base_url,
            "https://github.com/Annihilater/coda/releases/download",
            "linux",
            "x86_64",
            &srv_only_tag,
            srv_only_matches,
        );
        assert_eq!(srv_dl[0], format!("{base_url}/download/daemon-linux-x86_64.tar.gz"));
        assert_eq!(srv_sums[0], format!("{base_url}/download/checksums.txt"));
        assert_eq!(
            srv_sums[1],
            "https://github.com/Annihilater/coda/releases/download/v1.1.0/SHA256SUMS-v1.1.0.txt"
        );

        assert_eq!(
            extract_release_tag_from_download_url(
                "https://github.com/Annihilater/coda/releases/download/v1.1.1/coda-daemon-macos-aarch64-v1.1.1.tar.gz"
            ),
            Some("v1.1.1".to_string())
        );

        server_task.abort();
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
