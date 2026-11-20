use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use coda_server::{relay::RelayHub, start_full_server_with_options, ServerOptions};
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "coda-server", about = "Coda Multi-Tenant Coordinator & Anti-UDP-Block Relay Server")]
struct Args {
    /// Optional path to a `.env` file (auto-discovers `./.env` and `/etc/coda/.env` as well)
    #[arg(long, env = "CODA_ENV_FILE")]
    env_file: Option<PathBuf>,

    /// PostgreSQL connection string (not required in --relay-only mode)
    #[arg(long, env = "DATABASE_URL", default_value = "postgres://postgres@localhost:5432/coda_dev")]
    database_url: String,

    /// HTTP / Overseas CDN WebSocket bind address
    #[arg(long, env = "CODA_HTTP_BIND", default_value = "0.0.0.0:8080")]
    http_bind: SocketAddr,

    /// Domestic Aliyun/Tencent Cloud No-ICP Raw TCP Relay bind address (zero HTTP/SNI)
    #[arg(long, env = "CODA_RAW_TCP_BIND", default_value = "0.0.0.0:24443")]
    raw_tcp_bind: SocketAddr,

    /// Shared HMAC secret between `coda-coord` and stateless `coda-relay` edge nodes
    #[arg(long, env = "CODA_RELAY_SECRET")]
    relay_secret: Option<String>,

    /// Development-only escape hatch: allow ephemeral loopback relay secret if CODA_RELAY_SECRET is omitted
    #[arg(long, env = "CODA_ALLOW_INSECURE_DEV_SECRET", default_value_t = false)]
    allow_insecure_dev_secret: bool,

    /// Public region label advertised by stateless `--relay-only` node
    #[arg(long, env = "CODA_REGION_LABEL", default_value = "CN-BGP-NoICP")]
    region_label: String,

    /// Public base URL advertised in pairing commands
    #[arg(long, env = "CODA_PUBLIC_URL")]
    public_url: Option<String>,

    /// Static web assets directory (`web/dist`)
    #[arg(long, env = "CODA_WEB_DIST", default_value = "web/dist")]
    web_dist: PathBuf,

    /// Allow anyone to register without an invite code (recommended for public portal)
    #[arg(long, env = "CODA_OPEN_REGISTRATION", default_value_t = false)]
    open_registration: bool,

    /// Optional public Raw TCP relay endpoint (`host:port`); omit when behind Cloudflare Tunnel HTTPS
    #[arg(long, env = "CODA_RAW_TCP_PUBLIC_ENDPOINT")]
    raw_tcp_public_endpoint: Option<String>,

    /// Optional pinned release version advertised on the public download center (e.g. `v0.4.0`)
    #[arg(long, env = "CODA_RELEASE_VERSION")]
    release_version: Option<String>,

    /// GitHub repository slug used to build download links
    #[arg(long, env = "CODA_GITHUB_REPO", default_value = "Annihilater/coda")]
    github_repo: String,

    /// Optional iOS TestFlight / Ad-hoc OTA install URL
    #[arg(long, env = "CODA_IOS_BETA_URL")]
    ios_beta_url: Option<String>,

    /// Trust proxy forwarding headers (`CF-Connecting-IP`, `X-Real-IP`, `X-Forwarded-Host/Proto`) even from non-private peer IPs
    #[arg(long, env = "CODA_TRUST_PROXY_HEADERS", default_value_t = false)]
    trust_proxy_headers: bool,

    /// Comma-separated additional allowed browser CORS/WebSocket origins (e.g. `https://app.example.com`)
    #[arg(long, env = "CODA_ALLOWED_ORIGINS")]
    allowed_origins: Option<String>,

    /// Run ONLY as a stateless domestic relay node (zero PostgreSQL needed!)
    #[arg(long, default_value_t = false)]
    relay_only: bool,
}

