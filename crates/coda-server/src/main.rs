use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use coda_server::{relay::RelayHub, start_full_server};
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "coda-server", about = "Coda Multi-Tenant Coordinator & Anti-UDP-Block Relay Server")]
struct Args {
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

    /// Public base URL advertised in pairing commands
    #[arg(long, env = "CODA_PUBLIC_URL")]
    public_url: Option<String>,

    /// Static web assets directory (`web/dist`)
    #[arg(long, env = "CODA_WEB_DIST", default_value = "web/dist")]
    web_dist: PathBuf,

    /// Run ONLY as a stateless domestic relay node (zero PostgreSQL needed!)
    #[arg(long, default_value_t = false)]
    relay_only: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,coda_server=debug".into()),
        )
        .init();

    let args = Args::parse();

    if args.relay_only {
        let hub = RelayHub::new(args.relay_secret.into_bytes(), "CN-Stateless-Edge", None);
        let addr = hub.spawn_raw_tcp_listener(args.raw_tcp_bind).await?;
        info!("Stateless Relay-Only mode active on {addr} (no database required)");
        tokio::signal::ctrl_c().await?;
        return Ok(());
    }

    let running = start_full_server(
        &args.database_url,
        args.http_bind,
        args.raw_tcp_bind,
        args.relay_secret.into_bytes(),
        args.public_url,
        Some(args.web_dist),
    )
    .await?;

    info!(
        "Coda Coordinator & Web Console ready at http://{} | Domestic No-ICP Raw TCP Relay at {}",
        running.http_addr, running.raw_tcp_addr
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
