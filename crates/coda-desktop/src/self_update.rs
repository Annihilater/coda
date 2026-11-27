//! In-app one-click self-updater for `Coda.app` (`coda-desktop`).
//!
//! Checks `/api/v1/public/site-info` (and `/api/v1/site-info`) with fallback to GitHub Releases
//! `/releases/latest` redirect tag against `env!("CARGO_PKG_VERSION")`, strictly verifies the
//! downloaded `coda-macos-aarch64-vX.Y.Z.tar.gz` SHA-256 hex digest against `SHA256SUMS`, stages
//! the extracted binary next to `resolve_clean_exe_path()` (`Contents/MacOS/.coda-desktop.new-<pid>`),
//! strips `com.apple.quarantine`, runs `<staged_bin> --version`, atomically `rename()`s in place,
//! updates `Contents/Info.plist`, re-signs `<AppRoot>.app` ad-hoc (`codesign --force --deep --sign -`),
//! and relaunches `Coda.app` cleanly.

use std::cmp::Ordering;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::is_safe_http_url;

pub const CURRENT_DESKTOP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_GITHUB_REPO: &str = "Annihilater/coda";
const MAX_DESKTOP_UPDATE_ARCHIVE_BYTES: usize = 96 * 1024 * 1024;
const MAX_CHECKSUM_MANIFEST_BYTES: usize = 512 * 1024;

/// UI state for the in-app self-updater shown in the top header bar pill and Settings card.
#[derive(Debug, Clone)]
pub struct SelfUpdateUiState {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub server_matches_latest: bool,
    pub update_available: bool,
    pub checking: bool,
    pub updating: bool,
    pub progress_message: Option<String>,
    pub error_message: Option<String>,
}

impl Default for SelfUpdateUiState {
    fn default() -> Self {
        Self {
            current_version: CURRENT_DESKTOP_VERSION.to_string(),
            latest_version: None,
            server_matches_latest: false,
            update_available: false,
            checking: false,
            updating: false,
            progress_message: None,
            error_message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopUpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub server_matches_latest: bool,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PrereleaseIdent {
    Numeric(u64),
    AlphaNumeric(String),
}

impl Ord for PrereleaseIdent {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(a), Self::Numeric(b)) => a.cmp(b),
            // Per SemVer 2.0.0 §11.4.1: numeric identifiers always have lower precedence than non-numeric.
            (Self::Numeric(_), Self::AlphaNumeric(_)) => Ordering::Less,
            (Self::AlphaNumeric(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::AlphaNumeric(a), Self::AlphaNumeric(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PrereleaseIdent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSemver {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    prerelease: Vec<PrereleaseIdent>,
}

impl Ord for ParsedSemver {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)) {
            Ordering::Equal => {}
            non_eq => return non_eq,
        }
        match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
            (true, true) => Ordering::Equal,
            // Normal release has higher precedence than a prerelease of the same M.m.p.
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => self.prerelease.cmp(&other.prerelease),
        }
    }
}

impl PartialOrd for ParsedSemver {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Parses a semantic version string (with optional leading `v`/`V`, `-prerelease`, and `+build`).
pub fn parse_semver(raw: &str) -> Option<ParsedSemver> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return None;
    }
    let no_v = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let without_build = no_v.split('+').next().unwrap_or(no_v);
    if let Some((_, build)) = no_v.split_once('+') {
        if build.is_empty()
            || !build
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            return None;
        }
    }

    let (core, pre_opt) = match without_build.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (without_build, None),
    };

    let mut nums = core.split('.');
    let major_s = nums.next()?;
    let minor_s = nums.next()?;
    let patch_s = nums.next()?;
    if nums.next().is_some() {
        return None;
    }
    for part in [major_s, minor_s, patch_s] {
        if part.is_empty() || part.len() > 9 || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    let major = major_s.parse::<u64>().ok()?;
    let minor = minor_s.parse::<u64>().ok()?;
    let patch = patch_s.parse::<u64>().ok()?;

    let mut prerelease = Vec::new();
    if let Some(pre) = pre_opt {
        if pre.is_empty() {
            return None;
        }
        for seg in pre.split('.') {
            if seg.is_empty()
                || !seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return None;
            }
            if seg.chars().all(|c| c.is_ascii_digit()) {
                prerelease.push(PrereleaseIdent::Numeric(seg.parse::<u64>().ok()?));
            } else {
                prerelease.push(PrereleaseIdent::AlphaNumeric(seg.to_string()));
            }
        }
    }

    Some(ParsedSemver {
        major,
        minor,
        patch,
        prerelease,
    })
}

/// Returns `true` strictly when `latest` parses as a valid SemVer strictly newer than `current`.
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let (Some(lat), Some(cur)) = (parse_semver(latest), parse_semver(current)) else {
        return false;
    };
    lat > cur
}

/// Validates and normalizes a release version tag to `"vX.Y.Z"` (preserving any valid `-prerelease`).
pub fn normalize_version_tag(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let _parsed = parse_semver(trimmed)?;
    let no_v = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let without_build = no_v.split('+').next().unwrap_or(no_v);
    if !without_build
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return None;
    }
    Some(format!("v{without_build}"))
}

/// Extracts and validates a `vX.Y.Z` tag from a GitHub `/releases/latest` redirect `Location` header.
pub fn extract_github_release_tag_from_location(location: &str) -> Option<String> {
    let (_, after) = location.split_once("/releases/tag/")?;
    let raw_tag = after
        .split(['?', '#', '/'])
        .next()
        .unwrap_or(after)
        .trim();
    normalize_version_tag(raw_tag)
}

/// Resolves the clean on-disk path of the running `coda-desktop` executable.
pub fn resolve_clean_exe_path() -> Result<PathBuf, String> {
    coda_daemon::telemetry_ota::resolve_clean_exe_path().map_err(|e| e.to_string())
}

