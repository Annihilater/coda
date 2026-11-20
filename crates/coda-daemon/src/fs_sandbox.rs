//! Filesystem sandboxing, sensitive credential path filtering, bounded UTF-8 file previews,
//! and localhost port-preview SSRF guards for `coda-daemon`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use coda_protocol::{PeerMessage, RemoteFileItem, TrafficLane};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub fn sanitize_upload_filename(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        anyhow::bail!("invalid upload filename length");
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.contains('\0')
        || trimmed.starts_with('.')
        || trimmed.chars().any(|c| c.is_control())
    {
        anyhow::bail!("unsafe upload filename rejected");
    }
    let clean: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let final_name = clean.trim().to_string();
    if final_name.is_empty() || final_name.starts_with('.') {
        anyhow::bail!("unsafe upload filename rejected");
    }
    Ok(final_name)
}

pub fn is_sensitive_path(path: &Path) -> bool {
    let lower_full = path.to_string_lossy().to_ascii_lowercase();
    for sys_prefix in [
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/sudoers",
        "/private/etc/shadow",
        "/private/etc/master.passwd",
        "/private/etc/sudoers",
        "/proc/",
        "/sys/",
        "/dev/",
        "/boot/",
    ] {
        if lower_full == sys_prefix.trim_end_matches('/') || lower_full.starts_with(sys_prefix) {
            return true;
        }
    }

    let comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();

    for c in &comps {
        if matches!(
            c.as_str(),
            ".ssh"
                | ".gnupg"
                | ".aws"
                | ".kube"
                | ".azure"
                | "keychains"
                | ".docker"
                | ".claude"
                | ".codex"
                | ".grok"
                | ".antigravity"
                | ".gemini"
        ) {
            return true;
        }
    }
    for win in comps.windows(2) {
        if (win[0] == ".config"
            && matches!(
                win[1].as_str(),
                "coda" | "gcloud" | "gh" | "herdr" | "herdr-core" | "grok" | "antigravity"
            ))
            || (win[0] == ".cargo" && win[1].starts_with("credentials"))
            || (win[0] == "library" && win[1] == "keychains")
            || (win[0] == "application support" && matches!(win[1].as_str(), "herdr" | "herdr-core"))
            || (win[0] == "share" && matches!(win[1].as_str(), "herdr" | "herdr-core"))
        {
            return true;
        }
    }

    if let Some(fname) = comps.last() {
        if (fname == ".env"
            || (fname.starts_with(".env.")
                && !matches!(
                    fname.as_str(),
                    ".env.example" | ".env.sample" | ".env.template"
                )))
            || matches!(
                fname.as_str(),
                "id_rsa"
                    | "id_ed25519"
                    | "id_ecdsa"
                    | "id_dsa"
                    | "id_ed25519_sk"
                    | "id_ecdsa_sk"
                    | "authorized_keys"
                    | "known_hosts"
                    | "credentials"
                    | "credentials.toml"
                    | "daemon.json"
                    | "desktop-session.json"
                    | "desktop_client_identity.json"
                    | "trusted_peers.json"
                    | "coda-daemon.lock"
                    | ".netrc"
                    | ".npmrc"
                    | ".pypirc"
                    | ".pgpass"
                    | ".git-credentials"
                    | "shadow"
                    | "gshadow"
                    | "master.passwd"
                    | "sudoers"
                    | "terraform.tfstate"
                    | "terraform.tfstate.backup"
            )
        {
            return true;
        }
        if fname.ends_with(".pem")
            || fname.ends_with(".key")
            || fname.ends_with(".p12")
            || fname.ends_with(".pfx")
            || fname.ends_with(".kdbx")
            || fname.ends_with("_history")
            || fname.ends_with(".tfstate")
        {
            return true;
        }
    }

    false
}

pub fn validate_local_preview_target(port: u16, path: &str) -> Result<reqwest::Url, String> {
    if port < 1024 {
        return Err(format!(
            "Local preview port {port} blocked (privileged ports < 1024 are disallowed)"
        ));
    }
    if matches!(
        port,
        2375 | 2376 | 3306 | 5432 | 6379 | 9200 | 11211 | 24443 | 24444 | 27017
    ) {
        return Err(format!(
            "Local preview port {port} blocked (internal infrastructure/control port)"
        ));
    }
    if path.starts_with('@') || path.contains("://") {
        return Err("Invalid local preview path".into());
    }
    let clean_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if clean_path.chars().any(|c| c.is_control())
        || clean_path.contains('\\')
        || clean_path.starts_with("//")
    {
        return Err("Invalid local preview path".into());
    }
    let raw_url = format!("http://127.0.0.1:{port}{clean_path}");
    let parsed = reqwest::Url::parse(&raw_url).map_err(|e| format!("Invalid preview URL: {e}"))?;
    if parsed.scheme() != "http"
        || parsed.host_str() != Some("127.0.0.1")
        || parsed.port() != Some(port)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Local preview target must remain strictly http://127.0.0.1".into());
    }
    Ok(parsed)
}

