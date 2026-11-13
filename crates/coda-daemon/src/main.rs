use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use coda_daemon::{
    pair_daemon_with_code, poll_device_login_until_approved, spawn_direct_p2p_listener,
    start_device_login_flow, DaemonConfig,
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
    /// Authorize this Mac or Linux server via browser confirmation link (RFC 8628 Device Flow)
    Login {
        #[arg(long, env = "CODA_SERVER_URL", default_value = "http://127.0.0.1:8080")]
        server: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "daemon")]
        role: String,
    },
    /// Pair this Mac or Linux server to your Coda account using a 6-digit code
    Pair {
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        server: String,
        #[arg(long)]
        code: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Show current paired device & relay configuration status
    Status,
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
        Commands::Login { server, name, role } => {
            let existing_identity = DaemonConfig::load(&cfg_path).ok().map(|c| c.identity);
            let started = start_device_login_flow(&server, &role, name, existing_identity).await?;
            println!();
            println!("===============================================================");
            println!("  Coda Browser Device Authorization (Zero-Knowledge E2EE)");
            println!("===============================================================");
            println!("  1) Open this URL in any browser (PC / Mac / Phone):");
            println!("     {}", started.verification_uri_complete);
            println!("  2) Or visit {} and enter code: {}", started.verification_uri, started.user_code);
            println!("  Waiting for browser approval (valid for {}s)...", started.expires_in_secs);
            println!("===============================================================");
            println!();

            let cfg = poll_device_login_until_approved(&started).await?;
            cfg.save(&cfg_path)?;
            println!(
                "Authorized `{}` (device_id={}) under @{}! Saved config to {}",
                cfg.identity.name,
                cfg.device_id,
                cfg.owner_handle,
                cfg_path.display()
            );
        }
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
        Commands::Status => {
            let cfg = DaemonConfig::load(&cfg_path)?;
            println!("Config:     {}", cfg_path.display());
            println!("Device:     {} ({})", cfg.identity.name, cfg.device_id);
            println!("Account:    @{}", cfg.owner_handle);
            println!("Server URL: {}", cfg.server_url);
            println!("Relays:     {}", cfg.relays.len());
            for r in &cfg.relays {
                println!("  - [{}] {} ({})", r.mode, r.endpoint, r.id);
            }
        }
        Commands::Run { mode, p2p_bind } => {
            let cfg = DaemonConfig::load(&cfg_path)?;
            let bound_p2p = match spawn_direct_p2p_listener(cfg.clone(), p2p_bind).await {
                Ok(addr) => Some(addr),
                Err(e) => {
                    let fallback = SocketAddr::new(p2p_bind.ip(), 0);
                    match spawn_direct_p2p_listener(cfg.clone(), fallback).await {
                        Ok(addr) => Some(addr),
                        Err(_) => {
                            warn!("Direct P2P listener on {p2p_bind} not bound ({e}); continuing with Relay tunnel");
                            None
                        }
                    }
                }
            };
            let bound_port = bound_p2p.map(|a| a.port());
            if let Some(port) = bound_port {
                let udp_bind = SocketAddr::new(p2p_bind.ip(), coda_daemon::DEFAULT_DIRECT_P2P_PORT);
                if let Err(e) = coda_daemon::spawn_lan_udp_beacon(cfg.device_id, port, udp_bind).await {
                    warn!("UDP LAN discovery beacon not bound on {udp_bind} ({e})");
                }
            }
            loop {
                if let Err(e) =
                    coda_daemon::run_daemon_once_with_p2p(&cfg, mode.as_deref(), bound_port).await
                {
                    error!("tunnel disconnected ({e}); reconnecting in 2s...");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    }
    Ok(())
}