/// Detects the enclosing `<Name>.app` bundle root when `exe_path` resides at
/// `.../<Name>.app/Contents/MacOS/<executable>`.
pub fn detect_macos_app_bundle_root(exe_path: &Path) -> Option<PathBuf> {
    let macos_dir = exe_path.parent()?;
    if macos_dir.file_name().and_then(|s| s.to_str()) != Some("MacOS") {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name().and_then(|s| s.to_str()) != Some("Contents") {
        return None;
    }
    let app_dir = contents_dir.parent()?;
    let ext = app_dir.extension().and_then(|s| s.to_str())?;
    if !ext.eq_ignore_ascii_case("app") {
        return None;
    }
    let stem = app_dir.file_stem().and_then(|s| s.to_str())?;
    if stem.trim().is_empty() {
        return None;
    }
    Some(app_dir.to_path_buf())
}

/// Computes the sibling staging binary path next to `exe_path` (`Contents/MacOS/.coda-desktop.new-<pid>`).
pub fn staged_executable_path_for(exe_path: &Path, pid: u32) -> Result<PathBuf, String> {
    let parent = exe_path
        .parent()
        .ok_or_else(|| format!("Executable `{}` has no parent directory", exe_path.display()))?;
    Ok(parent.join(format!(".coda-desktop.new-{pid}")))
}

fn parse_checksum_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    // BSD style: `SHA256 (filename.tar.gz) = <64-hex>`
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
    // GNU / coreutils style: `<64-hex>  <filename>` or `<64-hex> *<filename>`
    let mut parts = trimmed.split_whitespace();
    let hex_part = parts.next()?.trim().to_ascii_lowercase();
    let file_part = parts.next()?.trim().trim_start_matches('*');
    let base = Path::new(file_part)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(file_part)
        .to_string();
    Some((hex_part, base))
}

fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Strictly verifies `archive_bytes` against `SHA256SUMS` manifest text.
///
/// Aborts with `Err` if:
/// - `manifest_text` is empty or HTML,
/// - none of `expected_asset_names` appears in `SHA256SUMS`,
/// - the listed hex digest is malformed, OR
/// - the computed SHA-256 hex digest does not match!
pub fn verify_sha256_sums_strict(
    manifest_text: &str,
    expected_asset_names: &[&str],
    archive_bytes: &[u8],
) -> Result<String, String> {
    let trimmed = manifest_text.trim();
    if trimmed.is_empty() || trimmed.starts_with('<') {
        return Err("SHA256SUMS manifest is empty or invalid".to_string());
    }
    if expected_asset_names.is_empty() {
        return Err("No expected asset name specified for SHA256SUMS check".to_string());
    }

    let actual_hex = hex::encode(Sha256::digest(archive_bytes));
    for line in trimmed.lines() {
        let Some((expected_hex, base_name)) = parse_checksum_line(line) else {
            continue;
        };
        if expected_asset_names.contains(&base_name.as_str()) {
            if !is_valid_sha256_hex(&expected_hex) {
                return Err(format!(
                    "Malformed SHA-256 digest in SHA256SUMS for `{base_name}`"
                ));
            }
            if expected_hex != actual_hex {
                return Err(format!(
                    "SHA-256 checksum mismatch for `{base_name}` (expected {expected_hex}, got {actual_hex})"
                ));
            }
            return Ok(actual_hex);
        }
    }

    Err(format!(
        "Target archive ({}) missing from SHA256SUMS manifest; aborting update",
        expected_asset_names.join(" / ")
    ))
}

/// Returns ordered `site-info` probe URLs, querying `/api/v1/public/site-info` BEFORE
/// `/api/v1/site-info` so `coda-server` responds 200 on the first request without a 404 probe.
pub fn build_site_info_probe_urls(server_url: &str) -> Vec<String> {
    let base = server_url.trim().trim_end_matches('/');
    if !is_safe_http_url(base) {
        return Vec::new();
    }
    ["/api/v1/public/site-info", "/api/v1/site-info"]
        .into_iter()
        .map(|route| format!("{base}{route}"))
        .collect()
}

async fn fetch_site_info_version(http: &reqwest::Client, server_url: &str) -> Option<String> {
    for url in build_site_info_probe_urls(server_url) {
        let Ok(resp) = http.get(&url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(txt) = resp.text().await else {
            continue;
        };
        if txt.trim_start().starts_with('<') {
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&txt) else {
            continue;
        };
        if let Some(raw_ver) = val.get("version").and_then(|v| v.as_str()) {
            if let Some(tag) = normalize_version_tag(raw_ver) {
                return Some(tag);
            }
        }
    }
    None
}

async fn fetch_github_latest_tag(latest_url: &str) -> Option<String> {
    if !is_safe_http_url(latest_url) {
        return None;
    }
    let no_redir = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(format!("coda-desktop-updater/{CURRENT_DESKTOP_VERSION}"))
        .build()
        .ok()?;

    if let Ok(resp) = no_redir.head(latest_url).send().await {
        if let Some(loc) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        {
            if let Some(tag) = extract_github_release_tag_from_location(loc) {
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
            if let Some(tag) = extract_github_release_tag_from_location(loc) {
                return Some(tag);
            }
        }
    }
    None
}

/// Checks `/api/v1/site-info` (and `/api/v1/public/site-info`) with fallback to GitHub Releases
/// `/releases/latest` redirect tag against `current_version`.
pub async fn check_for_desktop_update(
    server_url: &str,
    current_version: &str,
) -> Result<DesktopUpdateCheckResult, String> {
    let gh_latest = format!("https://github.com/{DEFAULT_GITHUB_REPO}/releases/latest");
    check_for_desktop_update_with_endpoints(server_url, Some(&gh_latest), current_version).await
}

pub async fn check_for_desktop_update_with_endpoints(
    server_url: &str,
    github_latest_url: Option<&str>,
    current_version: &str,
) -> Result<DesktopUpdateCheckResult, String> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(format!("coda-desktop-updater/{current_version}"))
        .build()
        .map_err(|e| e.to_string())?;

    let site_tag = fetch_site_info_version(&http, server_url).await;
    if let Some(ref tag) = site_tag {
        if is_newer_version(tag, current_version) {
            return Ok(DesktopUpdateCheckResult {
                current_version: current_version.to_string(),
                latest_version: tag.clone(),
                update_available: true,
                server_matches_latest: true,
                source: "site-info",
            });
        }
    }

    if let Some(gh_url) = github_latest_url {
        if let Some(gh_tag) = fetch_github_latest_tag(gh_url).await {
            let server_matches = site_tag.as_deref() == Some(gh_tag.as_str());
            let newer = is_newer_version(&gh_tag, current_version);
            return Ok(DesktopUpdateCheckResult {
                current_version: current_version.to_string(),
                latest_version: gh_tag,
                update_available: newer,
                server_matches_latest: server_matches,
                source: "github-releases",
            });
        }
    }

    if let Some(tag) = site_tag {
        return Ok(DesktopUpdateCheckResult {
            current_version: current_version.to_string(),
            update_available: is_newer_version(&tag, current_version),
            latest_version: tag,
            server_matches_latest: true,
            source: "site-info",
        });
    }

    Err("Could not reach server site-info or GitHub Releases".to_string())
}

fn is_allowed_download_origin(url: &str) -> bool {
    let trimmed = url.trim();
    trimmed.starts_with("https://")
        || trimmed.starts_with("http://127.0.0.1")
        || trimmed.starts_with("http://localhost")
}

async fn download_bounded_bytes(
    http: &reqwest::Client,
    url: &str,
    max_bytes: usize,
    min_bytes: usize,
) -> Result<Vec<u8>, String> {
    let mut resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url} failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("HTTP error from {url}: {e}"))?;

    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(format!("Response from {url} exceeds {max_bytes} bytes"));
        }
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if out.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("Stream from {url} exceeded {max_bytes} bytes"));
        }
        out.extend_from_slice(&chunk);
    }
    if out.len() < min_bytes {
        return Err(format!(
            "Downloaded payload from {url} is unexpectedly small ({} bytes)",
            out.len()
        ));
    }
    Ok(out)
}

