//! Coda Host Daemon (`coda-daemon` for macOS & Linux Servers).
//! - Pairs via 6-digit code (`coda-daemon pair --server ... --code 839-204`).
//! - Persists Zero-Knowledge `Ed25519 + X25519` device keys locally (`0600`).
//! - Connects over SmartPath (Domestic No-ICP Raw TCP `:24443` or Overseas CDN `WSS`)
//!   with Dual-Lane QoS (`ControlP0` + `BulkP1`) and automatic 75s CDN Make-Before-Break rotation.
//! - Bridges E2EE encrypted `PeerMessage` requests directly to local `herdr-core` (`hc`),
//!   `git diff`, and localhost web port preview (`127.0.0.1:PORT`).

use std::{
    collections::HashMap,
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_herdr::{
    scan_git_workspaces, spawn_pty_attach, HerdrClient, PtyBridgeHandle, PtyOutputEvent,
};
use coda_protocol::{
    E2eeSession, NetworkEndpointCandidate, PeerMessage, RelayWireFrame, StoredDeviceIdentity,
    TrafficLane, MAX_WIRE_FRAME_BYTES,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{mpsc, Mutex},
};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMsg};
use tracing::{info, warn};
use uuid::Uuid;

/// Default Direct P2P port for LAN IPv4 & Mainland China Public IPv6 zero-relay hole-punching.
pub const DEFAULT_DIRECT_P2P_PORT: u16 = 24444;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonRelayEntry {
    pub id: String,
    pub mode: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub server_url: String,
    pub owner_handle: String,
    pub device_id: Uuid,
    pub daemon_relay_ticket: String,
    pub relays: Vec<DaemonRelayEntry>,
    pub identity: StoredDeviceIdentity,
}

impl DaemonConfig {
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".config/coda/daemon.json")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)?;
        std::fs::write(path, body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let txt = std::fs::read_to_string(path)
            .with_context(|| format!("daemon config not found at {}", path.display()))?;
        Ok(serde_json::from_str(&txt)?)
    }
}

/// Discovers LAN IPv4 and Mainland China Public IPv6 (`2408:`/`2409:`/`240e:`) socket candidates
/// without sending any external packets (`connect` on UDP socket only queries kernel routing table).
pub fn discover_local_endpoints() -> Vec<NetworkEndpointCandidate> {
    discover_local_endpoints_with_port(DEFAULT_DIRECT_P2P_PORT)
}

pub fn discover_local_endpoints_with_port(p2p_port: u16) -> Vec<NetworkEndpointCandidate> {
    let mut out = Vec::new();
    if let Ok(sock4) = UdpSocket::bind("0.0.0.0:0") {
        if sock4.connect("223.5.5.5:53").is_ok() {
            if let Ok(local) = sock4.local_addr() {
                if !local.ip().is_loopback() {
                    out.push(NetworkEndpointCandidate {
                        kind: "lan_ipv4".into(),
                        addr: format!("{}:{}", local.ip(), p2p_port),
                    });
                }
            }
        }
    }
    if let Ok(sock6) = UdpSocket::bind("[::]:0") {
        if sock6.connect("[2400:3200::1]:53").is_ok() {
            if let Ok(local) = sock6.local_addr() {
                if !local.ip().is_loopback() {
                    out.push(NetworkEndpointCandidate {
                        kind: "cn_public_ipv6".into(),
                        addr: format!("[{}]:{}", local.ip(), p2p_port),
                    });
                }
            }
        }
    }
    out
}

