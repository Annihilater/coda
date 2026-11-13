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
    #[arg(long, env = "DATABASE_URL", default_value = "postgres://vampire@localhost:5432/coda_dev")]
    database_url: String,

    /// HTTP / Overseas CDN WebSocket bind address
    #[arg(long, env = "CODA_HTTP_BIND", default_value = "0.0.0.0:8080")]
    http_bind: SocketAddr,

    /// Domestic Aliyun/Tencent Cloud No-ICP Raw TCP Relay bind address (zero HTTP/SNI)
    #[arg(long, env = "CODA_RAW_TCP_BIND", default_value = "0.0.0.0:24443")]
    raw_tcp_bind: SocketAddr,

    /// Shared HMAC secret between `coda-coord` and stateless `coda-relay` edge nodes
    #[arg(long, env = "CODA_RELAY_SECRET", default_value = "coda-dev-hmac-secret-change-in-prod")]
    relay_secret: String,

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

    /// Release version advertised on the public download center (e.g. `v0.3.0`)
    #[arg(long, env = "CODA_RELEASE_VERSION", default_value = concat!("v", env!("CARGO_PKG_VERSION")))]
    release_version: String,

    /// GitHub repository slug used to build download links
    #[arg(long, env = "CODA_GITHUB_REPO", default_value = "Annihilater/coda")]
    github_repo: String,

    /// Optional iOS TestFlight / Ad-hoc OTA install URL
    #[arg(long, env = "CODA_IOS_BETA_URL")]
    ios_beta_url: Option<String>,

    /// Run ONLY as a stateless domestic relay node (zero PostgreSQL needed!)
    #[arg(long, default_value_t = false)]
    relay_only: bool,
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
    let val = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
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

async fn async_main(args: Args) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,coda_server=debug".into()),
        )
        .init();

    let _ = &args.env_file;

    if args.relay_only {
        let hub = RelayHub::new(args.relay_secret.into_bytes(), &args.region_label, None);
        let addr = hub.spawn_raw_tcp_listener(args.raw_tcp_bind).await?;
        info!(
            "Stateless Relay-Only mode active on {addr} (region={}, no database required)",
            args.region_label
        );
        tokio::signal::ctrl_c().await?;
        return Ok(());
    }

    let running = start_full_server_with_options(
        &args.database_url,
        args.http_bind,
        args.raw_tcp_bind,
        args.relay_secret.into_bytes(),
        args.public_url,
        Some(args.web_dist),
        ServerOptions {
            open_registration: args.open_registration,
            raw_tcp_public_endpoint: args.raw_tcp_public_endpoint,
            release_version: args.release_version,
            github_repo: args.github_repo,
            ios_beta_url: args.ios_beta_url,
        },
    )
    .await?;

    info!(
        "Coda Coordinator & Web Console ready at http://{} | Domestic No-ICP Raw TCP Relay at {}",
        running.http_addr, running.raw_tcp_addr
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_dotenv_line;

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
    }
}