fn run_process_bounded(
    mut cmd: std::process::Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    use std::io::Read;

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let out_handle = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(ref mut r) = stdout_pipe {
            let _ = r.take(16 * 1024).read_to_end(&mut b);
        }
        b
    });
    let err_handle = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(ref mut r) = stderr_pipe {
            let _ = r.take(16 * 1024).read_to_end(&mut b);
        }
        b
    });

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
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
                    return Err("Process smoke check timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
    }
}

fn strip_macos_quarantine_best_effort(target: &Path) {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("/usr/bin/xattr");
        cmd.args(["-dr", "com.apple.quarantine"]).arg(target);
        let _ = run_process_bounded(cmd, Duration::from_secs(4));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = target;
    }
}

fn replace_plist_key_string_value(xml: &str, key_name: &str, new_val: &str) -> Option<String> {
    let key_tag = format!("<key>{key_name}</key>");
    let key_pos = xml.find(&key_tag)?;
    let after_key = key_pos + key_tag.len();
    let rest = &xml[after_key..];
    let trimmed_ws_len = rest.len() - rest.trim_start().len();
    let after_ws = &rest[trimmed_ws_len..];
    let inner = after_ws.strip_prefix("<string>")?;
    let close_offset = inner.find("</string>")?;
    let replace_start = after_key + trimmed_ws_len + "<string>".len();
    let replace_end = replace_start + close_offset;
    let mut out = String::with_capacity(xml.len() + new_val.len());
    out.push_str(&xml[..replace_start]);
    out.push_str(new_val);
    out.push_str(&xml[replace_end..]);
    Some(out)
}

/// Pure XML transformer that updates `CFBundleShortVersionString` and `CFBundleVersion` in a
/// macOS `Info.plist` document to `target_tag.trim_start_matches('v')`.
pub fn patch_plist_xml_bundle_versions(
    plist_xml: &str,
    target_tag_or_version: &str,
) -> Result<String, String> {
    let normalized = normalize_version_tag(target_tag_or_version).ok_or_else(|| {
        format!("Invalid target version `{target_tag_or_version}` for Info.plist update")
    })?;
    let short_ver = normalized.trim_start_matches('v');
    let mut updated = plist_xml.to_string();
    let mut patched_any = false;
    for key in ["CFBundleShortVersionString", "CFBundleVersion"] {
        if let Some(next) = replace_plist_key_string_value(&updated, key, short_ver) {
            updated = next;
            patched_any = true;
        }
    }
    if !patched_any {
        return Err(
            "Info.plist is missing CFBundleShortVersionString / CFBundleVersion keys".to_string(),
        );
    }
    Ok(updated)
}

/// Atomically updates `CFBundleShortVersionString` and `CFBundleVersion` inside `<App>.app/Contents/Info.plist`.
/// Returns `Ok(false)` when `plist_path` does not exist.
pub fn patch_macos_info_plist_version(
    plist_path: &Path,
    target_tag_or_version: &str,
) -> Result<bool, String> {
    let normalized = normalize_version_tag(target_tag_or_version).ok_or_else(|| {
        format!("Invalid target version `{target_tag_or_version}` for Info.plist update")
    })?;
    if !plist_path.is_file() {
        return Ok(false);
    }
    let existing = std::fs::read_to_string(plist_path).map_err(|e| {
        format!(
            "Failed reading `{}` for version update: {e}",
            plist_path.display()
        )
    })?;
    let updated = patch_plist_xml_bundle_versions(&existing, &normalized)?;
    let parent = plist_path
        .parent()
        .ok_or_else(|| format!("Info.plist `{}` has no parent directory", plist_path.display()))?;
    let tmp_plist = parent.join(format!(
        ".Info.plist.new-{}-{}",
        std::process::id(),
        &Uuid::new_v4().simple().to_string()[..8]
    ));
    let _ = std::fs::remove_file(&tmp_plist);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_plist)
            .map_err(|e| format!("Failed staging `{}`: {e}", tmp_plist.display()))?;
        f.write_all(updated.as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp_plist);
                format!("Failed writing staged Info.plist: {e}")
            })?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_plist, std::fs::Permissions::from_mode(0o644));
    }
    if let Err(e) = std::fs::rename(&tmp_plist, plist_path) {
        let _ = std::fs::remove_file(&tmp_plist);
        return Err(format!(
            "Failed atomically replacing `{}`: {e}",
            plist_path.display()
        ));
    }
    tracing::info!(
        "Updated `{}` bundle version to `{}`",
        plist_path.display(),
        normalized.trim_start_matches('v')
    );
    Ok(true)
}