pub fn lossy_utf8_prefix(bytes: &[u8], max_bytes: usize) -> String {
    let slice = if bytes.len() <= max_bytes {
        bytes
    } else {
        let mut end = max_bytes;
        let floor = max_bytes.saturating_sub(3);
        while end > floor && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end -= 1;
        }
        &bytes[..end]
    };
    String::from_utf8_lossy(slice).to_string()
}

pub fn expand_tilde_path(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.chars().any(|c| c.is_control()) {
        anyhow::bail!("invalid empty or control-character path");
    }
    if trimmed == "~" || trimmed == "~/" {
        let home = std::env::var("HOME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("HOME environment variable is not set"))?;
        return Ok(PathBuf::from(home));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var("HOME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("HOME environment variable is not set"))?;
        return Ok(PathBuf::from(home).join(rest));
    }
    if trimmed.starts_with('~') {
        anyhow::bail!("unsupported ~user path syntax; use ~/...");
    }
    Ok(PathBuf::from(trimmed))
}

pub fn validate_workspace_folder_name(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        anyhow::bail!("folder_name must be 1..=128 characters");
    }
    if trimmed.starts_with('-') {
        anyhow::bail!("folder_name must not start with '-'");
    }
    if trimmed == "."
        || trimmed == ".."
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
        || trimmed.chars().any(|c| c.is_control())
    {
        anyhow::bail!("unsafe folder_name rejected");
    }
    let mut normal_count = 0usize;
    for comp in Path::new(trimmed).components() {
        match comp {
            std::path::Component::Normal(_) => normal_count += 1,
            _ => anyhow::bail!("unsafe folder_name path component rejected"),
        }
    }
    if normal_count != 1 {
        anyhow::bail!("folder_name must be a single directory name");
    }
    Ok(trimmed.to_string())
}

fn is_blocked_git_clone_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if h.is_empty()
        || h.starts_with('-')
        || h.starts_with('.')
        || h.ends_with('.')
        || h == "localhost"
        || h.ends_with(".localhost")
        || h == "metadata.google.internal"
        || h == "169.254.169.254"
    {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                    || v4.is_multicast()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        };
    }
    false
}

/// Validates a remote `git clone` URL against command/argument/transport injection (`ext::`,
/// `file://`, `http://`, `git://`, `-oProxyCommand`, `--upload-pack`, etc.).
/// Permits ONLY `https://`, `ssh://`, or `git@<host>:<path>`.
pub fn validate_git_clone_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 1024 {
        anyhow::bail!("git_clone_url must be 1..=1024 characters");
    }
    if trimmed.starts_with('-') {
        anyhow::bail!("git_clone_url must not start with '-'");
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        anyhow::bail!("git_clone_url must not contain whitespace or control characters");
    }

    if trimmed.starts_with("https://") || trimmed.starts_with("ssh://") {
        let parsed =
            reqwest::Url::parse(trimmed).map_err(|e| anyhow::anyhow!("invalid git URL: {e}"))?;
        if !matches!(parsed.scheme(), "https" | "ssh") {
            anyhow::bail!("only https://, ssh://, or git@host:path clone URLs are allowed");
        }
        if parsed.username().starts_with('-') {
            anyhow::bail!("unsafe username in git_clone_url");
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("git_clone_url missing host"))?;
        if is_blocked_git_clone_host(host) {
            anyhow::bail!("disallowed or unsafe host in git_clone_url");
        }
        let repo_path = parsed.path().trim_matches('/');
        if repo_path.is_empty() || repo_path.starts_with('-') {
            anyhow::bail!("git_clone_url missing repository path");
        }
        if Path::new(repo_path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            anyhow::bail!("git_clone_url path must not contain '..'");
        }
        return Ok(trimmed.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("git@") {
        if rest.contains("://") {
            anyhow::bail!("malformed git@host:path URL");
        }
        let Some((host, repo_path)) = rest.split_once(':') else {
            anyhow::bail!("git@ clone URL must match git@<host>:<path>");
        };
        if host.len() > 253
            || is_blocked_git_clone_host(host)
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            anyhow::bail!("invalid host in git@ clone URL");
        }
        let clean_repo = repo_path.trim_start_matches('/');
        if clean_repo.is_empty()
            || repo_path.starts_with('-')
            || clean_repo.starts_with('-')
            || repo_path.contains("..")
            || !repo_path
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~'))
        {
            anyhow::bail!("invalid repository path in git@ clone URL");
        }
        return Ok(trimmed.to_string());
    }

    anyhow::bail!(
        "disallowed git_clone_url scheme; only https://, ssh://, or git@host:path are permitted"
    )
}

