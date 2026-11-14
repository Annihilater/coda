use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use clap::{Parser, Subcommand};
use coda_daemon::{
    list_account_hosts, login_with_password_and_enroll, mint_peer_relay_config,
    pair_daemon_with_code, poll_device_login_until_approved, register_client_identity,
    spawn_direct_p2p_listener, start_device_login_flow, subscribe_account_events_ws,
    DaemonConfig, LivePeerSession, PeerAsyncEvent,
};
use coda_protocol::{PeerMessage, TrafficLane};
use tracing::{error, info, warn};
use uuid::Uuid;

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
    /// Authorize this Mac or Linux server via browser confirmation link (RFC 8628 Device Flow) or non-interactive email/password
    Login {
        #[arg(long, env = "CODA_SERVER_URL", default_value = "https://codahub.dev")]
        server: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "daemon")]
        role: String,
        #[arg(long, env = "CODA_LOGIN_EMAIL")]
        email: Option<String>,
        #[arg(long, env = "CODA_LOGIN_PASSWORD")]
        password: Option<String>,
    },
    /// Verify real-time account push (`/api/v1/events/ws`) and E2EE `herdr-core` mesh RPCs across online hosts
    VerifyMesh {
        #[arg(long, env = "CODA_SERVER_URL")]
        server: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        password: String,
        #[arg(long, default_value_t = 1)]
        expect_hosts: usize,
    },
    /// Pair this Mac or Linux server to your Coda account using a 6-digit code
    Pair {
        #[arg(long, env = "CODA_SERVER_URL", default_value = "https://codahub.dev")]
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
        Commands::Login {
            server,
            name,
            role,
            email,
            password,
        } => {
            if let (Some(email_val), Some(pass_val)) = (email, password) {
                let cfg = login_with_password_and_enroll(&server, &email_val, &pass_val, &role, name)
                    .await?;
                cfg.save(&cfg_path)?;
                println!(
                    "Authorized `{}` (device_id={}) under @{}! Saved config to {}",
                    cfg.identity.name,
                    cfg.device_id,
                    cfg.owner_handle,
                    cfg_path.display()
                );
                return Ok(());
            }
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
        Commands::VerifyMesh {
            server,
            email,
            password,
            expect_hosts,
        } => {
            let clean_server = server.trim().trim_end_matches('/').to_string();
            let http = reqwest::Client::new();
            let login_resp = http
                .post(format!("{clean_server}/api/v1/auth/login"))
                .json(&serde_json::json!({
                    "email_or_handle": email,
                    "password": password,
                    "client_label": "coda-mesh-verifier",
                }))
                .send()
                .await?
                .error_for_status()
                .context("verify-mesh login failed")?
                .json::<serde_json::Value>()
                .await?;
            let token = login_resp["token"]
                .as_str()
                .context("verify-mesh missing session token")?
                .to_string();

            let mut ws_rx = subscribe_account_events_ws(&clean_server, &token).await?;
            let first_ev = tokio::time::timeout(Duration::from_secs(5), ws_rx.recv())
                .await
                .context("timed out waiting for /api/v1/events/ws connected event")?
                .context("events ws closed before connected event")?;
            if first_ev.kind != "connected" {
                anyhow::bail!("unexpected first events ws kind: {}", first_ev.kind);
            }

            let wait_deadline = tokio::time::Instant::now() + Duration::from_secs(12);
            let online_hosts = loop {
                let hosts: Vec<_> = list_account_hosts(&clean_server, &token)
                    .await?
                    .into_iter()
                    .filter(|h| h.is_online)
                    .collect();
                if hosts.len() >= expect_hosts {
                    break hosts;
                }
                if tokio::time::Instant::now() >= wait_deadline {
                    anyhow::bail!(
                        "expected at least {} online hosts, found {}: {:?}",
                        expect_hosts,
                        hosts.len(),
                        hosts.iter().map(|h| (&h.name, &h.platform)).collect::<Vec<_>>()
                    );
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            };

            let mut verified = Vec::new();
            for host in &online_hosts {
                let (client_dev_id, client_id) = register_client_identity(
                    &clean_server,
                    &token,
                    &format!("verify-client-{}", &host.id.to_string()[..8]),
                    "macos",
                )
                .await?;
                let relay_cfg = mint_peer_relay_config(
                    &clean_server,
                    &token,
                    client_dev_id,
                    host.id,
                    &host.x25519_pubkey,
                )
                .await
                .ok();
                let peer = LivePeerSession::connect_auto(
                    client_id,
                    client_dev_id,
                    host.id,
                    host.endpoints.clone(),
                    None,
                    relay_cfg,
                )
                .await
                .with_context(|| format!("failed connecting to host `{}` ({})", host.name, host.id))?;
                let mut rx = peer.subscribe_events();

                let ws_reply = peer
                    .request(
                        PeerMessage::ListWorkspaces {
                            req_id: format!("vw-{}", Uuid::new_v4().simple()),
                            root_dirs: Vec::new(),
                        },
                        TrafficLane::BulkP1,
                        Duration::from_secs(10),
                    )
                    .await?;
                let repos = match ws_reply {
                    PeerMessage::WorkspacesSnapshot { repos, .. } => repos,
                    other => anyhow::bail!("unexpected ListWorkspaces reply from {}: {:?}", host.name, other),
                };

                let mut checked_diff = false;
                if let Some(first_repo) = repos.first() {
                    let diff_reply = peer
                        .request(
                            PeerMessage::GitDiffRequest {
                                req_id: format!("vd-{}", Uuid::new_v4().simple()),
                                cwd: first_repo.root_path.clone(),
                            },
                            TrafficLane::BulkP1,
                            Duration::from_secs(10),
                        )
                        .await?;
                    if let PeerMessage::GitDiffResponse { branch, .. } = diff_reply {
                        checked_diff = !branch.is_empty();
                    }
                }

                let launch_reply = peer
                    .request(
                        PeerMessage::LaunchAgent {
                            req_id: format!("vl-{}", Uuid::new_v4().simple()),
                            agent: "sh".to_string(),
                            name: Some(format!("rust-mesh-verify-{}", &Uuid::new_v4().simple().to_string()[..8])),
                            cwd: repos.first().map(|r| r.root_path.clone()),
                            initial_prompt: None,
                            cols: 100,
                            rows: 30,
                        },
                        TrafficLane::ControlP0,
                        Duration::from_secs(10),
                    )
                    .await?;
                let term_addr = match launch_reply {
                    PeerMessage::LaunchAgentResult {
                        ok: true,
                        terminal: Some(t),
                        ..
                    } => t.address,
                    other => anyhow::bail!("LaunchAgent failed on {}: {:?}", host.name, other),
                };

                let att_reply = peer
                    .request(
                        PeerMessage::PtyAttach {
                            req_id: format!("va-{}", Uuid::new_v4().simple()),
                            terminal: term_addr.clone(),
                            cols: 100,
                            rows: 30,
                        },
                        TrafficLane::ControlP0,
                        Duration::from_secs(10),
                    )
                    .await?;
                match att_reply {
                    PeerMessage::PtyAttached { ok: true, .. } => {}
                    other => anyhow::bail!("PtyAttach failed on {}: {:?}", host.name, other),
                }

                let short_id = host.id.to_string()[..8].to_string();
                let marker = format!("RUST_DESKTOP_MESH_OK_{short_id}");
                peer.send_oneway(
                    PeerMessage::PtyInput {
                        terminal: term_addr.clone(),
                        data_b64: B64.encode(format!("echo {marker}\n")),
                    },
                    TrafficLane::ControlP0,
                )
                .await?;

                let mut captured = String::new();
                let chunk_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                while !captured.contains(&marker) {
                    let rem = chunk_deadline.saturating_duration_since(tokio::time::Instant::now());
                    if rem.is_zero() {
                        anyhow::bail!(
                            "timed out waiting for `{marker}` from host `{}` ({}); captured={:?}",
                            host.name,
                            host.id,
                            captured
                        );
                    }
                    match tokio::time::timeout(rem, rx.recv()).await {
                        Ok(Ok(PeerAsyncEvent::PtyOutputChunk {
                            terminal,
                            data_b64,
                            ..
                        })) if terminal == term_addr => {
                            if let Ok(bytes) = B64.decode(&data_b64) {
                                captured.push_str(&String::from_utf8_lossy(&bytes));
                            }
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                        Ok(Err(_)) | Err(_) => break,
                    }
                }
                if !captured.contains(&marker) {
                    anyhow::bail!("missing `{marker}` in PTY output on `{}`", host.name);
                }

                let _ = peer
                    .send_oneway(
                        PeerMessage::PtyInput {
                            terminal: term_addr.clone(),
                            data_b64: B64.encode("exit\n"),
                        },
                        TrafficLane::ControlP0,
                    )
                    .await;
                let _ = tokio::time::sleep(Duration::from_millis(120)).await;
                peer.close();

                verified.push(serde_json::json!({
                    "id": host.id,
                    "name": host.name,
                    "platform": host.platform,
                    "route": peer.route_label(),
                    "terminal": term_addr,
                    "checked_diff": checked_diff,
                }));
            }

            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ok": true,
                    "verified_hosts": verified,
                }))?
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