/// Re-signs `<AppRoot>.app` ad-hoc via `/usr/bin/codesign --force --deep --sign - <AppRoot>.app`
/// so `Contents/_CodeSignature/CodeResources`, `Info.plist` (`dev.codahub.desktop`), and
/// `Contents/MacOS/coda-desktop` remain sealed and pass `codesign --verify --deep --strict`.
pub fn resign_macos_app_bundle_adhoc(app_root: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("/usr/bin/codesign");
        cmd.args(["--force", "--deep", "--sign", "-"]).arg(app_root);
        let out = run_process_bounded(cmd, Duration::from_secs(20)).map_err(|e| {
            tracing::error!(
                "Failed invoking `/usr/bin/codesign` on `{}`: {e}",
                app_root.display()
            );
            format!(
                "Failed invoking `/usr/bin/codesign` on `{}`: {e}",
                app_root.display()
            )
        })?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            tracing::error!(
                "Ad-hoc bundle codesign failed for `{}` ({}): {stderr}",
                app_root.display(),
                out.status
            );
            return Err(format!(
                "Ad-hoc bundle codesign failed for `{}` ({}): {stderr}",
                app_root.display(),
                out.status
            ));
        }
        tracing::info!(
            "Re-signed macOS app bundle `{}` ad-hoc after self-update",
            app_root.display()
        );
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_root;
        Ok(())
    }
}

fn extract_version_tag_from_smoke_stdout(version_stdout: &str) -> Option<String> {
    version_stdout
        .split_whitespace()
        .rev()
        .find_map(normalize_version_tag)
}

fn locate_extracted_desktop_binary(unpack_dir: &Path) -> Result<PathBuf, String> {
    let preferred = unpack_dir.join("coda-macos-aarch64").join("coda-desktop");
    if let Ok(meta) = std::fs::symlink_metadata(&preferred) {
        if meta.file_type().is_symlink() {
            return Err("Refusing symlinked `coda-desktop` inside update archive".to_string());
        }
        if meta.is_file() {
            return Ok(preferred);
        }
    }
    let direct = unpack_dir.join("coda-desktop");
    if let Ok(meta) = std::fs::symlink_metadata(&direct) {
        if meta.file_type().is_symlink() {
            return Err("Refusing symlinked `coda-desktop` inside update archive".to_string());
        }
        if meta.is_file() {
            return Ok(direct);
        }
    }
    if let Ok(rd) = std::fs::read_dir(unpack_dir) {
        for entry in rd.flatten() {
            let sub = entry.path();
            if sub.is_dir() {
                let cand = sub.join("coda-desktop");
                if let Ok(meta) = std::fs::symlink_metadata(&cand) {
                    if meta.file_type().is_symlink() {
                        return Err(
                            "Refusing symlinked `coda-desktop` inside update archive".to_string()
                        );
                    }
                    if meta.is_file() {
                        return Ok(cand);
                    }
                }
            }
        }
    }
    Err("`coda-macos-aarch64/coda-desktop` not found inside update archive".to_string())
}

/// Extracts `coda-macos-aarch64/coda-desktop` from `archive_bytes`, stages it at
/// `<exe_dir>/.coda-desktop.new-<pid>` (`0o755`), strips `com.apple.quarantine`, runs
/// `<staged_bin> --version` smoke check, atomically `rename()`s over `target_exe`, and when inside
/// `<AppRoot>.app`, patches `Contents/Info.plist` and re-signs `<AppRoot>.app` ad-hoc.
pub fn stage_and_swap_extracted_desktop_binary(
    archive_bytes: &[u8],
    target_exe: &Path,
    pid: u32,
) -> Result<String, String> {
    stage_and_swap_extracted_desktop_binary_with_tag(archive_bytes, target_exe, pid, None)
}

pub fn stage_and_swap_extracted_desktop_binary_with_tag(
    archive_bytes: &[u8],
    target_exe: &Path,
    pid: u32,
    target_tag: Option<&str>,
) -> Result<String, String> {
    let staged_path = staged_executable_path_for(target_exe, pid)?;
    let work_dir = std::env::temp_dir().join(format!(
        "coda-desktop-upd-{pid}-{}",
        &Uuid::new_v4().simple().to_string()[..12]
    ));
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| format!("Failed creating temp unpack dir: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&work_dir, std::fs::Permissions::from_mode(0o700));
    }

    let outcome: Result<String, String> = (|| {
        let tar_path = work_dir.join("desktop.tar.gz");
        std::fs::write(&tar_path, archive_bytes)
            .map_err(|e| format!("Failed writing temporary archive: {e}"))?;

        let unpack_dir = work_dir.join("unpack");
        std::fs::create_dir_all(&unpack_dir)
            .map_err(|e| format!("Failed creating unpack dir: {e}"))?;

        let mut tar_cmd = std::process::Command::new("tar");
        tar_cmd.arg("-xzf").arg(&tar_path).arg("-C").arg(&unpack_dir);
        let tar_out = run_process_bounded(tar_cmd, Duration::from_secs(25))
            .map_err(|e| format!("Failed invoking `tar -xzf`: {e}"))?;
        if !tar_out.status.success() {
            let err_s = String::from_utf8_lossy(&tar_out.stderr).trim().to_string();
            return Err(format!("`tar -xzf` failed ({}): {err_s}", tar_out.status));
        }

        let extracted_bin = locate_extracted_desktop_binary(&unpack_dir)?;
        let bin_bytes = std::fs::read(&extracted_bin)
            .map_err(|e| format!("Failed reading extracted `coda-desktop`: {e}"))?;
        if bin_bytes.is_empty() {
            return Err("Extracted `coda-desktop` binary is empty".to_string());
        }

        // Remove any stale sibling file from an interrupted prior run so create_new allocates a fresh inode.
        let _ = std::fs::remove_file(&staged_path);
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o755);
            }
            let mut f = opts.open(&staged_path).map_err(|e| {
                format!(
                    "Cannot stage update at `{}` ({e}) — check folder permissions",
                    staged_path.display()
                )
            })?;
            f.write_all(&bin_bytes)
                .and_then(|_| f.sync_all())
                .map_err(|e| format!("Failed writing staged binary: {e}"))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&staged_path, std::fs::Permissions::from_mode(0o755));
        }

        strip_macos_quarantine_best_effort(&staged_path);

        let mut smoke_cmd = std::process::Command::new(&staged_path);
        smoke_cmd.arg("--version");
        let smoke_out = match run_process_bounded(smoke_cmd, Duration::from_secs(6)) {
            Ok(o) => o,
            Err(e) => {
                let _ = std::fs::remove_file(&staged_path);
                return Err(format!("Staged binary `--version` smoke check failed: {e}"));
            }
        };
        if !smoke_out.status.success() {
            let _ = std::fs::remove_file(&staged_path);
            let stderr = String::from_utf8_lossy(&smoke_out.stderr).trim().to_string();
            return Err(format!(
                "Staged binary failed `--version` smoke check ({}): {stderr}",
                smoke_out.status
            ));
        }
        let version_stdout = String::from_utf8_lossy(&smoke_out.stdout).trim().to_string();

        if let Err(e) = std::fs::rename(&staged_path, target_exe) {
            let _ = std::fs::remove_file(&staged_path);
            return Err(format!(
                "Atomic rename over `{}` failed ({e})",
                target_exe.display()
            ));
        }

        if let Some(app_root) = detect_macos_app_bundle_root(target_exe) {
            let resolved_tag = target_tag
                .and_then(normalize_version_tag)
                .or_else(|| extract_version_tag_from_smoke_stdout(&version_stdout));
            let plist_path = app_root.join("Contents").join("Info.plist");
            if let Some(ref tag_for_plist) = resolved_tag {
                patch_macos_info_plist_version(&plist_path, tag_for_plist)?;
            }
            strip_macos_quarantine_best_effort(&app_root);
            if plist_path.is_file() {
                resign_macos_app_bundle_adhoc(&app_root)?;
            } else {
                tracing::warn!(
                    "Bundle `{}` has no Contents/Info.plist; skipping deep bundle codesign",
                    app_root.display()
                );
            }
        } else {
            strip_macos_quarantine_best_effort(target_exe);
        }

        Ok(version_stdout)
    })();

    let _ = std::fs::remove_dir_all(&work_dir);
    outcome
}