/// Claims a 6-digit pairing code and writes the Zero-Knowledge DaemonConfig.
pub async fn pair_daemon_with_code(
    server_url: &str,
    code: &str,
    custom_name: Option<String>,
) -> Result<DaemonConfig> {
    let hostname = custom_name.unwrap_or_else(detect_hostname);
    let mut identity = StoredDeviceIdentity::generate(&hostname);
    let sig_b64 = identity.sign(code.trim().as_bytes())?;

    let (herdr_id, herdr_ver) = match HerdrClient::connect_default().await {
        Ok(hc) => (
            Some(hc.info().core_id.clone()),
            Some(hc.info().version.clone()),
        ),
        Err(_) => (None, None),
    };

    let platform = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    let endpoints = discover_local_endpoints();
    let http = reqwest::Client::new();
    let url = format!("{}/api/v1/devices/claim-pair", server_url.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .json(&json!({
            "code": code.trim(),
            "name": hostname,
            "platform": platform,
            "ed25519_pubkey": identity.ed25519_public_b64,
            "x25519_pubkey": identity.x25519_public_b64,
            "signature_b64": sig_b64,
            "endpoints": endpoints,
            "herdr_core_id": herdr_id,
            "herdr_version": herdr_ver,
        }))
        .send()
        .await?
        .error_for_status()
        .context("pairing request rejected by server")?
        .json::<serde_json::Value>()
        .await?;

    let device_id: Uuid = serde_json::from_value(resp["device"]["id"].clone())?;
    identity.device_id = Some(device_id);
    let owner_handle = resp["owner_handle"].as_str().unwrap_or("owner").to_string();
    let daemon_relay_ticket = resp["daemon_relay_ticket"]
        .as_str()
        .ok_or_else(|| anyhow!("missing daemon_relay_ticket"))?
        .to_string();

    let relays: Vec<DaemonRelayEntry> = resp["relays"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            Some(DaemonRelayEntry {
                id: v["id"].as_str()?.to_string(),
                mode: v["mode"].as_str()?.to_string(),
                endpoint: v["endpoint"].as_str()?.to_string(),
            })
        })
        .collect();

    Ok(DaemonConfig {
        server_url: server_url.trim_end_matches('/').to_string(),
        owner_handle,
        device_id,
        daemon_relay_ticket,
        relays,
        identity,
    })
}