pub async fn handle_bootstrap_workspace_rpc(
    req_id: String,
    parent_dir: String,
    folder_name: String,
    git_clone_url: Option<String>,
) -> (PeerMessage, TrafficLane) {
    let fail = |req_id: String, detail: String| {
        (
            PeerMessage::BootstrapWorkspaceResult {
                req_id,
                ok: false,
                workspace_path: String::new(),
                detail,
            },
            TrafficLane::ControlP0,
        )
    };

    let expanded_parent = match expand_tilde_path(&parent_dir) {
        Ok(p) => p,
        Err(e) => return fail(req_id, format!("invalid parent_dir: {e}")),
    };
    if !expanded_parent.is_absolute() {
        return fail(
            req_id,
            "parent_dir must be an absolute path or start with ~/".into(),
        );
    }
    if expanded_parent
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return fail(req_id, "parent_dir must not contain '..' traversal".into());
    }
    if is_sensitive_path(&expanded_parent) {
        return fail(req_id, "access denied: parent_dir is restricted".into());
    }

    let clean_folder = match validate_workspace_folder_name(&folder_name) {
        Ok(f) => f,
        Err(e) => return fail(req_id, e.to_string()),
    };

    if is_sensitive_path(&expanded_parent.join(&clean_folder)) {
        return fail(
            req_id,
            "access denied: target workspace path is restricted".into(),
        );
    }

    let clone_url_opt = match git_clone_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw_url) => match validate_git_clone_url(raw_url) {
            Ok(valid) => Some(valid),
            Err(e) => return fail(req_id, e.to_string()),
        },
        None => None,
    };

    if let Err(e) = tokio::fs::create_dir_all(&expanded_parent).await {
        return fail(req_id, format!("failed creating parent_dir: {e}"));
    }
    let canon_parent = match tokio::fs::canonicalize(&expanded_parent).await {
        Ok(p) => p,
        Err(e) => return fail(req_id, format!("failed resolving parent_dir: {e}")),
    };
    if is_sensitive_path(&canon_parent) {
        return fail(
            req_id,
            "access denied: parent_dir resolves to restricted path".into(),
        );
    }

    let target_path = canon_parent.join(&clean_folder);
    if is_sensitive_path(&target_path) {
        return fail(req_id, "access denied: target path is restricted".into());
    }

    if let Some(valid_url) = clone_url_opt {
        if tokio::fs::metadata(&target_path).await.is_ok() {
            return fail(
                req_id,
                format!("target directory already exists: {}", target_path.display()),
            );
        }
        let target_str = target_path.to_string_lossy().to_string();
        let mut cmd = Command::new("git");
        cmd.kill_on_drop(true)
            .env("PATH", coda_herdr::build_augmented_path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "-c",
                "protocol.allow=user",
                "-c",
                "protocol.ext.allow=never",
                "-c",
                "protocol.file.allow=never",
                "-c",
                "core.symlinks=false",
                "clone",
                "--",
                &valid_url,
                &target_str,
            ]);
        match tokio::time::timeout(std::time::Duration::from_secs(60), cmd.output()).await {
            Ok(Ok(out)) if out.status.success() => {
                let final_path = tokio::fs::canonicalize(&target_path)
                    .await
                    .unwrap_or(target_path);
                if is_sensitive_path(&final_path) || !final_path.starts_with(&canon_parent) {
                    return fail(
                        req_id,
                        "access denied: cloned workspace resolved outside parent_dir".into(),
                    );
                }
                (
                    PeerMessage::BootstrapWorkspaceResult {
                        req_id,
                        ok: true,
                        workspace_path: final_path.to_string_lossy().to_string(),
                        detail: "cloned".into(),
                    },
                    TrafficLane::ControlP0,
                )
            }
            Ok(Ok(out)) => {
                let stderr = lossy_utf8_prefix(&out.stderr, 2048);
                let msg = stderr.trim();
                fail(
                    req_id,
                    if msg.is_empty() {
                        format!("git clone failed with status {}", out.status)
                    } else {
                        format!("git clone failed: {msg}")
                    },
                )
            }
            Ok(Err(e)) => fail(req_id, format!("failed spawning git: {e}")),
            Err(_) => {
                let _ = tokio::fs::remove_dir_all(&target_path).await;
                fail(req_id, "git clone timed out after 60s".into())
            }
        }
    } else {
        if let Err(e) = tokio::fs::create_dir_all(&target_path).await {
            return fail(req_id, format!("failed creating workspace folder: {e}"));
        }
        let canon_target = match tokio::fs::canonicalize(&target_path).await {
            Ok(p) => p,
            Err(e) => return fail(req_id, format!("failed resolving workspace folder: {e}")),
        };
        if !canon_target.starts_with(&canon_parent) || is_sensitive_path(&canon_target) {
            return fail(
                req_id,
                "access denied: workspace folder escapes parent_dir or is restricted".into(),
            );
        }
        (
            PeerMessage::BootstrapWorkspaceResult {
                req_id,
                ok: true,
                workspace_path: canon_target.to_string_lossy().to_string(),
                detail: "created".into(),
            },
            TrafficLane::ControlP0,
        )
    }
}