/// Downloads `coda-macos-aarch64-vX.Y.Z.tar.gz` + `SHA256SUMS`, strictly verifies the SHA-256
/// digest, extracts & smoke-tests `<staged_bin> --version`, and atomically replaces `target_exe`.
pub async fn download_verify_and_stage_update<F>(
    server_url: &str,
    target_tag: &str,
    server_matches_tag: bool,
    target_exe: &Path,
    on_progress: F,
) -> Result<String, String>
where
    F: FnMut(&str),
{
    let gh_release_base = format!(
        "https://github.com/{DEFAULT_GITHUB_REPO}/releases/download"
    );
    download_verify_and_stage_update_with_bases(
        server_url,
        &gh_release_base,
        target_tag,
        server_matches_tag,
        target_exe,
        256,
        on_progress,
    )
    .await
}

/// Builds ordered `(archive_urls, checksum_urls)` candidates for `target_tag`, querying
/// `/download/checksums.txt` and `SHA256SUMS-{tag}.txt` / `SHA256SUMS.txt` BEFORE bare `SHA256SUMS`
/// so real releases succeed without 404 probes.
pub fn build_update_download_candidates(
    server_url: &str,
    github_download_base: &str,
    target_tag: &str,
    server_matches_tag: bool,
) -> Result<(Vec<String>, Vec<String>), String> {
    let tag = normalize_version_tag(target_tag)
        .ok_or_else(|| format!("Invalid target release version `{target_tag}`"))?;
    let versioned_asset = format!("coda-macos-aarch64-{tag}.tar.gz");
    let clean_srv = server_url.trim().trim_end_matches('/');
    let clean_gh = github_download_base.trim().trim_end_matches('/');

    let mut archive_urls = Vec::new();
    let mut checksum_urls = Vec::new();
    if server_matches_tag && is_allowed_download_origin(clean_srv) {
        archive_urls.push(format!("{clean_srv}/download/desktop-macos.tar.gz"));
        checksum_urls.push(format!("{clean_srv}/download/checksums.txt"));
        checksum_urls.push(format!("{clean_srv}/download/SHA256SUMS"));
    }
    if is_allowed_download_origin(clean_gh) {
        archive_urls.push(format!("{clean_gh}/{tag}/{versioned_asset}"));
        checksum_urls.push(format!("{clean_gh}/{tag}/SHA256SUMS-{tag}.txt"));
        checksum_urls.push(format!("{clean_gh}/{tag}/SHA256SUMS.txt"));
        checksum_urls.push(format!("{clean_gh}/{tag}/SHA256SUMS"));
    }
    if archive_urls.is_empty() {
        return Err("No valid HTTPS download endpoint configured".to_string());
    }
    Ok((archive_urls, checksum_urls))
}

pub async fn download_verify_and_stage_update_with_bases<F>(
    server_url: &str,
    github_download_base: &str,
    target_tag: &str,
    server_matches_tag: bool,
    target_exe: &Path,
    min_archive_bytes: usize,
    mut on_progress: F,
) -> Result<String, String>
where
    F: FnMut(&str),
{
    let tag = normalize_version_tag(target_tag)
        .ok_or_else(|| format!("Invalid target release version `{target_tag}`"))?;
    let versioned_asset = format!("coda-macos-aarch64-{tag}.tar.gz");
    let unversioned_asset = "coda-macos-aarch64.tar.gz";
    let (archive_urls, checksum_urls) = build_update_download_candidates(
        server_url,
        github_download_base,
        &tag,
        server_matches_tag,
    )?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(8))
        .user_agent(format!("coda-desktop-updater/{CURRENT_DESKTOP_VERSION}"))
        .build()
        .map_err(|e| e.to_string())?;

    on_progress(&format!("Downloading {versioned_asset}…"));
    let mut archive_bytes_opt: Option<Vec<u8>> = None;
    let mut last_dl_err = String::new();
    for url in &archive_urls {
        match download_bounded_bytes(
            &http,
            url,
            MAX_DESKTOP_UPDATE_ARCHIVE_BYTES,
            min_archive_bytes,
        )
        .await
        {
            Ok(bytes) => {
                archive_bytes_opt = Some(bytes);
                break;
            }
            Err(e) => last_dl_err = e,
        }
    }
    let archive_bytes = archive_bytes_opt.ok_or_else(|| {
        if last_dl_err.is_empty() {
            format!("Failed downloading {versioned_asset}")
        } else {
            last_dl_err
        }
    })?;

    on_progress("Downloading & verifying SHA256SUMS…");
    let mut manifest_text_opt: Option<String> = None;
    let mut last_sum_err = String::new();
    for sum_url in &checksum_urls {
        match download_bounded_bytes(&http, sum_url, MAX_CHECKSUM_MANIFEST_BYTES, 64).await {
            Ok(bytes) => {
                let txt = String::from_utf8_lossy(&bytes).to_string();
                if txt.trim_start().starts_with('<') {
                    last_sum_err = format!("Checksum endpoint {sum_url} returned HTML");
                    continue;
                }
                manifest_text_opt = Some(txt);
                break;
            }
            Err(e) => last_sum_err = e,
        }
    }
    let manifest_text = manifest_text_opt.ok_or_else(|| {
        if last_sum_err.is_empty() {
            "SHA256SUMS manifest could not be downloaded; aborting update".to_string()
        } else {
            format!("SHA256SUMS download failed ({last_sum_err}); aborting update")
        }
    })?;

    verify_sha256_sums_strict(
        &manifest_text,
        &[versioned_asset.as_str(), unversioned_asset],
        &archive_bytes,
    )?;

    on_progress("Staging binary, updating Info.plist & re-signing Coda.app…");
    let target_owned = target_exe.to_path_buf();
    let tag_owned = tag.clone();
    let pid = std::process::id();
    let reported = tokio::task::spawn_blocking(move || {
        stage_and_swap_extracted_desktop_binary_with_tag(
            &archive_bytes,
            &target_owned,
            pid,
            Some(&tag_owned),
        )
    })
    .await
    .map_err(|e| format!("Update staging task panicked: {e}"))??;

    Ok(reported)
}

