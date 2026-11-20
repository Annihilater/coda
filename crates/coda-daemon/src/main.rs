use std::{
    io::BufRead,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use clap::{Parser, Subcommand};
use coda_daemon::{
    active_daemon_lock_pid, list_account_hosts, login_with_password_and_enroll_with_identity,
    logout_account_session, maybe_renew_daemon_config, mint_peer_relay_config,
    pair_daemon_with_code_and_identity, parse_relay_ticket_exp_unix,
    poll_device_login_until_approved, register_client_identity,
    run_daemon_once_with_p2p_and_peers, spawn_direct_p2p_listener_with_peers,
    start_device_login_flow, subscribe_account_events_ws, try_acquire_daemon_lock, DaemonConfig,
    LivePeerSession, PeerAsyncEvent, TrustedPeersStore,
};
use coda_protocol::{current_unix_ms, PeerMessage, TrafficLane};
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
        /// Read password from standard input (avoids exposing secrets in process arguments)
        #[arg(long, default_value_t = false)]
        password_stdin: bool,
    },
    /// Verify real-time account push (`/api/v1/events/ws`) and E2EE `herdr-core` mesh RPCs across online hosts
    VerifyMesh {
        #[arg(long, env = "CODA_SERVER_URL")]
        server: String,
        #[arg(long)]
        email: String,
        #[arg(long, env = "CODA_LOGIN_PASSWORD")]
        password: Option<String>,
        #[arg(long, default_value_t = false)]
        password_stdin: bool,
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
    /// Show current paired device, lock/process health, herdr-core & relay ticket expiry status
    Status,
    /// Stop the running background coda-daemon service or locked process
    Stop,
    /// Restart the background coda-daemon service (or signal reload)
    Restart,
    /// Sign out locally, revoke session token if present, and remove daemon config
    Logout,
    /// Stop and remove the background launchd/systemd service definition
    Uninstall,
    /// Run persistent encrypted mesh tunnel bridging local `herdr-core` (`hc`)
    Run {
        /// Optional preferred relay mode: `raw_tcp_no_sni` (Domestic Cloud) or `cdn_wss` (Overseas CDN)
        #[arg(long)]
        mode: Option<String>,
        /// Direct P2P listener bind address (wildcard binds dual-stack IPv6 + IPv4 automatically)
        #[arg(long, default_value = "[::]:24444")]
        p2p_bind: SocketAddr,
    },
}

fn signal_running_daemon(cfg_path: &std::path::Path, sig: libc::c_int) -> bool {
    if let Some(pid) = active_daemon_lock_pid(cfg_path) {
        if pid > 0 {
            let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
            return rc == 0;
        }
    }
    false
}

fn stop_daemon_service_or_process(cfg_path: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let plist = PathBuf::from(&home).join("Library/LaunchAgents/dev.coda.daemon.plist");
        if plist.exists() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", &plist.to_string_lossy()])
                .status();
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "stop", "coda-daemon.service"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["stop", "coda-daemon.service"])
            .status();
    }
    if signal_running_daemon(cfg_path, libc::SIGTERM) {
        println!("Sent SIGTERM to running coda-daemon.");
    } else {
        println!("No active coda-daemon process detected.");
    }
    Ok(())
}

fn restart_daemon_service_or_process(cfg_path: &std::path::Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let plist = PathBuf::from(&home).join("Library/LaunchAgents/dev.coda.daemon.plist");
        if plist.exists() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", &plist.to_string_lossy()])
                .status();
            let status = std::process::Command::new("launchctl")
                .args(["load", "-w", &plist.to_string_lossy()])
                .status()?;
            if status.success() {
                println!("Restarted launchd service dev.coda.daemon.");
                return Ok(());
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(st) = std::process::Command::new("systemctl")
            .args(["--user", "restart", "coda-daemon.service"])
            .status()
        {
            if st.success() {
                println!("Restarted user systemd service coda-daemon.service.");
                return Ok(());
            }
        }
        if let Ok(st) = std::process::Command::new("systemctl")
            .args(["restart", "coda-daemon.service"])
            .status()
        {
            if st.success() {
                println!("Restarted system systemd service coda-daemon.service.");
                return Ok(());
            }
        }
    }
    if signal_running_daemon(cfg_path, libc::SIGHUP) {
        println!("Sent SIGHUP reload signal to running coda-daemon.");
    } else {
        println!("No service manager or running coda-daemon found; start via `coda-daemon run`.");
    }
    Ok(())
}