pub async fn handle_list_directory_rpc(
    req_id: String,
    root_path: String,
    sub_path: String,
) -> (PeerMessage, TrafficLane) {
    let clean_sub = sub_path.trim_matches('/');
    let bad_sub = Path::new(clean_sub).components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    });
    if bad_sub {
        return (
            PeerMessage::DirectoryListing {
                req_id,
                root_path,
                sub_path: clean_sub.to_string(),
                entries: Vec::new(),
                error: Some("access denied: invalid sub_path traversal".into()),
            },
            TrafficLane::BulkP1,
        );
    }

    let expanded_root = match expand_tilde_path(&root_path) {
        Ok(p) => p,
        Err(e) => {
            return (
                PeerMessage::DirectoryListing {
                    req_id,
                    root_path,
                    sub_path: clean_sub.to_string(),
                    entries: Vec::new(),
                    error: Some(e.to_string()),
                },
                TrafficLane::BulkP1,
            );
        }
    };
    let root_path = if root_path.trim().starts_with('~') {
        expanded_root.to_string_lossy().to_string()
    } else {
        root_path
    };

    let Ok(canon_base) = tokio::fs::canonicalize(&expanded_root).await else {
        return (
            PeerMessage::DirectoryListing {
                req_id,
                root_path: root_path.clone(),
                sub_path: clean_sub.to_string(),
                entries: Vec::new(),
                error: Some(format!("root_path not found: {root_path}")),
            },
            TrafficLane::BulkP1,
        );
    };
    if is_sensitive_path(&expanded_root) || is_sensitive_path(&canon_base) {
        return (
            PeerMessage::DirectoryListing {
                req_id,
                root_path,
                sub_path: clean_sub.to_string(),
                entries: Vec::new(),
                error: Some("access denied: restricted path".into()),
            },
            TrafficLane::BulkP1,
        );
    }

    let mut candidate = canon_base.clone();
    if !clean_sub.is_empty() {
        for comp in Path::new(clean_sub).components() {
            if let std::path::Component::Normal(c) = comp {
                candidate.push(c);
            }
        }
    }
    let Ok(canon_target) = tokio::fs::canonicalize(&candidate).await else {
        return (
            PeerMessage::DirectoryListing {
                req_id,
                root_path,
                sub_path: clean_sub.to_string(),
                entries: Vec::new(),
                error: Some(format!("directory not found: {}", candidate.display())),
            },
            TrafficLane::BulkP1,
        );
    };
    if !canon_target.starts_with(&canon_base) || is_sensitive_path(&canon_target) {
        return (
            PeerMessage::DirectoryListing {
                req_id,
                root_path,
                sub_path: clean_sub.to_string(),
                entries: Vec::new(),
                error: Some("access denied: path escapes workspace or is sensitive".into()),
            },
            TrafficLane::BulkP1,
        );
    }

    match tokio::fs::read_dir(&canon_target).await {
        Ok(mut rd) => {
            let mut entries = Vec::new();
            while let Ok(Some(ent)) = rd.next_entry().await {
                let name = ent.file_name().to_string_lossy().to_string();
                if matches!(
                    name.as_str(),
                    ".git" | "target" | "node_modules" | ".DS_Store"
                ) {
                    continue;
                }
                let ent_path = ent.path();
                if is_sensitive_path(&ent_path) {
                    continue;
                }
                if let Ok(ent_canon) = tokio::fs::canonicalize(&ent_path).await {
                    if !ent_canon.starts_with(&canon_base) || is_sensitive_path(&ent_canon) {
                        continue;
                    }
                } else {
                    continue;
                }
                let meta = tokio::fs::metadata(&ent_path).await.ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size_bytes = if is_dir {
                    0
                } else {
                    meta.as_ref().map(|m| m.len()).unwrap_or(0)
                };
                let rel_path = if clean_sub.is_empty() {
                    name.clone()
                } else {
                    format!("{clean_sub}/{name}")
                };
                entries.push(RemoteFileItem {
                    name,
                    rel_path,
                    is_dir,
                    size_bytes,
                });
            }
            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            entries.truncate(120);
            (
                PeerMessage::DirectoryListing {
                    req_id,
                    root_path,
                    sub_path: clean_sub.to_string(),
                    entries,
                    error: None,
                },
                TrafficLane::BulkP1,
            )
        }
        Err(e) => (
            PeerMessage::DirectoryListing {
                req_id,
                root_path,
                sub_path: clean_sub.to_string(),
                entries: Vec::new(),
                error: Some(format!("{}: {e}", canon_target.display())),
            },
            TrafficLane::BulkP1,
        ),
    }
}