fn unescape_double_quoted(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_dotenv_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let without_export = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim_start();
    let (raw_key, raw_val) = without_export.split_once('=')?;
    let key = raw_key.trim();
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let v = raw_val.trim();
    let val = if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
        unescape_double_quoted(&v[1..v.len() - 1])
    } else if v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2 {
        v[1..v.len() - 1].to_string()
    } else {
        v.split(" #").next().unwrap_or(v).trim().to_string()
    };
    Some((key.to_string(), val))
}

fn load_dotenv_file(path: &Path) {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };
    for line in contents.lines() {
        if let Some((key, val)) = parse_dotenv_line(line) {
            if std::env::var_os(&key).is_none() {
                std::env::set_var(key, val);
            }
        }
    }
}

fn extract_cli_env_file() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let s = arg.to_string_lossy();
        if s == "--env-file" {
            if let Some(p) = args.next() {
                return Some(PathBuf::from(p));
            }
        } else if let Some(rest) = s.strip_prefix("--env-file=") {
            return Some(PathBuf::from(rest));
        }
    }
    std::env::var_os("CODA_ENV_FILE").map(PathBuf::from)
}

/// Loads `.env` on the main thread before Tokio spawns worker threads so
/// `coda-server` binary, Docker Compose, and systemd services share one `.env`.
fn preload_dotenv() {
    if let Some(explicit) = extract_cli_env_file() {
        load_dotenv_file(&explicit);
    }
    load_dotenv_file(Path::new(".env"));
    load_dotenv_file(Path::new("/etc/coda/.env"));
}

fn main() -> Result<()> {
    preload_dotenv();
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main(args))
}

fn resolve_relay_secret(
    configured: Option<&str>,
    relay_only: bool,
    allow_insecure_dev_secret: bool,
    http_bind: SocketAddr,
    raw_tcp_bind: SocketAddr,
) -> Result<Vec<u8>> {
    const INSECURE_DEFAULTS: &[&str] = &[
        "coda-dev-hmac-secret-change-in-prod",
        "change-me-shared-hmac-relay-secret-2026",
        "coda_shared_hmac_relay_secret_change_me_2026",
        "changeme",
        "secret",
        "default",
    ];
    if let Some(raw) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        let lower = raw.to_ascii_lowercase();
        if INSECURE_DEFAULTS
            .iter()
            .any(|d| d.eq_ignore_ascii_case(raw))
            || lower.contains("change_me")
            || lower.contains("change-me")
            || lower.contains("changeme")
            || lower.contains("placeholder")
        {
            anyhow::bail!(
                "refusing to start with insecure placeholder CODA_RELAY_SECRET; generate a random secret with `openssl rand -hex 32`"
            );
        }
        if raw.len() < 24 {
            anyhow::bail!(
                "CODA_RELAY_SECRET is too short ({} bytes); must be at least 24 bytes (generate with `openssl rand -hex 32`)",
                raw.len()
            );
        }
        return Ok(raw.as_bytes().to_vec());
    }

    if !relay_only
        && allow_insecure_dev_secret
        && http_bind.ip().is_loopback()
        && raw_tcp_bind.ip().is_loopback()
    {
        let ephemeral = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        tracing::warn!(
            "CODA_RELAY_SECRET omitted with --allow-insecure-dev-secret on loopback; generated ephemeral secret for this run"
        );
        return Ok(ephemeral.into_bytes());
    }

    anyhow::bail!(
        "CODA_RELAY_SECRET is required; generate a shared secret with `openssl rand -hex 32`"
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            let _ = sigterm.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    info!("Shutdown signal received; stopping Coda server gracefully");
}