/// Runs a single connected Daemon session over either Raw TCP No-SNI or CDN WebSocket.
pub async fn run_daemon_once(cfg: &DaemonConfig, prefer_mode: Option<&str>) -> Result<()> {
    let chosen = cfg
        .relays
        .iter()
        .find(|r| prefer_mode.map_or(true, |m| r.mode == m))
        .or_else(|| cfg.relays.first())
        .ok_or_else(|| anyhow!("no relay nodes configured"))?;

    info!(
        "Connecting Coda Daemon `{}` ({}) via {} [{}]",
        cfg.identity.name, cfg.device_id, chosen.endpoint, chosen.mode
    );

    let (out_tx, mut out_rx) = mpsc::channel::<RelayWireFrame>(256);
    let (in_tx, mut in_rx) = mpsc::channel::<RelayWireFrame>(256);

    let bind_frame = RelayWireFrame::AuthBind {
        ticket: cfg.daemon_relay_ticket.clone(),
        lane: TrafficLane::ControlP0,
        generation: 1,
    };

    if chosen.mode == "raw_tcp_no_sni" {
        let stream = TcpStream::connect(&chosen.endpoint)
            .await
            .with_context(|| format!("failed connecting raw TCP relay {}", chosen.endpoint))?;
        let _ = stream.set_nodelay(true);
        let (mut reader, mut writer) = stream.into_split();
        writer
            .write_all(&bind_frame.encode_length_delimited()?)
            .await?;

        let ack = read_raw_frame(&mut reader).await?;
        match ack {
            RelayWireFrame::AuthAck { ok: true, .. } => {}
            other => anyhow::bail!("unexpected raw TCP relay auth response: {other:?}"),
        }

        tokio::spawn(async move {
            while let Some(f) = out_rx.recv().await {
                if let Ok(bytes) = f.encode_length_delimited() {
                    if writer.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            while let Ok(f) = read_raw_frame(&mut reader).await {
                if in_tx.send(f).await.is_err() {
                    break;
                }
            }
        });
    } else {
        let (ws_stream, _) = connect_async(&chosen.endpoint)
            .await
            .with_context(|| format!("failed connecting WSS relay {}", chosen.endpoint))?;
        let (mut sink, mut stream) = ws_stream.split();
        sink.send(WsMsg::Text(serde_json::to_string(&bind_frame)?.into()))
            .await?;

        if let Some(Ok(WsMsg::Text(txt))) = stream.next().await {
            let ack: RelayWireFrame = serde_json::from_str(&txt)?;
            if !matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }) {
                anyhow::bail!("unexpected WSS relay auth response: {ack:?}");
            }
        }

        tokio::spawn(async move {
            while let Some(f) = out_rx.recv().await {
                if let Ok(txt) = serde_json::to_string(&f) {
                    if sink.send(WsMsg::Text(txt.into())).await.is_err() {
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                if let WsMsg::Text(t) = msg {
                    if let Ok(f) = serde_json::from_str::<RelayWireFrame>(&t) {
                        if in_tx.send(f).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    let herdr = match HerdrClient::connect_default().await {
        Ok(hc) => {
            info!(
                "Bridged local herdr-core `{}` (v{})",
                hc.info().core_name,
                hc.info().version
            );
            Some(Arc::new(hc))
        }
        Err(e) => {
            warn!("herdr-core not active yet ({e}); daemon running in degraded diagnostics mode");
            None
        }
    };

    let sessions: Arc<Mutex<HashMap<Uuid, E2eeSession>>> = Arc::new(Mutex::new(HashMap::new()));
    let active_ptys: Arc<Mutex<HashMap<(Uuid, String), PtyBridgeHandle>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let my_dh_secret = cfg.identity.x25519_secret()?;
    let my_dh_pub_b64 = cfg.identity.x25519_public_b64.clone();
    let my_device_id = cfg.device_id;
    let hostname = cfg.identity.name.clone();

    while let Some(frame) = in_rx.recv().await {
        if let RelayWireFrame::RoutePacket {
            src_device_id,
            e2ee,
            sender_x25519_pub_b64,
            ..
        } = frame
        {
            let req_msg = {
                let mut map = sessions.lock().await;
                if !map.contains_key(&src_device_id) {
                    if let Some(ref peer_pub) = sender_x25519_pub_b64 {
                        if let Ok(sess) = E2eeSession::derive(&my_dh_secret, peer_pub, false) {
                            map.insert(src_device_id, sess);
                        }
                    }
                }
                let Some(sess) = map.get_mut(&src_device_id) else {
                    warn!("cannot decrypt packet from {src_device_id}: missing peer X25519 pubkey");
                    continue;
                };

                match sess.open_message(&e2ee) {
                    Ok(m) => m,
                    Err(e) => {
                        if e2ee.seq == 1 {
                            if let Some(ref peer_pub) = sender_x25519_pub_b64 {
                                if let Ok(mut fresh) =
                                    E2eeSession::derive(&my_dh_secret, peer_pub, false)
                                {
                                    if let Ok(m) = fresh.open_message(&e2ee) {
                                        *sess = fresh;
                                        m
                                    } else {
                                        warn!("E2EE open failed after re-derive: {e}");
                                        continue;
                                    }
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        } else {
                            warn!("E2EE open error from {src_device_id}: {e}");
                            continue;
                        }
                    }
                }
            };

            let maybe_reply = handle_peer_request_full(
                src_device_id,
                req_msg,
                herdr.as_deref(),
                &hostname,
                &active_ptys,
                &sessions,
                &out_tx,
                my_device_id,
                &my_dh_pub_b64,
            )
            .await;

            if let Some((reply_msg, reply_lane)) = maybe_reply {
                let mut map = sessions.lock().await;
                if let Some(sess) = map.get_mut(&src_device_id) {
                    if let Ok(sealed_reply) = sess.seal_message(&reply_msg) {
                        let _ = out_tx
                            .send(RelayWireFrame::RoutePacket {
                                src_device_id: my_device_id,
                                dst_device_id: src_device_id,
                                lane: reply_lane,
                                e2ee: sealed_reply,
                                sender_x25519_pub_b64: Some(my_dh_pub_b64.clone()),
                            })
                            .await;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Phase 3 Direct P2P Listener (`L0 LAN IPv4` + `L1 Mainland China Native IPv6 Direct`).
/// Binds a direct TCP socket (`SO_REUSEADDR`) so clients on the same LAN or Mainland China IPv6
/// can establish a `< 10ms` zero-relay E2EE session directly to `coda-daemon`.
pub async fn spawn_direct_p2p_listener(
    cfg: DaemonConfig,
    bind_addr: SocketAddr,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    info!(
        "Direct P2P zero-relay listener active on {} (device={})",
        local_addr, cfg.device_id
    );

    let herdr = HerdrClient::connect_default().await.ok().map(Arc::new);

    tokio::spawn(async move {
        while let Ok((stream, peer_addr)) = listener.accept().await {
            let cfg_clone = cfg.clone();
            let herdr_clone = herdr.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_direct_p2p_socket(stream, peer_addr, cfg_clone, herdr_clone).await {
                    warn!("direct P2P session from {peer_addr} ended: {e}");
                }
            });
        }
    });

    Ok(local_addr)
}

async fn handle_direct_p2p_socket(
    stream: TcpStream,
    peer_addr: SocketAddr,
    cfg: DaemonConfig,
    herdr: Option<Arc<HerdrClient>>,
) -> Result<()> {
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();

    let hello = read_raw_frame(&mut reader).await?;
    let (client_device_id, client_x25519_pub_b64) = match hello {
        RelayWireFrame::DirectP2pHello {
            client_device_id,
            target_device_id,
            client_ed25519_pub_b64,
            client_x25519_pub_b64,
            timestamp_ms,
            signature_b64,
        } => {
            if target_device_id != cfg.device_id {
                anyhow::bail!("P2P target device mismatch");
            }
            let signed_msg = format!("p2p:{client_device_id}:{target_device_id}:{timestamp_ms}");
            StoredDeviceIdentity::verify_signature(
                &client_ed25519_pub_b64,
                signed_msg.as_bytes(),
                &signature_b64,
            )?;
            (client_device_id, client_x25519_pub_b64)
        }
        other => anyhow::bail!("expected DirectP2pHello, got {other:?}"),
    };

    let path_kind = if peer_addr.is_ipv6() {
        "cn_public_ipv6"
    } else {
        "lan_ipv4"
    };
    let ack = RelayWireFrame::DirectP2pAck {
        ok: true,
        daemon_device_id: cfg.device_id,
        daemon_x25519_pub_b64: cfg.identity.x25519_public_b64.clone(),
        path_kind: path_kind.into(),
    };
    writer.write_all(&ack.encode_length_delimited()?).await?;

    let (out_tx, mut out_rx) = mpsc::channel::<RelayWireFrame>(256);
    tokio::spawn(async move {
        while let Some(f) = out_rx.recv().await {
            if let Ok(bytes) = f.encode_length_delimited() {
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        }
    });

    let my_dh_secret = cfg.identity.x25519_secret()?;
    let sess = E2eeSession::derive(&my_dh_secret, &client_x25519_pub_b64, false)?;
    let sessions: Arc<Mutex<HashMap<Uuid, E2eeSession>>> = Arc::new(Mutex::new(HashMap::new()));
    sessions.lock().await.insert(client_device_id, sess);

    let active_ptys: Arc<Mutex<HashMap<(Uuid, String), PtyBridgeHandle>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let my_dh_pub_b64 = cfg.identity.x25519_public_b64.clone();
    let my_device_id = cfg.device_id;
    let hostname = cfg.identity.name.clone();

    while let Ok(frame) = read_raw_frame(&mut reader).await {
        if let RelayWireFrame::RoutePacket {
            src_device_id,
            e2ee,
            ..
        } = frame
        {
            let req_msg = {
                let mut map = sessions.lock().await;
                let Some(s) = map.get_mut(&src_device_id) else {
                    continue;
                };
                match s.open_message(&e2ee) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("P2P E2EE open error: {e}");
                        continue;
                    }
                }
            };

            if let Some((reply_msg, reply_lane)) = handle_peer_request_full(
                src_device_id,
                req_msg,
                herdr.as_deref(),
                &hostname,
                &active_ptys,
                &sessions,
                &out_tx,
                my_device_id,
                &my_dh_pub_b64,
            )
            .await
            {
                let mut map = sessions.lock().await;
                if let Some(s) = map.get_mut(&src_device_id) {
                    if let Ok(sealed) = s.seal_message(&reply_msg) {
                        let _ = out_tx
                            .send(RelayWireFrame::RoutePacket {
                                src_device_id: my_device_id,
                                dst_device_id: src_device_id,
                                lane: reply_lane,
                                e2ee: sealed,
                                sender_x25519_pub_b64: Some(my_dh_pub_b64.clone()),
                            })
                            .await;
                    }
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_peer_request_full(
    src_device_id: Uuid,
    msg: PeerMessage,
    herdr: Option<&HerdrClient>,
    hostname: &str,
    active_ptys: &Arc<Mutex<HashMap<(Uuid, String), PtyBridgeHandle>>>,
    sessions: &Arc<Mutex<HashMap<Uuid, E2eeSession>>>,
    out_tx: &mpsc::Sender<RelayWireFrame>,
    my_device_id: Uuid,
    my_dh_pub_b64: &str,
) -> Option<(PeerMessage, TrafficLane)> {
    match msg {
        PeerMessage::ListWorkspaces { req_id, root_dirs } => {
            let terminals = match herdr {
                Some(hc) => hc.list_terminals(false).await.unwrap_or_default(),
                None => vec![],
            };
            let repos = scan_git_workspaces(hostname, &root_dirs, &terminals).await;
            let voice_config = match herdr {
                Some(hc) => hc.get_voice_config().await.ok().flatten(),
                None => None,
            };
            return Some((
                PeerMessage::WorkspacesSnapshot {
                    req_id,
                    machine_name: hostname.to_string(),
                    repos,
                    voice_config,
                },
                TrafficLane::ControlP0,
            ));
        }
        PeerMessage::PtyAttach {
            req_id,
            terminal,
            cols,
            rows,
        } => {
            let Some(hc) = herdr else {
                return Some((
                    PeerMessage::PtyAttached {
                        req_id,
                        terminal,
                        ok: false,
                        error: Some("herdr-core (`hc`) unavailable on host".into()),
                    },
                    TrafficLane::ControlP0,
                ));
            };

            let (pty_tx, mut pty_rx) = mpsc::channel::<PtyOutputEvent>(256);
            match spawn_pty_attach(hc.hc_path(), &terminal, cols, rows, pty_tx) {
                Ok(handle) => {
                    active_ptys
                        .lock()
                        .await
                        .insert((src_device_id, terminal.clone()), handle);

                    let sessions_clone = Arc::clone(sessions);
                    let out_tx_clone = out_tx.clone();
                    let dh_pub = my_dh_pub_b64.to_string();
                    tokio::spawn(async move {
                        while let Some(ev) = pty_rx.recv().await {
                            let chunk_msg = PeerMessage::PtyOutputChunk {
                                terminal: ev.terminal,
                                seq: ev.seq,
                                data_b64: ev.data_b64,
                            };
                            let sealed_opt = {
                                let mut map = sessions_clone.lock().await;
                                map.get_mut(&src_device_id)
                                    .and_then(|s| s.seal_message(&chunk_msg).ok())
                            };
                            if let Some(sealed) = sealed_opt {
                                if out_tx_clone
                                    .send(RelayWireFrame::RoutePacket {
                                        src_device_id: my_device_id,
                                        dst_device_id: src_device_id,
                                        lane: TrafficLane::ControlP0,
                                        e2ee: sealed,
                                        sender_x25519_pub_b64: Some(dh_pub.clone()),
                                    })
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    });

                    return Some((
                        PeerMessage::PtyAttached {
                            req_id,
                            terminal,
                            ok: true,
                            error: None,
                        },
                        TrafficLane::ControlP0,
                    ));
                }
                Err(e) => {
                    return Some((
                        PeerMessage::PtyAttached {
                            req_id,
                            terminal,
                            ok: false,
                            error: Some(e.to_string()),
                        },
                        TrafficLane::ControlP0,
                    ));
                }
            }
        }
        PeerMessage::PtyInput { terminal, data_b64 } => {
            if let Ok(bytes) = B64.decode(&data_b64) {
                let map = active_ptys.lock().await;
                if let Some(pty) = map.get(&(src_device_id, terminal.clone())) {
                    let _ = pty.write_raw(&bytes);
                    return None;
                }
            }
            // Fallback: if no active PTY attach yet, forward via herdr send_text
            if let (Some(hc), Ok(bytes)) = (herdr, B64.decode(&data_b64)) {
                let txt = String::from_utf8_lossy(&bytes).to_string();
                let _ = hc.send_text(&terminal, &txt, true).await;
            }
            return None;
        }
        PeerMessage::PtyResize {
            terminal,
            cols,
            rows,
        } => {
            let map = active_ptys.lock().await;
            if let Some(pty) = map.get(&(src_device_id, terminal)) {
                let _ = pty.resize(cols, rows);
            }
            return None;
        }
        PeerMessage::PtyDetach { req_id, terminal } => {
            active_ptys.lock().await.remove(&(src_device_id, terminal));
            return Some((
                PeerMessage::ActionAck {
                    req_id,
                    ok: true,
                    detail: "pty_detached".into(),
                },
                TrafficLane::ControlP0,
            ));
        }
        PeerMessage::UploadTerminalFile {
            req_id,
            terminal,
            filename,
            data_b64,
        } => {
            let Some(hc) = herdr else {
                return Some((
                    PeerMessage::UploadTerminalFileResult {
                        req_id,
                        terminal,
                        ok: false,
                        remote_path: None,
                        bytes: 0,
                        error: Some("herdr-core unavailable".into()),
                    },
                    TrafficLane::ControlP0,
                ));
            };
            match hc.put_file(&terminal, &filename, &data_b64).await {
                Ok((remote_path, bytes)) => {
                    return Some((
                        PeerMessage::UploadTerminalFileResult {
                            req_id,
                            terminal,
                            ok: true,
                            remote_path: Some(remote_path),
                            bytes,
                            error: None,
                        },
                        TrafficLane::ControlP0,
                    ));
                }
                Err(e) => {
                    return Some((
                        PeerMessage::UploadTerminalFileResult {
                            req_id,
                            terminal,
                            ok: false,
                            remote_path: None,
                            bytes: 0,
                            error: Some(e.to_string()),
                        },
                        TrafficLane::ControlP0,
                    ));
                }
            }
        }
        PeerMessage::SetVoiceConfig { req_id, config } => {
            let ok = match herdr {
                Some(hc) => hc.set_voice_config(&config).await.is_ok(),
                None => false,
            };
            return Some((
                PeerMessage::VoiceConfigAck {
                    req_id,
                    ok,
                    config,
                },
                TrafficLane::ControlP0,
            ));
        }
        other => Some(handle_peer_request(other, herdr, hostname).await),
    }
}

async fn handle_peer_request(
    msg: PeerMessage,
    herdr: Option<&HerdrClient>,
    hostname: &str,
) -> (PeerMessage, TrafficLane) {
    match msg {
        PeerMessage::Ping { req_id, sent_ms } => (
            PeerMessage::Pong {
                req_id,
                sent_ms,
                daemon_hostname: hostname.to_string(),
                herdr_version: herdr
                    .map(|h| h.info().version.clone())
                    .unwrap_or_else(|| "unavailable".into()),
            },
            TrafficLane::ControlP0,
        ),
        PeerMessage::ListTerminals {
            req_id,
            include_finished,
        } => {
            if let Some(hc) = herdr {
                match hc.list_terminals(include_finished).await {
                    Ok(terminals) => (
                        PeerMessage::TerminalsSnapshot {
                            req_id,
                            core_name: hc.info().core_name.clone(),
                            core_id: hc.info().core_id.clone(),
                            terminals,
                        },
                        TrafficLane::ControlP0,
                    ),
                    Err(e) => (
                        PeerMessage::ActionAck {
                            req_id,
                            ok: false,
                            detail: e.to_string(),
                        },
                        TrafficLane::ControlP0,
                    ),
                }
            } else {
                (
                    PeerMessage::TerminalsSnapshot {
                        req_id,
                        core_name: hostname.to_string(),
                        core_id: "offline".into(),
                        terminals: vec![],
                    },
                    TrafficLane::ControlP0,
                )
            }
        }
        PeerMessage::LaunchAgent {
            req_id,
            agent,
            name,
            cwd,
            initial_prompt,
            cols,
            rows,
        } => {
            let Some(hc) = herdr else {
                return (
                    PeerMessage::LaunchAgentResult {
                        req_id,
                        ok: false,
                        terminal: None,
                        error: Some("herdr-core (`hc`) is not installed on host".into()),
                    },
                    TrafficLane::ControlP0,
                );
            };
            match hc
                .launch_agent(
                    &agent,
                    name.as_deref(),
                    cwd.as_deref(),
                    initial_prompt.as_deref(),
                    cols,
                    rows,
                )
                .await
            {
                Ok(term) => (
                    PeerMessage::LaunchAgentResult {
                        req_id,
                        ok: true,
                        terminal: Some(term),
                        error: None,
                    },
                    TrafficLane::ControlP0,
                ),
                Err(e) => (
                    PeerMessage::LaunchAgentResult {
                        req_id,
                        ok: false,
                        terminal: None,
                        error: Some(e.to_string()),
                    },
                    TrafficLane::ControlP0,
                ),
            }
        }
        PeerMessage::SendTerminalText {
            req_id,
            terminal,
            text,
        } => {
            let res = match herdr {
                Some(hc) => hc.send_text(&terminal, &text, true).await,
                None => Err(anyhow!("herdr unavailable")),
            };
            (
                PeerMessage::ActionAck {
                    req_id,
                    ok: res.is_ok(),
                    detail: res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "sent".into()),
                },
                TrafficLane::ControlP0,
            )
        }
        PeerMessage::SendTerminalKeys {
            req_id,
            terminal,
            keys,
        } => {
            let res = match herdr {
                Some(hc) => hc.send_keys(&terminal, &keys).await,
                None => Err(anyhow!("herdr unavailable")),
            };
            (
                PeerMessage::ActionAck {
                    req_id,
                    ok: res.is_ok(),
                    detail: res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "keys_delivered".into()),
                },
                TrafficLane::ControlP0,
            )
        }
        PeerMessage::InterruptTerminal { req_id, terminal } => {
            let res = match herdr {
                Some(hc) => hc.interrupt(&terminal).await,
                None => Err(anyhow!("herdr unavailable")),
            };
            (
                PeerMessage::ActionAck {
                    req_id,
                    ok: res.is_ok(),
                    detail: res
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "interrupted".into()),
                },
                TrafficLane::ControlP0,
            )
        }
        PeerMessage::ReadTerminalScreen {
            req_id,
            terminal,
            history_lines,
        } => {
            if let Some(hc) = herdr {
                if let Ok((text, status)) = hc.read_screen(&terminal, history_lines).await {
                    return (
                        PeerMessage::TerminalScreenSnapshot {
                            req_id,
                            terminal,
                            text,
                            status,
                        },
                        TrafficLane::BulkP1,
                    );
                }
            }
            (
                PeerMessage::TerminalScreenSnapshot {
                    req_id,
                    terminal,
                    text: String::new(),
                    status: None,
                },
                TrafficLane::BulkP1,
            )
        }
        PeerMessage::GitDiffRequest { req_id, cwd } => {
            let branch = run_cmd_capture("git", &["-C", &cwd, "rev-parse", "--abbrev-ref", "HEAD"])
                .await
                .unwrap_or_else(|| "main".into());
            let status_short = run_cmd_capture("git", &["-C", &cwd, "status", "--short"])
                .await
                .unwrap_or_default();
            let diff_unified = run_cmd_capture("git", &["-C", &cwd, "diff", "--no-color"])
                .await
                .unwrap_or_default();
            (
                PeerMessage::GitDiffResponse {
                    req_id,
                    branch: branch.trim().to_string(),
                    status_short,
                    diff_unified,
                },
                TrafficLane::BulkP1,
            )
        }
        PeerMessage::HttpLocalPreviewRequest { req_id, port, path } => {
            let clean_path = if path.starts_with('/') {
                path
            } else {
                format!("/{path}")
            };
            let url = format!("http://127.0.0.1:{port}{clean_path}");
            match reqwest::get(&url).await {
                Ok(resp) => {
                    let status_code = resp.status().as_u16();
                    let content_type = resp
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("text/html; charset=utf-8")
                        .to_string();
                    let bytes = resp.bytes().await.unwrap_or_default();
                    (
                        PeerMessage::HttpLocalPreviewResponse {
                            req_id,
                            status_code,
                            content_type,
                            body_b64: B64.encode(&bytes),
                        },
                        TrafficLane::BulkP1,
                    )
                }
                Err(e) => (
                    PeerMessage::HttpLocalPreviewResponse {
                        req_id,
                        status_code: 502,
                        content_type: "text/plain".into(),
                        body_b64: B64.encode(format!("Local port {port} unreachable: {e}")),
                    },
                    TrafficLane::BulkP1,
                ),
            }
        }
        other => (
            PeerMessage::ActionAck {
                req_id: "unhandled".into(),
                ok: false,
                detail: format!("unhandled peer message variant: {other:?}"),
            },
            TrafficLane::ControlP0,
        ),
    }
}

async fn run_cmd_capture(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().await.ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn read_raw_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<RelayWireFrame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_WIRE_FRAME_BYTES {
        anyhow::bail!("invalid frame length {len}");
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

fn detect_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "coda-host".into())
}

// ============================================================================
// Phase 3 Direct P2P Client Dialer (Happy Eyeballs 250ms Probe + Zero-Relay E2EE)
// ============================================================================

pub struct DirectP2pClient {
    pub client_device_id: Uuid,
    pub target_device_id: Uuid,
    pub path_kind: String,
    writer: tokio::net::tcp::OwnedWriteHalf,
    reader: tokio::net::tcp::OwnedReadHalf,
    e2ee: E2eeSession,
    client_x25519_pub_b64: String,
}

impl DirectP2pClient {
    /// Races a 250ms Happy Eyeballs probe against a direct P2P endpoint (`lan_ipv4` or `cn_public_ipv6`).
    pub async fn connect_direct(
        endpoint: &str,
        client_id: &StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
    ) -> Result<Self> {
        let stream = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            TcpStream::connect(endpoint),
        )
        .await
        .context("direct P2P Happy Eyeballs probe timed out (250ms)")??;
        let _ = stream.set_nodelay(true);
        let (mut reader, mut writer) = stream.into_split();

        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let signed_msg = format!("p2p:{client_device_id}:{target_device_id}:{timestamp_ms}");
        let signature_b64 = client_id.sign(signed_msg.as_bytes())?;

        let hello = RelayWireFrame::DirectP2pHello {
            client_device_id,
            target_device_id,
            client_ed25519_pub_b64: client_id.ed25519_public_b64.clone(),
            client_x25519_pub_b64: client_id.x25519_public_b64.clone(),
            timestamp_ms,
            signature_b64,
        };
        writer.write_all(&hello.encode_length_delimited()?).await?;

        let ack = read_raw_frame(&mut reader).await?;
        let (daemon_pub_b64, path_kind) = match ack {
            RelayWireFrame::DirectP2pAck {
                ok: true,
                daemon_device_id,
                daemon_x25519_pub_b64,
                path_kind,
            } if daemon_device_id == target_device_id => (daemon_x25519_pub_b64, path_kind),
            other => anyhow::bail!("direct P2P handshake rejected: {other:?}"),
        };

        let e2ee = E2eeSession::derive(&client_id.x25519_secret()?, &daemon_pub_b64, true)?;

        Ok(Self {
            client_device_id,
            target_device_id,
            path_kind,
            writer,
            reader,
            e2ee,
            client_x25519_pub_b64: client_id.x25519_public_b64.clone(),
        })
    }

    pub async fn send_message(&mut self, msg: &PeerMessage, lane: TrafficLane) -> Result<()> {
        let sealed = self.e2ee.seal_message(msg)?;
        let pkt = RelayWireFrame::RoutePacket {
            src_device_id: self.client_device_id,
            dst_device_id: self.target_device_id,
            lane,
            e2ee: sealed,
            sender_x25519_pub_b64: Some(self.client_x25519_pub_b64.clone()),
        };
        self.writer
            .write_all(&pkt.encode_length_delimited()?)
            .await?;
        Ok(())
    }

    pub async fn recv_message(&mut self) -> Result<PeerMessage> {
        loop {
            let frame = read_raw_frame(&mut self.reader).await?;
            if let RelayWireFrame::RoutePacket { e2ee, .. } = frame {
                return self.e2ee.open_message(&e2ee);
            }
        }
    }
}