pub async fn handle_read_file_preview_rpc(
    req_id: String,
    path: String,
    max_bytes: usize,
) -> (PeerMessage, TrafficLane) {
    let cap = max_bytes.clamp(1024, 256_000);
    let expanded = match expand_tilde_path(&path) {
        Ok(p) => p,
        Err(e) => {
            return (
                PeerMessage::FilePreviewResponse {
                    req_id,
                    path,
                    content: String::new(),
                    error: Some(e.to_string()),
                },
                TrafficLane::BulkP1,
            );
        }
    };
    let canon = match tokio::fs::canonicalize(&expanded).await {
        Ok(p) => p,
        Err(e) => {
            return (
                PeerMessage::FilePreviewResponse {
                    req_id,
                    path,
                    content: String::new(),
                    error: Some(e.to_string()),
                },
                TrafficLane::BulkP1,
            );
        }
    };
    if is_sensitive_path(&expanded) || is_sensitive_path(&canon) {
        return (
            PeerMessage::FilePreviewResponse {
                req_id,
                path,
                content: String::new(),
                error: Some("access denied: sensitive file path blocked".into()),
            },
            TrafficLane::BulkP1,
        );
    }
    match tokio::fs::File::open(&canon).await {
        Ok(file) => {
            let meta = match file.metadata().await {
                Ok(m) => m,
                Err(e) => {
                    return (
                        PeerMessage::FilePreviewResponse {
                            req_id,
                            path,
                            content: String::new(),
                            error: Some(e.to_string()),
                        },
                        TrafficLane::BulkP1,
                    );
                }
            };
            if !meta.is_file() {
                return (
                    PeerMessage::FilePreviewResponse {
                        req_id,
                        path,
                        content: String::new(),
                        error: Some("not a regular file".into()),
                    },
                    TrafficLane::BulkP1,
                );
            }
            let total_bytes = meta.len();
            let mut buf = Vec::with_capacity(cap.min(65_536));
            if let Err(e) = file.take((cap + 1) as u64).read_to_end(&mut buf).await {
                return (
                    PeerMessage::FilePreviewResponse {
                        req_id,
                        path,
                        content: String::new(),
                        error: Some(e.to_string()),
                    },
                    TrafficLane::BulkP1,
                );
            }
            let was_truncated = buf.len() > cap || total_bytes > cap as u64;
            let slice = &buf[..buf.len().min(cap)];
            if slice.contains(&0u8) {
                (
                    PeerMessage::FilePreviewResponse {
                        req_id,
                        path,
                        content: format!(
                            "[Binary file · {} bytes — preview skipped]",
                            total_bytes
                        ),
                        error: None,
                    },
                    TrafficLane::BulkP1,
                )
            } else {
                let mut text = lossy_utf8_prefix(&buf, cap);
                if was_truncated {
                    text.push_str("\n\n... [truncated] ...");
                }
                (
                    PeerMessage::FilePreviewResponse {
                        req_id,
                        path,
                        content: text,
                        error: None,
                    },
                    TrafficLane::BulkP1,
                )
            }
        }
        Err(e) => (
            PeerMessage::FilePreviewResponse {
                req_id,
                path,
                content: String::new(),
                error: Some(e.to_string()),
            },
            TrafficLane::BulkP1,
        ),
    }
}