async fn async_main(args: Args) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,coda_server=debug".into()),
        )
        .init();

    let _ = &args.env_file;
    let relay_secret = resolve_relay_secret(
        args.relay_secret.as_deref(),
        args.relay_only,
        args.allow_insecure_dev_secret,
        args.http_bind,
        args.raw_tcp_bind,
    )?;

    if args.relay_only {
        let hub = RelayHub::new(relay_secret, &args.region_label, None);
        let addr = hub.spawn_raw_tcp_listener(args.raw_tcp_bind).await?;
        info!(
            "Stateless Relay-Only mode active on {addr} (region={}, no database required)",
            args.region_label
        );
        shutdown_signal().await;
        return Ok(());
    }

    let explicit_release_version = args
        .release_version
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let has_explicit_release_version = explicit_release_version.is_some();
    let release_version =
        explicit_release_version.unwrap_or_else(|| format!("v{}", env!("CARGO_PKG_VERSION")));
    let allowed_origins = args
        .allowed_origins
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let running = start_full_server_with_options(
        &args.database_url,
        args.http_bind,
        args.raw_tcp_bind,
        relay_secret,
        args.public_url,
        Some(args.web_dist),
        ServerOptions {
            open_registration: args.open_registration,
            raw_tcp_public_endpoint: args.raw_tcp_public_endpoint,
            release_version,
            has_explicit_release_version,
            github_repo: args.github_repo,
            ios_beta_url: args.ios_beta_url,
            trust_proxy_headers: args.trust_proxy_headers,
            allowed_origins,
        },
    )
    .await?;

    info!(
        "Coda Coordinator & Web Console ready at http://{} | Domestic No-ICP Raw TCP Relay at {}",
        running.http_addr, running.raw_tcp_addr
    );
    shutdown_signal().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use super::{parse_dotenv_line, resolve_relay_secret};

    #[test]
    fn test_relay_secret_policy() {
        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let public = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080);

        assert!(resolve_relay_secret(None, false, false, loopback, loopback).is_err());
        assert!(resolve_relay_secret(
            Some("coda-dev-hmac-secret-change-in-prod"),
            false,
            true,
            loopback,
            loopback
        )
        .is_err());
        assert!(resolve_relay_secret(
            Some("change-me-shared-hmac-relay-secret-2026"),
            false,
            false,
            public,
            public
        )
        .is_err());
        assert!(resolve_relay_secret(
            Some("coda_shared_hmac_relay_secret_change_me_2026"),
            false,
            false,
            public,
            public
        )
        .is_err());
        assert!(resolve_relay_secret(Some("too-short"), false, false, loopback, loopback).is_err());
        assert!(resolve_relay_secret(None, false, true, public, public).is_err());
        assert!(resolve_relay_secret(None, true, true, loopback, loopback).is_err());

        let ephem = resolve_relay_secret(None, false, true, loopback, loopback).unwrap();
        assert!(ephem.len() >= 32);

        let strong = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            resolve_relay_secret(Some(strong), false, false, public, public).unwrap(),
            strong.as_bytes()
        );
    }

    #[test]
    fn test_parse_dotenv_line_variants() {
        assert_eq!(parse_dotenv_line(""), None);
        assert_eq!(parse_dotenv_line("# CODA_RELAY_SECRET=ignored"), None);
        assert_eq!(
            parse_dotenv_line("CODA_RELAY_SECRET=secret-123"),
            Some(("CODA_RELAY_SECRET".into(), "secret-123".into()))
        );
        assert_eq!(
            parse_dotenv_line("export CODA_PUBLIC_URL=\"https://coda.example.com\""),
            Some(("CODA_PUBLIC_URL".into(), "https://coda.example.com".into()))
        );
        assert_eq!(
            parse_dotenv_line("CODA_REGION_LABEL='CN-Aliyun'"),
            Some(("CODA_REGION_LABEL".into(), "CN-Aliyun".into()))
        );
        assert_eq!(
            parse_dotenv_line("CODA_RAW_TCP_BIND=0.0.0.0:24443 # no-icp"),
            Some(("CODA_RAW_TCP_BIND".into(), "0.0.0.0:24443".into()))
        );
        assert_eq!(
            parse_dotenv_line(r#"CODA_RELAY_SECRET="abc\"def\\ghi""#),
            Some(("CODA_RELAY_SECRET".into(), "abc\"def\\ghi".into()))
        );
    }
}
