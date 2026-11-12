use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use coda_daemon::{
    pair_daemon_with_code, run_daemon_once, spawn_direct_p2p_listener, DaemonConfig,
};
use tracing::{error, info, warn};

#[derive(Debug, Parser)]
#[command(name = "coda-daemon", about = "Coda Zero-Knowledge Host Daemon (macOS & Linux)")]
struct Cli {
    #[arg(long, env = "CODA_DAEMON_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Pair this Mac or Linux server to your Coda account using a 6-digit code
    Pair {
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        server: String,
        #[arg(long)]
        code: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Run persistent encrypted mesh tunnel bridging local `herdr-core` (`hc`)
    Run {
        /// Optional preferred relay mode: `raw_tcp_no_sni` (Domestic Cloud) or `cdn_wss` (Overseas CDN)
        #[arg(long)]
        mode: Option<String>,
        /// Direct P2P listener bind address for LAN IPv4 / Mainland China IPv6 zero-relay connections
        #[arg(long, default_value = "0.0.0.0:24444")]
        p2p_bind: SocketAddr,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,coda_daemon=debug".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg_path = cli.config.unwrap_or_else(DaemonConfig::default_path);

    match cli.cmd {
        Commands::Pair { server, code, name } => {
            let cfg = pair_daemon_with_code(&server, &code, name).await?;
            cfg.save(&cfg_path)?;
            info!(
                "Successfully paired `{}` (device_id={}) to account @{}! Saved config to {}",
                cfg.identity.name,
                cfg.device_id,
                cfg.owner_handle,
                cfg_path.display()
            );
        }
        Commands::Run { mode, p2p_bind } => {
            let cfg = DaemonConfig::load(&cfg_path)?;
            if let Err(e) = spawn_direct_p2p_listener(cfg.clone(), p2p_bind).await {
                warn!("Direct P2P listener on {p2p_bind} not bound ({e}); continuing with Relay tunnel");
            }
            loop {
                if let Err(e) = run_daemon_once(&cfg, mode.as_deref()).await {
                    error!("tunnel disconnected ({e}); reconnecting in 2s...");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    }
    Ok(())
}
