//! Filesystem sandboxing, sensitive credential path filtering, bounded UTF-8 file previews,
//! and localhost port-preview SSRF guards for `coda-daemon`.

use std::path::Path;

use anyhow::Result;
use coda_protocol::{PeerMessage, RemoteFileItem, TrafficLane};
use tokio::io::AsyncReadExt;

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

    let Ok(canon_base) = tokio::fs::canonicalize(&root_path).await else {
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
    if is_sensitive_path(&canon_base) {
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
    let canon = match tokio::fs::canonicalize(&path).await {
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
    if is_sensitive_path(&canon) {
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