fn read_password_from_stdin() -> Result<String> {
    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("failed reading password from stdin")?;
    let trimmed = line.trim_end_matches(&['\r', '\n'][..]).to_string();
    if trimmed.is_empty() {
        anyhow::bail!("empty password received on stdin");
    }
    Ok(trimmed)
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
            password_stdin,
        } => {
            let resolved_password = if password_stdin {
                Some(read_password_from_stdin()?)
            } else {
                password
            };
            if let (Some(email_val), Some(pass_val)) = (email, resolved_password) {
                let existing_identity = DaemonConfig::load(&cfg_path).ok().map(|c| c.identity);
                let cfg = login_with_password_and_enroll_with_identity(
                    &server,
                    &email_val,
                    &pass_val,
                    &role,
                    name,
                    existing_identity,
                )
                .await?;
                cfg.save(&cfg_path)?;
                if signal_running_daemon(&cfg_path, libc::SIGHUP) {
                    println!("Signaled running coda-daemon to reload updated credentials.");
                }
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
            if signal_running_daemon(&cfg_path, libc::SIGHUP) {
                println!("Signaled running coda-daemon to reload updated credentials.");
            }
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
            password_stdin,
            expect_hosts,
        } => {
            let resolved_password = if password_stdin {
                read_password_from_stdin()?
            } else {
                password.context("verify-mesh requires --password, CODA_LOGIN_PASSWORD, or --password-stdin")?
            };
            let clean_server = server.trim().trim_end_matches('/').to_string();
            let http = reqwest::Client::new();
            let login_resp = http
                .post(format!("{clean_server}/api/v1/auth/login"))
                .json(&serde_json::json!({
                    "email_or_handle": email,
                    "password": resolved_password,
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
                    &host.ed25519_pubkey,
                    &host.x25519_pubkey,
                )
                .await
                .ok();
                let peer = LivePeerSession::connect_auto(
                    client_id,
                    client_dev_id,
                    host.id,
                    host.identity_pin(),
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
            let existing_identity = DaemonConfig::load(&cfg_path).ok().map(|c| c.identity);
            let cfg = pair_daemon_with_code_and_identity(&server, &code, name, existing_identity)
                .await?;
            cfg.save(&cfg_path)?;
            if signal_running_daemon(&cfg_path, libc::SIGHUP) {
                info!("Signaled running coda-daemon to reload updated credentials.");
            }
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
            let daemon_state = match active_daemon_lock_pid(&cfg_path) {
                Some(pid) if pid > 0 => format!("running (pid={pid})"),
                Some(_) => "running (lock held)".into(),
                None => "stopped".into(),
            };
            let herdr_state = match coda_herdr::discover_hc_binary() {
                Some(bin) => format!("available ({})", bin.display()),
                None => "not installed (using built-in native PTY shell fallback)".into(),
            };
            let ticket_state = match parse_relay_ticket_exp_unix(&cfg.daemon_relay_ticket) {
                Some(exp) => {
                    let now = (current_unix_ms() / 1000) as i64;
                    let rem = exp - now;
                    if rem <= 0 {
                        "EXPIRED (will auto-renew on run)".into()
                    } else {
                        let days = rem / 86_400;
                        let hours = (rem % 86_400) / 3600;
                        format!("valid ({days}d {hours}h remaining; auto-renews <7d)")
                    }
                }
                None => "unknown".into(),
            };
            println!("Config:     {}", cfg_path.display());
            println!("Daemon:     {daemon_state}");
            println!("Device:     {} ({})", cfg.identity.name, cfg.device_id);
            println!("Account:    @{}", cfg.owner_handle);
            println!("Server URL: {}", cfg.server_url);
            println!("Ticket:     {ticket_state}");
            println!("Herdr-Core: {herdr_state}");
            println!("Relays:     {}", cfg.relays.len());
            for r in &cfg.relays {
                println!("  - [{}] {} ({})", r.mode, r.endpoint, r.id);
            }
        }
        Commands::Stop => {
            stop_daemon_service_or_process(&cfg_path)?;
        }
        Commands::Restart => {
            restart_daemon_service_or_process(&cfg_path)?;
        }
        Commands::Logout => {
            let _ = stop_daemon_service_or_process(&cfg_path);
            if let Ok(cfg) = DaemonConfig::load(&cfg_path) {
                if let Some(tok) = cfg.session_token.as_deref() {
                    let _ = logout_account_session(&cfg.server_url, tok).await;
                }
            }
            if cfg_path.exists() {
                std::fs::remove_file(&cfg_path)?;
                println!("Removed daemon config at {}", cfg_path.display());
            } else {
                println!("No daemon config found at {}", cfg_path.display());
            }
            let tp = DaemonConfig::trusted_peers_path(&cfg_path);
            let _ = std::fs::remove_file(&tp);
        }
        Commands::Uninstall => {
            let _ = stop_daemon_service_or_process(&cfg_path);
            #[cfg(target_os = "macos")]
            {
                let home = std::env::var("HOME").unwrap_or_default();
                let plist = PathBuf::from(&home).join("Library/LaunchAgents/dev.coda.daemon.plist");
                if plist.exists() {
                    let _ = std::fs::remove_file(&plist);
                    println!("Removed {}", plist.display());
                }
            }
            #[cfg(target_os = "linux")]
            {
                let home = std::env::var("HOME").unwrap_or_default();
                let user_unit = PathBuf::from(&home).join(".config/systemd/user/coda-daemon.service");
                if user_unit.exists() {
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "disable", "--now", "coda-daemon.service"])
                        .status();
                    let _ = std::fs::remove_file(&user_unit);
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "daemon-reload"])
                        .status();
                    println!("Removed {}", user_unit.display());
                }
            }
            println!("Uninstalled coda-daemon background service. Run `coda-daemon logout` to also remove credentials.");
        }
        Commands::Run { mode, p2p_bind } => {
            let _daemon_lock = try_acquire_daemon_lock(&cfg_path)?
                .context("another coda-daemon or embedded Coda host already holds the lock")?;
            let mut cfg = DaemonConfig::load(&cfg_path)?;
            let peers_store = TrustedPeersStore::from_daemon_config(
                &cfg,
                Some(DaemonConfig::trusted_peers_path(&cfg_path)),
            );
            let _ = maybe_renew_daemon_config(&mut cfg, Some(&cfg_path), &peers_store, false).await;
            let _ = peers_store.refresh_from_server(true).await;

            let bound_p2p = match spawn_direct_p2p_listener_with_peers(
                cfg.clone(),
                p2p_bind,
                Arc::clone(&peers_store),
            )
            .await
            {
                Ok(addr) => Some(addr),
                Err(e) => {
                    let fallback = SocketAddr::new(p2p_bind.ip(), 0);
                    match spawn_direct_p2p_listener_with_peers(
                        cfg.clone(),
                        fallback,
                        Arc::clone(&peers_store),
                    )
                    .await
                    {
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
                let udp_bind = SocketAddr::from(([0, 0, 0, 0], coda_daemon::DEFAULT_DIRECT_P2P_PORT));
                if let Err(e) = coda_daemon::spawn_lan_udp_beacon(cfg.device_id, port, udp_bind).await {
                    warn!("UDP LAN discovery beacon not bound on {udp_bind} ({e})");
                }
            }

            let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let mut backoff_secs = 1u64;

            loop {
                let _ = maybe_renew_daemon_config(&mut cfg, Some(&cfg_path), &peers_store, false)
                    .await;
                let started = tokio::time::Instant::now();
                let run_fut = run_daemon_once_with_p2p_and_peers(
                    &cfg,
                    mode.as_deref(),
                    bound_port,
                    Arc::clone(&peers_store),
                );
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        info!("coda-daemon received SIGINT; shutting down cleanly");
                        break;
                    }
                    _ = sigterm.recv() => {
                        info!("coda-daemon received SIGTERM; shutting down cleanly");
                        break;
                    }
                    _ = sighup.recv() => {
                        info!("coda-daemon received SIGHUP; reloading {}", cfg_path.display());
                        if let Ok(reloaded) = DaemonConfig::load(&cfg_path) {
                            cfg = reloaded;
                            peers_store.update_relay_ticket(cfg.daemon_relay_ticket.clone()).await;
                            let _ = peers_store.refresh_from_server(true).await;
                        }
                        backoff_secs = 1;
                        continue;
                    }
                    res = run_fut => {
                        match res {
                            Ok(()) => {
                                if started.elapsed() >= Duration::from_secs(10) {
                                    backoff_secs = 1;
                                }
                                warn!("tunnel closed cleanly; reconnecting in {backoff_secs}s...");
                            }
                            Err(e) => {
                                let msg = e.to_string().to_ascii_lowercase();
                                if msg.contains("expired") || msg.contains("ticket") || msg.contains("unauthorized") {
                                    let _ = maybe_renew_daemon_config(&mut cfg, Some(&cfg_path), &peers_store, true).await;
                                }
                                if started.elapsed() >= Duration::from_secs(10) {
                                    backoff_secs = 1;
                                }
                                error!("tunnel disconnected ({e}); reconnecting in {backoff_secs}s...");
                            }
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                            _ = tokio::signal::ctrl_c() => break,
                            _ = sigterm.recv() => break,
                            _ = sighup.recv() => {
                                if let Ok(reloaded) = DaemonConfig::load(&cfg_path) {
                                    cfg = reloaded;
                                    peers_store.update_relay_ticket(cfg.daemon_relay_ticket.clone()).await;
                                }
                                backoff_secs = 1;
                                continue;
                            }
                        }
                        backoff_secs = (backoff_secs * 2).min(30);
                    }
                }
            }
        }
    }
    Ok(())
}