/// Relaunches `Coda.app` (`open -n <Coda.app>` when inside a macOS `.app` bundle, or spawns the
/// updated binary directly) and exits the current process cleanly.
pub fn relaunch_updated_desktop_and_exit(exe_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if let Some(app_bundle) = detect_macos_app_bundle_root(exe_path) {
        std::process::Command::new("/usr/bin/open")
            .arg("-n")
            .arg(&app_bundle)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed relaunching `{}`: {e}", app_bundle.display()))?;
        std::thread::sleep(Duration::from_millis(120));
        std::process::exit(0);
    }

    std::process::Command::new(exe_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed spawning updated `{}`: {e}", exe_path.display()))?;
    std::thread::sleep(Duration::from_millis(120));
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_comparator_and_tag_normalization() {
        assert!(is_newer_version("1.1.0", "1.0.0"));
        assert!(is_newer_version("v1.1.0", "1.0.9"));
        assert!(is_newer_version("v2.0.0", "v1.99.99"));
        assert!(is_newer_version("1.0.1", "v1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("v1.0.0", "1.0.0"));
        assert!(!is_newer_version("0.9.9", "1.0.0"));

        // Prerelease precedence rules (SemVer 2.0.0)
        assert!(is_newer_version("1.1.0", "1.1.0-rc.2"));
        assert!(!is_newer_version("1.1.0-rc.2", "1.1.0"));
        assert!(is_newer_version("1.1.0-rc.10", "1.1.0-rc.2"));
        assert!(is_newer_version("1.1.0-beta.1", "1.1.0-alpha.9"));

        // Build metadata ignored in precedence
        assert!(!is_newer_version("1.1.0+build9", "1.1.0+build1"));
        assert!(is_newer_version("1.1.1+sha.abc", "1.1.0+sha.def"));

        // Reject invalid / hostile inputs
        assert!(!is_newer_version("", "1.0.0"));
        assert!(!is_newer_version("v1.1", "1.0.0"));
        assert!(!is_newer_version("v1.1.0;rm -rf /", "1.0.0"));
        assert!(!is_newer_version("../1.1.0", "1.0.0"));
        assert_eq!(normalize_version_tag("1.1.0").as_deref(), Some("v1.1.0"));
        assert_eq!(
            normalize_version_tag("v1.1.0-rc.1+b42").as_deref(),
            Some("v1.1.0-rc.1")
        );
        assert_eq!(normalize_version_tag("v1.1"), None);
        assert_eq!(normalize_version_tag("../etc/passwd"), None);

        assert_eq!(
            extract_github_release_tag_from_location(
                "https://github.com/Annihilater/coda/releases/tag/v1.1.0"
            )
            .as_deref(),
            Some("v1.1.0")
        );
        assert_eq!(
            extract_github_release_tag_from_location(
                "https://github.com/Annihilater/coda/releases/tag/v1.2.0-rc.1?foo=bar"
            )
            .as_deref(),
            Some("v1.2.0-rc.1")
        );
        assert_eq!(
            extract_github_release_tag_from_location("https://github.com/Annihilater/coda/releases"),
            None
        );
    }

    #[test]
    fn test_sha256sums_manifest_matching_and_tamper_or_missing_rejection() {
        let archive = b"mock-coda-macos-aarch64-v1.1.0-tar-gz-bytes";
        let good_hex = hex::encode(Sha256::digest(archive));
        let other_hex = "a".repeat(64);
        let manifest = format!(
            "# Coda v1.1.0 checksums\n{other_hex}  Coda-Desktop-macos-aarch64-v1.1.0.dmg\n{good_hex} *./release-assets/coda-macos-aarch64-v1.1.0.tar.gz\n"
        );

        let verified = verify_sha256_sums_strict(
            &manifest,
            &["coda-macos-aarch64-v1.1.0.tar.gz"],
            archive,
        )
        .expect("valid SHA256SUMS must pass");
        assert_eq!(verified, good_hex);

        // BSD checksum format also supported
        let bsd_manifest = format!(
            "SHA256 (coda-macos-aarch64-v1.1.0.tar.gz) = {}\n",
            good_hex.to_ascii_uppercase()
        );
        assert!(verify_sha256_sums_strict(
            &bsd_manifest,
            &["coda-macos-aarch64-v1.1.0.tar.gz"],
            archive,
        )
        .is_ok());

        // Tampered archive bytes MUST fail
        let tamper_err = verify_sha256_sums_strict(
            &manifest,
            &["coda-macos-aarch64-v1.1.0.tar.gz"],
            b"tampered-bytes",
        )
        .unwrap_err();
        assert!(tamper_err.contains("mismatch"), "err={tamper_err}");

        // Missing target entry in SHA256SUMS MUST fail (never silently skip!)
        let missing_manifest =
            format!("{other_hex}  coda-daemon-linux-x86_64-v1.1.0.tar.gz\n");
        let missing_err = verify_sha256_sums_strict(
            &missing_manifest,
            &["coda-macos-aarch64-v1.1.0.tar.gz"],
            archive,
        )
        .unwrap_err();
        assert!(missing_err.contains("missing from SHA256SUMS"), "err={missing_err}");

        // HTML SPA fallback or empty manifest MUST fail
        assert!(verify_sha256_sums_strict(
            "<!DOCTYPE html><html></html>",
            &["coda-macos-aarch64-v1.1.0.tar.gz"],
            archive,
        )
        .is_err());
        assert!(verify_sha256_sums_strict(
            "not64hex  coda-macos-aarch64-v1.1.0.tar.gz\n",
            &["coda-macos-aarch64-v1.1.0.tar.gz"],
            archive,
        )
        .is_err());
    }

    #[test]
    fn test_macos_app_bundle_root_detection_and_staging_path() {
        let app_exe = Path::new("/Applications/Coda.app/Contents/MacOS/coda-desktop");
        assert_eq!(
            detect_macos_app_bundle_root(app_exe),
            Some(PathBuf::from("/Applications/Coda.app"))
        );
        assert_eq!(
            staged_executable_path_for(app_exe, 4242).unwrap(),
            PathBuf::from("/Applications/Coda.app/Contents/MacOS/.coda-desktop.new-4242")
        );

        let user_app_exe =
            Path::new("/Users/friend/Applications/Coda Desktop.app/Contents/MacOS/coda-desktop");
        assert_eq!(
            detect_macos_app_bundle_root(user_app_exe),
            Some(PathBuf::from("/Users/friend/Applications/Coda Desktop.app"))
        );

        // Standalone CLI / cargo target binaries are NOT `.app` bundles
        assert_eq!(
            detect_macos_app_bundle_root(Path::new("/usr/local/bin/coda-desktop")),
            None
        );
        assert_eq!(
            detect_macos_app_bundle_root(Path::new(
                "/tmp/Fake.app/bin/MacOS/coda-desktop"
            )),
            None
        );
        assert_eq!(
            detect_macos_app_bundle_root(Path::new(
                "/tmp/NotAnApp/Contents/MacOS/coda-desktop"
            )),
            None
        );
    }

    #[test]
    fn test_url_probe_candidate_order_avoids_guaranteed_404s() {
        let site_urls = build_site_info_probe_urls("https://codahub.dev/");
        assert_eq!(
            site_urls,
            vec![
                "https://codahub.dev/api/v1/public/site-info".to_string(),
                "https://codahub.dev/api/v1/site-info".to_string(),
            ],
            "must probe /api/v1/public/site-info BEFORE /api/v1/site-info"
        );

        let (archives, checksums) = build_update_download_candidates(
            "https://codahub.dev/",
            "https://github.com/Annihilater/coda/releases/download/",
            "v1.1.1",
            true,
        )
        .unwrap();
        assert_eq!(
            archives,
            vec![
                "https://codahub.dev/download/desktop-macos.tar.gz".to_string(),
                "https://github.com/Annihilater/coda/releases/download/v1.1.1/coda-macos-aarch64-v1.1.1.tar.gz".to_string(),
            ]
        );
        assert_eq!(
            checksums,
            vec![
                "https://codahub.dev/download/checksums.txt".to_string(),
                "https://codahub.dev/download/SHA256SUMS".to_string(),
                "https://github.com/Annihilater/coda/releases/download/v1.1.1/SHA256SUMS-v1.1.1.txt".to_string(),
                "https://github.com/Annihilater/coda/releases/download/v1.1.1/SHA256SUMS.txt".to_string(),
                "https://github.com/Annihilater/coda/releases/download/v1.1.1/SHA256SUMS".to_string(),
            ],
            "checksums.txt and SHA256SUMS-vX.Y.Z.txt / SHA256SUMS.txt must precede bare SHA256SUMS"
        );
    }

    #[test]
    fn test_patch_macos_info_plist_version_inline_and_multiline() {
        let release_plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.codahub.desktop</string>
  <key>CFBundleShortVersionString</key><string>1.0.0</string>
  <key>CFBundleVersion</key><string>1.0.0</string>
</dict>
</plist>
"#;
        let patched_inline =
            patch_plist_xml_bundle_versions(release_plist, "v1.1.1").expect("patch inline plist");
        assert!(patched_inline.contains("<key>CFBundleShortVersionString</key><string>1.1.1</string>"));
        assert!(patched_inline.contains("<key>CFBundleVersion</key><string>1.1.1</string>"));
        assert!(patched_inline.contains("<key>CFBundleIdentifier</key><string>dev.codahub.desktop</string>"));

        let multiline_plist = "<plist><dict>\n  <key>CFBundleShortVersionString</key>\n  <string>0.9.0</string>\n  <key>CFBundleVersion</key>\n  <string>0.9.0</string>\n</dict></plist>\n";
        let patched_multi =
            patch_plist_xml_bundle_versions(multiline_plist, "1.2.0-rc.1").expect("patch multiline");
        assert!(patched_multi.contains("<key>CFBundleShortVersionString</key>\n  <string>1.2.0-rc.1</string>"));
        assert!(patched_multi.contains("<key>CFBundleVersion</key>\n  <string>1.2.0-rc.1</string>"));

        // Reject hostile / non-SemVer inputs
        assert!(patch_plist_xml_bundle_versions(release_plist, "1.1.1</string><evil/>").is_err());

        // Atomic file update on disk
        let tmp_dir = std::env::temp_dir().join(format!("coda-plist-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let plist_file = tmp_dir.join("Info.plist");
        assert!(!patch_macos_info_plist_version(&plist_file, "v1.1.1").unwrap());
        std::fs::write(&plist_file, release_plist).unwrap();
        assert!(patch_macos_info_plist_version(&plist_file, "v1.1.1").unwrap());
        let on_disk = std::fs::read_to_string(&plist_file).unwrap();
        assert!(on_disk.contains("<string>1.1.1</string>"));
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn test_end_to_end_self_update_check_download_verify_stage_and_atomic_swap() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Build a real `.tar.gz` containing `coda-macos-aarch64/coda-desktop` shell script that passes `--version`
        let tmp = std::env::temp_dir().join(format!("coda-selfupd-e2e-{}", Uuid::new_v4()));
        let pkg_dir = tmp.join("pkg").join("coda-macos-aarch64");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let mock_new_bin = pkg_dir.join("coda-desktop");
        std::fs::write(
            &mock_new_bin,
            b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"coda-desktop 1.1.0\"; exit 0; fi\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_new_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tar_file = tmp.join("coda-macos-aarch64-v1.1.0.tar.gz");
        let status = std::process::Command::new("tar")
            .args([
                "-czf",
                tar_file.to_str().unwrap(),
                "-C",
                tmp.join("pkg").to_str().unwrap(),
                "coda-macos-aarch64",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let tar_bytes = std::fs::read(&tar_file).unwrap();
        let tar_sha256 = hex::encode(Sha256::digest(&tar_bytes));
        let sums_body = format!("{tar_sha256}  coda-macos-aarch64-v1.1.0.tar.gz\n");

        // Create a realistic installed `Coda.app` bundle (v1.0.0) with Info.plist & Resources/AppIcon.icns
        let app_root = tmp.join("Coda.app");
        let app_contents = app_root.join("Contents");
        let app_macos_dir = app_contents.join("MacOS");
        let app_res_dir = app_contents.join("Resources");
        std::fs::create_dir_all(&app_macos_dir).unwrap();
        std::fs::create_dir_all(&app_res_dir).unwrap();
        let installed_exe = app_macos_dir.join("coda-desktop");
        std::fs::write(&installed_exe, b"#!/bin/sh\necho \"coda-desktop 1.0.0\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&installed_exe, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        std::fs::write(app_res_dir.join("AppIcon.icns"), b"mock-icns-payload").unwrap();
        let plist_path = app_contents.join("Info.plist");
        std::fs::write(
            &plist_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>coda-desktop</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIdentifier</key><string>dev.codahub.desktop</string>
  <key>CFBundleName</key><string>Coda</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0.0</string>
  <key>CFBundleVersion</key><string>1.0.0</string>
</dict>
</plist>
"#,
        )
        .unwrap();
        #[cfg(target_os = "macos")]
        {
            resign_macos_app_bundle_adhoc(&app_root).expect("initial bundle sign must pass");
        }

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}");
        let tar_for_srv = tar_bytes.clone();
        let sums_for_srv = sums_body.clone();
        let not_found_hits = Arc::new(AtomicUsize::new(0));
        let not_found_bg = Arc::clone(&not_found_hits);

        tokio::spawn(async move {
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.starts_with("GET /api/v1/public/site-info ") {
                    let body = br#"{"service":"coda-server","version":"v1.1.0"}"#;
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(hdr.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                } else if req.starts_with("HEAD /gh-latest ") || req.starts_with("GET /gh-latest ") {
                    let hdr = "HTTP/1.1 302 Found\r\nLocation: https://github.com/Annihilater/coda/releases/tag/v1.2.0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = stream.write_all(hdr.as_bytes()).await;
                } else if req.starts_with("GET /download/desktop-macos.tar.gz ") {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        tar_for_srv.len()
                    );
                    let _ = stream.write_all(hdr.as_bytes()).await;
                    let _ = stream.write_all(&tar_for_srv).await;
                } else if req.starts_with("GET /download/checksums.txt ") {
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        sums_for_srv.len()
                    );
                    let _ = stream.write_all(hdr.as_bytes()).await;
                    let _ = stream.write_all(sums_for_srv.as_bytes()).await;
                } else {
                    not_found_bg.fetch_add(1, AtomicOrdering::Relaxed);
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                }
            }
        });

        // 1. Check via `/api/v1/public/site-info` (zero 404 probes!)
        let chk = check_for_desktop_update_with_endpoints(&base_url, None, "1.0.0")
            .await
            .unwrap();
        assert!(chk.update_available);
        assert_eq!(chk.latest_version, "v1.1.0");
        assert!(chk.server_matches_latest);
        assert_eq!(chk.source, "site-info");

        // 2. Fallback to GitHub Releases redirect when server is unreachable or older
        let gh_fb = check_for_desktop_update_with_endpoints(
            "http://127.0.0.1:9",
            Some(&format!("{base_url}/gh-latest")),
            "1.1.0",
        )
        .await
        .unwrap();
        assert!(gh_fb.update_available);
        assert_eq!(gh_fb.latest_version, "v1.2.0");
        assert_eq!(gh_fb.source, "github-releases");

        // 3. Full download -> checksums.txt verification -> staging -> `--version` smoke check -> atomic rename -> Info.plist patch -> bundle codesign
        let mut steps = Vec::new();
        let ver_out = download_verify_and_stage_update_with_bases(
            &base_url,
            &format!("{base_url}/gh-dl"),
            "v1.1.0",
            true,
            &installed_exe,
            32,
            |msg| steps.push(msg.to_string()),
        )
        .await
        .expect("end-to-end self-update staging must succeed");
        assert_eq!(ver_out, "coda-desktop 1.1.0");
        assert!(steps.len() >= 3);
        assert_eq!(
            not_found_hits.load(AtomicOrdering::Relaxed),
            0,
            "reordered site-info and checksums.txt URLs must produce zero 404 probes"
        );

        // Verify target binary was atomically replaced, Info.plist updated to 1.1.0, and codesign --verify --deep --strict passes!
        let swapped_content = std::fs::read_to_string(&installed_exe).unwrap();
        assert!(swapped_content.contains("coda-desktop 1.1.0"));
        let updated_plist = std::fs::read_to_string(&plist_path).unwrap();
        assert!(
            updated_plist.contains("<key>CFBundleShortVersionString</key><string>1.1.0</string>"),
            "plist={updated_plist}"
        );
        assert!(
            updated_plist.contains("<key>CFBundleVersion</key><string>1.1.0</string>"),
            "plist={updated_plist}"
        );
        #[cfg(target_os = "macos")]
        {
            let verify_status = std::process::Command::new("/usr/bin/codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(&app_root)
                .status()
                .expect("invoke codesign --verify");
            assert!(
                verify_status.success(),
                "`codesign --verify --deep --strict` must pass on updated Coda.app"
            );
        }
        assert!(
            !staged_executable_path_for(&installed_exe, std::process::id())
                .unwrap()
                .exists()
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
