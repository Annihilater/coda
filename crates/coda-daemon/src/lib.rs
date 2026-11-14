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
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_herdr::{
    ensure_or_install_hc_binary, scan_git_workspaces, spawn_pty_attach, HerdrClient,
    PtyBridgeHandle, PtyOutputEvent,
};
use coda_protocol::{
    AccountLiveEvent, E2eeSession, HerdrTerminalSummary, NetworkEndpointCandidate, PeerMessage,
    RelayWireFrame, RemoteFileItem, StoredDeviceIdentity, TrafficLane, MAX_WIRE_FRAME_BYTES,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream, UdpSocket as TokioUdpSocket,
    },
    process::Command,
    sync::{broadcast, mpsc, oneshot, Mutex},
};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message as WsMsg};
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
    #[serde(default)]
    pub owner_email: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
    pub device_id: Uuid,
    pub daemon_relay_ticket: String,
    pub relays: Vec<DaemonRelayEntry>,
    pub identity: StoredDeviceIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteHostSummary {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub platform: String,
    pub x25519_pubkey: String,
    #[serde(default)]
    pub endpoints: Vec<NetworkEndpointCandidate>,
    pub is_online: bool,
}

#[derive(Debug, Clone)]
pub struct DeviceAuthStartResult {
    pub server_url: String,
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in_secs: i64,
    pub interval_secs: u64,
    pub identity: StoredDeviceIdentity,
}

impl DaemonConfig {
    pub fn default_path() -> PathBuf {
        if let Ok(custom) = std::env::var("CODA_DAEMON_CONFIG") {
            let trimmed = custom.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
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
                    let ip4 = local.ip();
                    out.push(NetworkEndpointCandidate {
                        kind: "lan_ipv4".into(),
                        addr: format!("{ip4}:{p2p_port}"),
                    });
                    out.push(NetworkEndpointCandidate {
                        kind: "lan_ws".into(),
                        addr: format!("ws://{ip4}:{p2p_port}"),
                    });
                }
            }
        }
    }
    if let Ok(sock6) = UdpSocket::bind("[::]:0") {
        if sock6.connect("[2400:3200::1]:53").is_ok() {
            if let Ok(local) = sock6.local_addr() {
                if !local.ip().is_loopback() {
                    let ip6 = local.ip();
                    out.push(NetworkEndpointCandidate {
                        kind: "cn_public_ipv6".into(),
                        addr: format!("[{ip6}]:{p2p_port}"),
                    });
                    out.push(NetworkEndpointCandidate {
                        kind: "ipv6_ws".into(),
                        addr: format!("ws://[{ip6}]:{p2p_port}"),
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
        owner_email: None,
        session_token: None,
        device_id,
        daemon_relay_ticket,
        relays,
        identity,
    })
}

fn detect_platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

fn is_loopback_endpoint(endpoint: &str) -> bool {
    let lower = endpoint.trim().to_ascii_lowercase();
    lower.starts_with("127.0.0.1:")
        || lower.starts_with("localhost:")
        || lower.starts_with("0.0.0.0:")
        || lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://localhost")
}

fn parse_relays_value(val: &serde_json::Value) -> Vec<DaemonRelayEntry> {
    val.as_array()
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
        .collect()
}

/// Starts an RFC 8628-style browser device authorization request.
/// Works without reading stdin so `curl ... | sh` pipes work seamlessly.
pub async fn start_device_login_flow(
    server_url: &str,
    role: &str,
    custom_name: Option<String>,
    existing_identity: Option<StoredDeviceIdentity>,
) -> Result<DeviceAuthStartResult> {
    let clean_server = server_url.trim().trim_end_matches('/').to_string();
    let hostname = custom_name.unwrap_or_else(detect_hostname);
    let mut identity = existing_identity.unwrap_or_else(|| StoredDeviceIdentity::generate(&hostname));
    identity.name = hostname.clone();

    let (herdr_id, herdr_ver) = match HerdrClient::connect_default().await {
        Ok(hc) => (
            Some(hc.info().core_id.clone()),
            Some(hc.info().version.clone()),
        ),
        Err(_) => (None, None),
    };
    let endpoints = discover_local_endpoints();

    let http = reqwest::Client::new();
    let url = format!("{clean_server}/api/v1/auth/device/start");
    let resp = http
        .post(&url)
        .json(&json!({
            "device_name": hostname,
            "role": role,
            "platform": detect_platform_name(),
            "ed25519_pubkey": identity.ed25519_public_b64,
            "x25519_pubkey": identity.x25519_public_b64,
            "endpoints": endpoints,
            "herdr_core_id": herdr_id,
            "herdr_version": herdr_ver,
        }))
        .send()
        .await?
        .error_for_status()
        .context("failed starting browser device authorization")?
        .json::<serde_json::Value>()
        .await?;

    Ok(DeviceAuthStartResult {
        server_url: clean_server,
        device_code: resp["device_code"]
            .as_str()
            .ok_or_else(|| anyhow!("missing device_code"))?
            .to_string(),
        user_code: resp["user_code"]
            .as_str()
            .ok_or_else(|| anyhow!("missing user_code"))?
            .to_string(),
        verification_uri: resp["verification_uri"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        verification_uri_complete: resp["verification_uri_complete"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        expires_in_secs: resp["expires_in_secs"].as_i64().unwrap_or(600),
        interval_secs: resp["interval_secs"].as_u64().unwrap_or(2).max(1),
        identity,
    })
}

/// Polls `/api/v1/auth/device/poll` once; returns `Ok(Some(cfg))` when approved, `Ok(None)` while pending.
pub async fn poll_device_login_once(started: &DeviceAuthStartResult) -> Result<Option<DaemonConfig>> {
    let http = reqwest::Client::new();
    let url = format!("{}/api/v1/auth/device/poll", started.server_url);
    let resp = http
        .post(&url)
        .json(&json!({ "device_code": started.device_code }))
        .send()
        .await?
        .error_for_status()
        .context("device authorization poll failed")?
        .json::<serde_json::Value>()
        .await?;

    match resp["state"].as_str().unwrap_or("expired") {
        "pending" => Ok(None),
        "approved" => {
            let device_id: Uuid = serde_json::from_value(resp["device"]["id"].clone())?;
            let mut identity = started.identity.clone();
            identity.device_id = Some(device_id);
            if let Some(name) = resp["device"]["name"].as_str() {
                identity.name = name.to_string();
            }
            let owner_handle = resp["owner_handle"]
                .as_str()
                .or_else(|| resp["user"]["handle"].as_str())
                .unwrap_or("owner")
                .to_string();
            let owner_email = resp["user"]["email"].as_str().map(|s| s.to_string());
            let session_token = resp["session_token"].as_str().map(|s| s.to_string());
            let daemon_relay_ticket = resp["daemon_relay_ticket"]
                .as_str()
                .ok_or_else(|| anyhow!("missing daemon_relay_ticket"))?
                .to_string();
            let relays = parse_relays_value(&resp["relays"]);
            Ok(Some(DaemonConfig {
                server_url: started.server_url.clone(),
                owner_handle,
                owner_email,
                session_token,
                device_id,
                daemon_relay_ticket,
                relays,
                identity,
            }))
        }
        other => anyhow::bail!("device authorization {other}; run login again"),
    }
}

/// Polls `/api/v1/auth/device/poll` until the user clicks "Authorize" in the browser or the code expires.
pub async fn poll_device_login_until_approved(
    started: &DeviceAuthStartResult,
) -> Result<DaemonConfig> {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(started.expires_in_secs.max(30) as u64);
    let step = std::time::Duration::from_secs(started.interval_secs.max(1));
    loop {
        if let Some(cfg) = poll_device_login_once(started).await? {
            return Ok(cfg);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("device authorization timed out waiting for browser confirmation");
        }
        tokio::time::sleep(step).await;
    }
}

/// Logs in directly using email/handle + password and enrolls this device as a `daemon` or `hybrid` host.
pub async fn login_with_password_and_enroll(
    server_url: &str,
    email_or_handle: &str,
    password: &str,
    role: &str,
    custom_name: Option<String>,
) -> Result<DaemonConfig> {
    let clean_server = server_url.trim().trim_end_matches('/').to_string();
    let http = reqwest::Client::new();
    let login_resp = http
        .post(format!("{clean_server}/api/v1/auth/login"))
        .json(&json!({
            "email_or_handle": email_or_handle,
            "password": password,
            "client_label": format!("{}-{}", detect_platform_name(), role),
        }))
        .send()
        .await?
        .error_for_status()
        .context("login rejected by server")?
        .json::<serde_json::Value>()
        .await?;

    let token = login_resp["token"]
        .as_str()
        .ok_or_else(|| anyhow!("missing session token"))?
        .to_string();
    let owner_email = login_resp["user"]["email"].as_str().map(|s| s.to_string());

    let hostname = custom_name.unwrap_or_else(detect_hostname);
    let mut identity = StoredDeviceIdentity::generate(&hostname);
    let (herdr_id, herdr_ver) = match HerdrClient::connect_default().await {
        Ok(hc) => (
            Some(hc.info().core_id.clone()),
            Some(hc.info().version.clone()),
        ),
        Err(_) => (None, None),
    };
    let endpoints = discover_local_endpoints();

    let enroll_resp = http
        .post(format!("{clean_server}/api/v1/devices/enroll-host"))
        .bearer_auth(&token)
        .json(&json!({
            "name": hostname,
            "role": role,
            "platform": detect_platform_name(),
            "ed25519_pubkey": identity.ed25519_public_b64,
            "x25519_pubkey": identity.x25519_public_b64,
            "endpoints": endpoints,
            "herdr_core_id": herdr_id,
            "herdr_version": herdr_ver,
        }))
        .send()
        .await?
        .error_for_status()
        .context("host enrollment rejected by server")?
        .json::<serde_json::Value>()
        .await?;

    let device_id: Uuid = serde_json::from_value(enroll_resp["device"]["id"].clone())?;
    identity.device_id = Some(device_id);
    let owner_handle = enroll_resp["owner_handle"]
        .as_str()
        .unwrap_or("owner")
        .to_string();
    let daemon_relay_ticket = enroll_resp["daemon_relay_ticket"]
        .as_str()
        .ok_or_else(|| anyhow!("missing daemon_relay_ticket"))?
        .to_string();
    let relays = parse_relays_value(&enroll_resp["relays"]);

    Ok(DaemonConfig {
        server_url: clean_server,
        owner_handle,
        owner_email,
        session_token: Some(token),
        device_id,
        daemon_relay_ticket,
        relays,
        identity,
    })
}

/// Registers an ephemeral or dedicated client device identity with `/api/v1/devices/register-client`
/// so each remote relay tunnel uses its own `device_id` without colliding with an embedded host.
pub async fn register_client_identity(
    server_url: &str,
    session_token: &str,
    name: &str,
    platform: &str,
) -> Result<(Uuid, StoredDeviceIdentity)> {
    let clean_server = server_url.trim().trim_end_matches('/');
    let mut identity = StoredDeviceIdentity::generate(name);
    let resolved_platform = if platform.trim().is_empty() {
        detect_platform_name()
    } else {
        platform.trim()
    };
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{clean_server}/api/v1/devices/register-client"))
        .bearer_auth(session_token)
        .json(&json!({
            "name": identity.name,
            "platform": resolved_platform,
            "ed25519_pubkey": identity.ed25519_public_b64,
            "x25519_pubkey": identity.x25519_public_b64,
            "endpoints": [],
        }))
        .send()
        .await?
        .error_for_status()
        .context("failed registering client device identity")?
        .json::<serde_json::Value>()
        .await?;

    let dev_id: Uuid = serde_json::from_value(resp["id"].clone())
        .context("missing id in register-client response")?;
    identity.device_id = Some(dev_id);
    Ok((dev_id, identity))
}

/// Fetches all remote controllable hosts (`daemon` and `hybrid`) under this account.
pub async fn list_account_hosts(
    server_url: &str,
    session_token: &str,
) -> Result<Vec<RemoteHostSummary>> {
    let clean_server = server_url.trim().trim_end_matches('/');
    let http = reqwest::Client::new();
    let raw = http
        .get(format!("{clean_server}/api/v1/devices"))
        .bearer_auth(session_token)
        .send()
        .await?
        .error_for_status()
        .context("failed listing account devices")?
        .json::<Vec<serde_json::Value>>()
        .await?;

    let mut out = Vec::new();
    for item in raw {
        let role = item["role"].as_str().unwrap_or("");
        if role != "daemon" && role != "hybrid" {
            continue;
        }
        let Ok(id) = serde_json::from_value::<Uuid>(item["id"].clone()) else {
            continue;
        };
        let endpoints: Vec<NetworkEndpointCandidate> =
            serde_json::from_value(item["endpoints_json"].clone()).unwrap_or_default();
        out.push(RemoteHostSummary {
            id,
            name: item["name"].as_str().unwrap_or("coda-host").to_string(),
            role: role.to_string(),
            platform: item["platform"].as_str().unwrap_or("linux").to_string(),
            x25519_pubkey: item["x25519_pubkey"].as_str().unwrap_or("").to_string(),
            endpoints,
            is_online: item["is_online"].as_bool().unwrap_or(false),
        });
    }
    Ok(out)
}

/// Connects to `coda-server` `/api/v1/events/ws` for real-time same-account device & presence pushes.
pub async fn subscribe_account_events_ws(
    server_url: &str,
    session_token: &str,
) -> Result<mpsc::Receiver<AccountLiveEvent>> {
    use tokio_tungstenite::tungstenite::{
        client::IntoClientRequest, http::HeaderValue,
    };

    let clean = server_url.trim().trim_end_matches('/');
    let ws_base = if let Some(rest) = clean.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = clean.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if clean.starts_with("wss://") || clean.starts_with("ws://") {
        clean.to_string()
    } else {
        format!("ws://{clean}")
    };
    let encoded_tok: String = session_token
        .trim()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!("{ws_base}/api/v1/events/ws?token={encoded_tok}");
    let mut req = url
        .into_client_request()
        .context("invalid /api/v1/events/ws request URL")?;
    if let Ok(hdr) = HeaderValue::from_str(&format!("Bearer {}", session_token.trim())) {
        req.headers_mut().insert("Authorization", hdr);
    }

    let (ws_stream, _) = connect_async(req)
        .await
        .context("failed connecting to /api/v1/events/ws")?;
    let (mut sink, mut stream) = ws_stream.split();
    let (tx, rx) = mpsc::channel::<AccountLiveEvent>(128);

    tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                WsMsg::Text(txt) => {
                    if let Ok(ev) = serde_json::from_str::<AccountLiveEvent>(&txt) {
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                }
                WsMsg::Binary(bin) => {
                    if let Ok(ev) = serde_json::from_slice::<AccountLiveEvent>(&bin) {
                        if tx.send(ev).await.is_err() {
                            break;
                        }
                    }
                }
                WsMsg::Ping(payload) => {
                    if sink.send(WsMsg::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                WsMsg::Close(_) => break,
                _ => {}
            }
        }
    });

    Ok(rx)
}

/// Mints a short-lived relay ticket to connect this device to a remote peer host under the same user.
pub async fn mint_peer_relay_config(
    server_url: &str,
    session_token: &str,
    src_device_id: Uuid,
    target_device_id: Uuid,
    peer_x25519_pub_b64: &str,
) -> Result<SmartRelayFallbackConfig> {
    let clean_server = server_url.trim().trim_end_matches('/');
    let remote_server = clean_server.starts_with("https://") || !is_loopback_endpoint(clean_server.trim_start_matches("http://"));
    let http = reqwest::Client::new();
    let resp = http
        .post(format!("{clean_server}/api/v1/relay/ticket"))
        .bearer_auth(session_token)
        .json(&json!({
            "device_id": src_device_id,
            "peer_device_id": target_device_id,
        }))
        .send()
        .await?
        .error_for_status()
        .context("failed minting peer relay ticket")?
        .json::<serde_json::Value>()
        .await?;

    let relay_ticket = resp["ticket"]
        .as_str()
        .ok_or_else(|| anyhow!("missing relay ticket"))?
        .to_string();
    let relays = parse_relays_value(&resp["relays"]);
    let raw_tcp_addr = relays
        .iter()
        .find(|r| r.mode == "raw_tcp_no_sni" && !(remote_server && is_loopback_endpoint(&r.endpoint)))
        .map(|r| r.endpoint.clone());
    let wss_url = relays
        .iter()
        .find(|r| r.mode == "cdn_wss")
        .map(|r| r.endpoint.clone());

    Ok(SmartRelayFallbackConfig {
        raw_tcp_addr,
        wss_url,
        relay_ticket,
        peer_x25519_pub_b64: peer_x25519_pub_b64.to_string(),
    })
}

async fn connect_herdr_with_install() -> Option<Arc<HerdrClient>> {
    match HerdrClient::connect_default().await {
        Ok(hc) => Some(Arc::new(hc)),
        Err(first_err) => {
            warn!("herdr-core missing or inactive ({first_err}); attempting auto-install");
            match ensure_or_install_hc_binary().await {
                Ok(bin) => match HerdrClient::connect_with_bin(&bin).await {
                    Ok(hc) => {
                        info!("Bridged freshly installed herdr-core `{}`", hc.info().core_name);
                        Some(Arc::new(hc))
                    }
                    Err(e) => {
                        warn!("herdr-core failed to connect after install: {e}");
                        None
                    }
                },
                Err(e) => {
                    warn!("herdr-core auto-install skipped/failed: {e}");
                    None
                }
            }
        }
    }
}

/// Seals and enqueues an outbound peer message while preserving exact monotonic E2EE sequence order.
async fn send_e2ee_peer_message(
    sessions: &Arc<Mutex<HashMap<Uuid, E2eeSession>>>,
    out_tx: &mpsc::Sender<RelayWireFrame>,
    my_device_id: Uuid,
    dst_device_id: Uuid,
    my_dh_pub_b64: &str,
    msg: &PeerMessage,
    lane: TrafficLane,
) -> bool {
    let Ok(permit) = out_tx.reserve().await else {
        return false;
    };
    let mut map = sessions.lock().await;
    let Some(sess) = map.get_mut(&dst_device_id) else {
        return false;
    };
    let Ok(sealed) = sess.seal_message(msg) else {
        return false;
    };
    permit.send(RelayWireFrame::RoutePacket {
        src_device_id: my_device_id,
        dst_device_id,
        lane,
        e2ee: sealed,
        sender_x25519_pub_b64: Some(my_dh_pub_b64.to_string()),
    });
    true
}

fn spawn_herdr_event_forwarder(
    herdr: Arc<HerdrClient>,
    sessions: Arc<Mutex<HashMap<Uuid, E2eeSession>>>,
    out_tx: mpsc::Sender<RelayWireFrame>,
    my_device_id: Uuid,
    my_dh_pub_b64: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Ok(mut ev_rx) = herdr.subscribe_live_events().await else {
            return;
        };
        let mut seq: u64 = 0;
        while let Some(raw_event) = ev_rx.recv().await {
            if out_tx.is_closed() {
                break;
            }
            seq += 1;
            let event_kind = raw_event
                .get("kind")
                .or_else(|| raw_event.get("type"))
                .or_else(|| raw_event.get("event"))
                .and_then(|v| v.as_str())
                .unwrap_or("update")
                .to_string();
            let terminal = raw_event
                .get("terminal")
                .and_then(|v| serde_json::from_value::<HerdrTerminalSummary>(v.clone()).ok())
                .or_else(|| serde_json::from_value::<HerdrTerminalSummary>(raw_event.clone()).ok());
            let peers: Vec<Uuid> = {
                let map = sessions.lock().await;
                map.keys().copied().collect()
            };
            if peers.is_empty() {
                continue;
            }
            let msg = PeerMessage::HerdrLiveEvent {
                seq,
                event_kind,
                terminal,
                raw_event,
            };
            for peer_id in peers {
                let _ = send_e2ee_peer_message(
                    &sessions,
                    &out_tx,
                    my_device_id,
                    peer_id,
                    &my_dh_pub_b64,
                    &msg,
                    TrafficLane::ControlP0,
                )
                .await;
            }
        }
    })
}

struct TaskAbortGuard(Vec<tokio::task::AbortHandle>);

impl Drop for TaskAbortGuard {
    fn drop(&mut self) {
        for handle in &self.0 {
            handle.abort();
        }
    }
}

/// Runs a single connected Daemon session over either Raw TCP No-SNI or CDN WebSocket.
pub async fn run_daemon_once(cfg: &DaemonConfig, prefer_mode: Option<&str>) -> Result<()> {
    run_daemon_once_with_p2p(cfg, prefer_mode, Some(DEFAULT_DIRECT_P2P_PORT)).await
}

/// Runs a single connected Daemon session over Relay and dynamically advertises fresh LAN/IPv6 P2P endpoints.
pub async fn run_daemon_once_with_p2p(
    cfg: &DaemonConfig,
    prefer_mode: Option<&str>,
    p2p_port: Option<u16>,
) -> Result<()> {
    let remote_server = cfg.server_url.starts_with("https://")
        || !is_loopback_endpoint(cfg.server_url.trim_start_matches("http://"));
    let mut candidates: Vec<&DaemonRelayEntry> = cfg
        .relays
        .iter()
        .filter(|r| {
            if let Some(m) = prefer_mode {
                return r.mode == m;
            }
            !(remote_server && r.mode == "raw_tcp_no_sni" && is_loopback_endpoint(&r.endpoint))
        })
        .collect();
    if candidates.is_empty() {
        candidates = cfg.relays.iter().collect();
    }
    if candidates.is_empty() {
        anyhow::bail!("no relay nodes configured");
    }

    let (out_tx, mut out_rx) = mpsc::channel::<RelayWireFrame>(256);
    let (in_tx, mut in_rx) = mpsc::channel::<RelayWireFrame>(256);
    let mut _task_guard = TaskAbortGuard(Vec::new());

    let bind_frame = RelayWireFrame::AuthBind {
        ticket: cfg.daemon_relay_ticket.clone(),
        lane: TrafficLane::ControlP0,
        generation: 1,
    };

    let mut connected = false;
    let mut last_err: Option<anyhow::Error> = None;

    for chosen in candidates {
        info!(
            "Connecting Coda Daemon `{}` ({}) via {} [{}]",
            cfg.identity.name, cfg.device_id, chosen.endpoint, chosen.mode
        );

        if chosen.mode == "raw_tcp_no_sni" {
            let conn_res = tokio::time::timeout(
                std::time::Duration::from_secs(4),
                TcpStream::connect(&chosen.endpoint),
            )
            .await;
            let stream = match conn_res {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    warn!("raw TCP relay {} failed ({e}); trying next relay", chosen.endpoint);
                    last_err = Some(e.into());
                    continue;
                }
                Err(_) => {
                    warn!("raw TCP relay {} timed out; trying next relay", chosen.endpoint);
                    last_err = Some(anyhow!("raw TCP relay {} timed out", chosen.endpoint));
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let (mut reader, mut writer) = stream.into_split();
            writer
                .write_all(&bind_frame.encode_length_delimited()?)
                .await?;

            let ack = read_raw_frame(&mut reader).await?;
            match ack {
                RelayWireFrame::AuthAck { ok: true, .. } => {}
                other => {
                    last_err = Some(anyhow!("unexpected raw TCP relay auth response: {other:?}"));
                    continue;
                }
            }

            let in_tx_c = in_tx.clone();
            let tcp_writer = tokio::spawn(async move {
                while let Some(f) = out_rx.recv().await {
                    if let Ok(bytes) = f.encode_length_delimited() {
                        if writer.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                }
            });
            _task_guard.0.push(tcp_writer.abort_handle());

            let tcp_reader = tokio::spawn(async move {
                while let Ok(f) = read_raw_frame(&mut reader).await {
                    if in_tx_c.send(f).await.is_err() {
                        break;
                    }
                }
            });
            _task_guard.0.push(tcp_reader.abort_handle());
            connected = true;
            break;
        } else {
            let ws_res = tokio::time::timeout(
                std::time::Duration::from_secs(6),
                connect_async(&chosen.endpoint),
            )
            .await;
            let (ws_stream, _) = match ws_res {
                Ok(Ok(pair)) => pair,
                Ok(Err(e)) => {
                    warn!("WSS relay {} failed ({e}); trying next relay", chosen.endpoint);
                    last_err = Some(e.into());
                    continue;
                }
                Err(_) => {
                    warn!("WSS relay {} timed out; trying next relay", chosen.endpoint);
                    last_err = Some(anyhow!("WSS relay {} timed out", chosen.endpoint));
                    continue;
                }
            };
            let (mut sink, mut stream) = ws_stream.split();
            sink.send(WsMsg::Text(serde_json::to_string(&bind_frame)?.into()))
                .await?;

            if let Some(Ok(WsMsg::Text(txt))) = stream.next().await {
                let ack: RelayWireFrame = serde_json::from_str(&txt)?;
                if !matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }) {
                    last_err = Some(anyhow!("unexpected WSS relay auth response: {ack:?}"));
                    continue;
                }
            } else {
                last_err = Some(anyhow!("WSS relay closed before AuthAck"));
                continue;
            }

            let in_tx_c = in_tx.clone();
            let wss_writer = tokio::spawn(async move {
                while let Some(f) = out_rx.recv().await {
                    if let Ok(txt) = serde_json::to_string(&f) {
                        if sink.send(WsMsg::Text(txt.into())).await.is_err() {
                            break;
                        }
                    }
                }
            });
            _task_guard.0.push(wss_writer.abort_handle());

            let wss_reader = tokio::spawn(async move {
                while let Some(Ok(msg)) = stream.next().await {
                    if let WsMsg::Text(t) = msg {
                        if let Ok(f) = serde_json::from_str::<RelayWireFrame>(&t) {
                            if in_tx_c.send(f).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
            _task_guard.0.push(wss_reader.abort_handle());
            connected = true;
            break;
        }
    }

    drop(in_tx);
    if !connected {
        return Err(last_err.unwrap_or_else(|| anyhow!("all relay endpoints failed")));
    }

    let endpoint_watcher = if let Some(port) = p2p_port {
        let initial_eps = discover_local_endpoints_with_port(port);
        let _ = out_tx
            .send(RelayWireFrame::AdvertiseEndpoints {
                endpoints: initial_eps.clone(),
            })
            .await;
        let out_tx_watch = out_tx.clone();
        let watcher = tokio::spawn(async move {
            let mut last_eps = initial_eps;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            interval.tick().await;
            loop {
                interval.tick().await;
                let fresh = discover_local_endpoints_with_port(port);
                if fresh != last_eps {
                    last_eps = fresh.clone();
                    if out_tx_watch
                        .send(RelayWireFrame::AdvertiseEndpoints { endpoints: fresh })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        _task_guard.0.push(watcher.abort_handle());
        Some(watcher)
    } else {
        None
    };

    let herdr = connect_herdr_with_install().await;
    if let Some(ref hc) = herdr {
        info!(
            "Bridged local herdr-core `{}` (v{})",
            hc.info().core_name,
            hc.info().version
        );
    } else {
        warn!("herdr-core not active yet; daemon running in degraded diagnostics mode");
    }

    let sessions: Arc<Mutex<HashMap<Uuid, E2eeSession>>> = Arc::new(Mutex::new(HashMap::new()));
    let active_ptys: Arc<Mutex<HashMap<(Uuid, String), PtyBridgeHandle>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let my_dh_secret = cfg.identity.x25519_secret()?;
    let my_dh_pub_b64 = cfg.identity.x25519_public_b64.clone();
    let my_device_id = cfg.device_id;
    let hostname = cfg.identity.name.clone();

    let event_forwarder = herdr.as_ref().map(|hc| {
        let h = spawn_herdr_event_forwarder(
            Arc::clone(hc),
            Arc::clone(&sessions),
            out_tx.clone(),
            my_device_id,
            my_dh_pub_b64.clone(),
        );
        _task_guard.0.push(h.abort_handle());
        h
    });

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
                let _ = send_e2ee_peer_message(
                    &sessions,
                    &out_tx,
                    my_device_id,
                    src_device_id,
                    &my_dh_pub_b64,
                    &reply_msg,
                    reply_lane,
                )
                .await;
            }
        }
    }

    if let Some(f) = event_forwarder {
        f.abort();
    }
    active_ptys.lock().await.clear();
    if let Some(w) = endpoint_watcher {
        w.abort();
    }

    Ok(())
}

/// Phase 3 Direct P2P Listener (`L0 LAN IPv4` + `L1 Mainland China Native IPv6 Direct`).
/// Auto-detects incoming protocol on the SAME port (`:24444`):
/// - Browser WebSocket (`GET ... HTTP/1.1` Upgrade -> JSON Text frames)
/// - Native Raw Binary TCP (`[u32_be_len][json]` for iOS `NWConnection` & GPUI Desktop)
pub async fn spawn_direct_p2p_listener(
    cfg: DaemonConfig,
    bind_addr: SocketAddr,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    info!(
        "Direct P2P zero-relay listener active on {} (RawTCP + LAN WebSocket auto-detect, device={})",
        local_addr, cfg.device_id
    );

    let herdr = connect_herdr_with_install().await;

    tokio::spawn(async move {
        while let Ok((stream, peer_addr)) = listener.accept().await {
            let cfg_clone = cfg.clone();
            let herdr_clone = herdr.clone();
            tokio::spawn(async move {
                let _ = stream.set_nodelay(true);
                let mut peek4 = [0u8; 4];
                let is_ws = matches!(
                    tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        stream.peek(&mut peek4),
                    )
                    .await,
                    Ok(Ok(n)) if n >= 4 && &peek4 == b"GET "
                );
                let res = if is_ws {
                    handle_direct_p2p_ws_socket(stream, peer_addr, cfg_clone, herdr_clone).await
                } else {
                    handle_direct_p2p_socket(stream, peer_addr, cfg_clone, herdr_clone).await
                };
                if let Err(e) = res {
                    warn!("direct P2P session from {peer_addr} ended: {e}");
                }
            });
        }
    });

    Ok(local_addr)
}

fn verify_direct_p2p_hello(cfg: &DaemonConfig, hello: RelayWireFrame) -> Result<(Uuid, String)> {
    match hello {
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
            Ok((client_device_id, client_x25519_pub_b64))
        }
        other => anyhow::bail!("expected DirectP2pHello, got {other:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_direct_p2p_frame(
    frame: RelayWireFrame,
    cfg: &DaemonConfig,
    herdr: &Option<Arc<HerdrClient>>,
    sessions: &Arc<Mutex<HashMap<Uuid, E2eeSession>>>,
    active_ptys: &Arc<Mutex<HashMap<(Uuid, String), PtyBridgeHandle>>>,
    out_tx: &mpsc::Sender<RelayWireFrame>,
) {
    match frame {
        RelayWireFrame::KeepAlive { ts_ms } => {
            let _ = out_tx.send(RelayWireFrame::KeepAlive { ts_ms }).await;
        }
        RelayWireFrame::RoutePacket {
            src_device_id,
            e2ee,
            ..
        } => {
            let req_msg = {
                let mut map = sessions.lock().await;
                let Some(s) = map.get_mut(&src_device_id) else {
                    return;
                };
                match s.open_message(&e2ee) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("P2P E2EE open error: {e}");
                        return;
                    }
                }
            };

            let my_dh_pub_b64 = &cfg.identity.x25519_public_b64;
            if let Some((reply_msg, reply_lane)) = handle_peer_request_full(
                src_device_id,
                req_msg,
                herdr.as_deref(),
                &cfg.identity.name,
                active_ptys,
                sessions,
                out_tx,
                cfg.device_id,
                my_dh_pub_b64,
            )
            .await
            {
                let _ = send_e2ee_peer_message(
                    sessions,
                    out_tx,
                    cfg.device_id,
                    src_device_id,
                    my_dh_pub_b64,
                    &reply_msg,
                    reply_lane,
                )
                .await;
            }
        }
        _ => {}
    }
}

async fn init_p2p_session_state(
    cfg: &DaemonConfig,
    client_device_id: Uuid,
    client_x25519_pub_b64: &str,
) -> Result<(
    Arc<Mutex<HashMap<Uuid, E2eeSession>>>,
    Arc<Mutex<HashMap<(Uuid, String), PtyBridgeHandle>>>,
)> {
    let my_dh_secret = cfg.identity.x25519_secret()?;
    let sess = E2eeSession::derive(&my_dh_secret, client_x25519_pub_b64, false)?;
    let sessions = Arc::new(Mutex::new(HashMap::new()));
    sessions.lock().await.insert(client_device_id, sess);
    let active_ptys = Arc::new(Mutex::new(HashMap::new()));
    Ok((sessions, active_ptys))
}

async fn handle_direct_p2p_socket(
    stream: TcpStream,
    peer_addr: SocketAddr,
    cfg: DaemonConfig,
    herdr: Option<Arc<HerdrClient>>,
) -> Result<()> {
    let (mut reader, mut writer) = stream.into_split();

    let hello = read_raw_frame(&mut reader).await?;
    let (client_device_id, client_x25519_pub_b64) = verify_direct_p2p_hello(&cfg, hello)?;

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
    let write_task = tokio::spawn(async move {
        while let Some(f) = out_rx.recv().await {
            if let Ok(bytes) = f.encode_length_delimited() {
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        }
    });

    let (sessions, active_ptys) =
        init_p2p_session_state(&cfg, client_device_id, &client_x25519_pub_b64).await?;
    let event_forwarder = herdr.as_ref().map(|hc| {
        spawn_herdr_event_forwarder(
            Arc::clone(hc),
            Arc::clone(&sessions),
            out_tx.clone(),
            cfg.device_id,
            cfg.identity.x25519_public_b64.clone(),
        )
    });

    while let Ok(frame) = read_raw_frame(&mut reader).await {
        dispatch_direct_p2p_frame(frame, &cfg, &herdr, &sessions, &active_ptys, &out_tx).await;
    }

    if let Some(f) = event_forwarder {
        f.abort();
    }
    active_ptys.lock().await.clear();
    write_task.abort();
    Ok(())
}

async fn handle_direct_p2p_ws_socket(
    stream: TcpStream,
    peer_addr: SocketAddr,
    cfg: DaemonConfig,
    herdr: Option<Arc<HerdrClient>>,
) -> Result<()> {
    let ws_stream = accept_async(stream)
        .await
        .with_context(|| format!("direct P2P WebSocket upgrade failed for {peer_addr}"))?;
    let (mut sink, mut rx) = ws_stream.split();

    let Some(first_msg) = rx.next().await else {
        anyhow::bail!("direct P2P WebSocket closed before hello");
    };
    let hello: RelayWireFrame = match first_msg? {
        WsMsg::Text(t) => serde_json::from_str(&t)?,
        WsMsg::Binary(b) => serde_json::from_slice(&b)?,
        other => anyhow::bail!("unexpected initial WS message: {other:?}"),
    };
    let (client_device_id, client_x25519_pub_b64) = verify_direct_p2p_hello(&cfg, hello)?;

    let path_kind = if peer_addr.is_ipv6() {
        "ipv6_ws"
    } else {
        "lan_ws"
    };
    let ack = RelayWireFrame::DirectP2pAck {
        ok: true,
        daemon_device_id: cfg.device_id,
        daemon_x25519_pub_b64: cfg.identity.x25519_public_b64.clone(),
        path_kind: path_kind.into(),
    };
    sink.send(WsMsg::Text(serde_json::to_string(&ack)?.into()))
        .await?;

    let (out_tx, mut out_rx) = mpsc::channel::<RelayWireFrame>(256);
    let write_task = tokio::spawn(async move {
        while let Some(f) = out_rx.recv().await {
            if let Ok(txt) = serde_json::to_string(&f) {
                if sink.send(WsMsg::Text(txt.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    let (sessions, active_ptys) =
        init_p2p_session_state(&cfg, client_device_id, &client_x25519_pub_b64).await?;
    let event_forwarder = herdr.as_ref().map(|hc| {
        spawn_herdr_event_forwarder(
            Arc::clone(hc),
            Arc::clone(&sessions),
            out_tx.clone(),
            cfg.device_id,
            cfg.identity.x25519_public_b64.clone(),
        )
    });

    while let Some(Ok(msg)) = rx.next().await {
        let frame_opt: Option<RelayWireFrame> = match msg {
            WsMsg::Text(t) => serde_json::from_str(&t).ok(),
            WsMsg::Binary(b) => serde_json::from_slice(&b).ok(),
            _ => None,
        };
        if let Some(frame) = frame_opt {
            dispatch_direct_p2p_frame(frame, &cfg, &herdr, &sessions, &active_ptys, &out_tx).await;
        }
    }

    if let Some(f) = event_forwarder {
        f.abort();
    }
    active_ptys.lock().await.clear();
    write_task.abort();
    Ok(())
}

/// Spawns a Local Wi-Fi UDP broadcast discovery responder on `bind_addr` (default `0.0.0.0:24444`).
/// Replies unicast to `{"coda_lan_probe":1,"target_device_id":"<uuid>"}` with the host's TCP P2P port.
pub async fn spawn_lan_udp_beacon(
    device_id: Uuid,
    tcp_p2p_port: u16,
    bind_addr: SocketAddr,
) -> Result<SocketAddr> {
    let sock = TokioUdpSocket::bind(bind_addr).await?;
    let local_addr = sock.local_addr()?;
    info!(
        "Local Wi-Fi UDP discovery beacon listening on {} (tcp_p2p_port={})",
        local_addr, tcp_p2p_port
    );

    tokio::spawn(async move {
        let mut buf = [0u8; 512];
        loop {
            let Ok((len, src_addr)) = sock.recv_from(&mut buf).await else {
                break;
            };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[..len]) else {
                continue;
            };
            if v.get("coda_lan_probe").and_then(|x| x.as_u64()) != Some(1) {
                continue;
            }
            let Some(target_str) = v.get("target_device_id").and_then(|x| x.as_str()) else {
                continue;
            };
            let Ok(target_id) = Uuid::parse_str(target_str) else {
                continue;
            };
            if target_id == device_id {
                let reply = json!({
                    "coda_lan_here": 1,
                    "device_id": device_id,
                    "tcp_port": tcp_p2p_port,
                });
                if let Ok(bytes) = serde_json::to_vec(&reply) {
                    let _ = sock.send_to(&bytes, src_addr).await;
                }
            }
        }
    });

    Ok(local_addr)
}

/// Sends a zero-config Wi-Fi UDP discovery probe (NEVER `connect()`s the UDP socket so unicast
/// replies from concrete LAN IPs are accepted cleanly) and returns the discovered TCP/WS `SocketAddr`.
pub async fn probe_lan_udp_peer(
    target_device_id: Uuid,
    broadcast_or_target: SocketAddr,
    timeout_ms: u64,
) -> Option<SocketAddr> {
    let sock = TokioUdpSocket::bind("0.0.0.0:0").await.ok()?;
    let _ = sock.set_broadcast(true);
    let probe_bytes = serde_json::to_vec(&json!({
        "coda_lan_probe": 1,
        "target_device_id": target_device_id,
    }))
    .ok()?;

    let _ = sock.send_to(&probe_bytes, broadcast_or_target).await;
    if matches!(broadcast_or_target.ip(), std::net::IpAddr::V4(v4) if v4.is_broadcast()) {
        let loopback = SocketAddr::from(([127, 0, 0, 1], broadcast_or_target.port()));
        let _ = sock.send_to(&probe_bytes, loopback).await;
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut buf = [0u8; 512];
    while tokio::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remain, sock.recv_from(&mut buf)).await {
            Ok(Ok((len, peer_src))) => {
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[..len]) else {
                    continue;
                };
                if v.get("coda_lan_here").and_then(|x| x.as_u64()) != Some(1) {
                    continue;
                }
                let Some(id_str) = v.get("device_id").and_then(|x| x.as_str()) else {
                    continue;
                };
                let Ok(id) = Uuid::parse_str(id_str) else {
                    continue;
                };
                let Some(tcp_port) = v.get("tcp_port").and_then(|x| x.as_u64()) else {
                    continue;
                };
                if id == target_device_id && (1..=65535).contains(&tcp_port) {
                    return Some(SocketAddr::new(peer_src.ip(), tcp_port as u16));
                }
            }
            _ => break,
        }
    }
    None
}

/// Starts embedded host P2P + UDP LAN beacon + Relay uplink inside desktop apps (e.g. `coda-desktop`)
/// if `cfg_path` exists and port `24444` is not already owned by a standalone `coda-daemon` process.
pub async fn start_embedded_host_if_configured(cfg_path: &Path) -> Option<SocketAddr> {
    if let Some(parent) = cfg_path.parent() {
        let pid_file = parent.join("coda-daemon.pid");
        if let Ok(raw) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = raw.trim().parse::<u32>() {
                if pid > 0 {
                    let alive = std::process::Command::new("kill")
                        .args(["-0", &pid.to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false);
                    if alive {
                        return None;
                    }
                }
            }
        }
    }

    let cfg = DaemonConfig::load(cfg_path).ok()?;
    let p2p_bind = SocketAddr::from(([0, 0, 0, 0], DEFAULT_DIRECT_P2P_PORT));
    let p2p_addr = match spawn_direct_p2p_listener(cfg.clone(), p2p_bind).await {
        Ok(addr) => addr,
        Err(_) => {
            let fallback = SocketAddr::from(([0, 0, 0, 0], 0));
            spawn_direct_p2p_listener(cfg.clone(), fallback).await.ok()?
        }
    };
    let _ = spawn_lan_udp_beacon(cfg.device_id, p2p_addr.port(), p2p_bind).await;

    let port = p2p_addr.port();
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_daemon_once_with_p2p(&cfg, None, Some(port)).await {
                warn!("embedded Mac host relay disconnected ({e}); reconnecting in 2s...");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    });

    Some(p2p_addr)
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
                            if !send_e2ee_peer_message(
                                &sessions_clone,
                                &out_tx_clone,
                                my_device_id,
                                src_device_id,
                                &dh_pub,
                                &chunk_msg,
                                TrafficLane::ControlP0,
                            )
                            .await
                            {
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
        PeerMessage::KillTerminal { req_id, terminal } => {
            {
                let mut map = active_ptys.lock().await;
                map.retain(|(_, term), _| term != &terminal);
            }
            let (ok, detail) = match herdr {
                Some(hc) => match hc.kill_and_remove(&terminal).await {
                    Ok(()) => (true, format!("killed:{terminal}")),
                    Err(e) => (false, format!("kill_failed:{terminal}:{e}")),
                },
                None => (false, "herdr-core unavailable".into()),
            };
            return Some((
                PeerMessage::ActionAck { req_id, ok, detail },
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
        PeerMessage::ListDirectory {
            req_id,
            root_path,
            sub_path,
        } => {
            let clean_sub = sub_path.trim_matches('/');
            let base = std::path::PathBuf::from(&root_path);
            let target = if clean_sub.is_empty() {
                base
            } else {
                let mut p = base;
                for comp in std::path::Path::new(clean_sub).components() {
                    if let std::path::Component::Normal(c) = comp {
                        p.push(c);
                    }
                }
                p
            };
            match tokio::fs::read_dir(&target).await {
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
                        let meta = ent.metadata().await.ok();
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
                        error: Some(format!("{}: {e}", target.display())),
                    },
                    TrafficLane::BulkP1,
                ),
            }
        }
        PeerMessage::ReadFilePreview {
            req_id,
            path,
            max_bytes,
        } => {
            let cap = max_bytes.clamp(1024, 256_000);
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    let slice = &bytes[..bytes.len().min(cap)];
                    if slice.contains(&0u8) {
                        (
                            PeerMessage::FilePreviewResponse {
                                req_id,
                                path,
                                content: format!(
                                    "[Binary file · {} bytes — preview skipped]",
                                    bytes.len()
                                ),
                                error: None,
                            },
                            TrafficLane::BulkP1,
                        )
                    } else {
                        let mut text = String::from_utf8_lossy(slice).to_string();
                        if bytes.len() > cap {
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
                let msg = self.e2ee.open_message(&e2ee)?;
                if matches!(msg, PeerMessage::HerdrLiveEvent { .. }) {
                    continue;
                }
                return Ok(msg);
            }
        }
    }
}

// ============================================================================
// Auto-SmartPath Negotiator (`SmartPeerClient`): LAN/IPv6 P2P <-> Relay Failover
// ============================================================================

#[derive(Debug, Clone)]
pub struct SmartRelayFallbackConfig {
    pub raw_tcp_addr: Option<String>,
    pub wss_url: Option<String>,
    pub relay_ticket: String,
    pub peer_x25519_pub_b64: String,
}

enum RelaySocketTransport {
    RawTcp {
        reader: tokio::net::tcp::OwnedReadHalf,
        writer: tokio::net::tcp::OwnedWriteHalf,
    },
    Wss {
        sink: futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<TcpStream>,
            >,
            WsMsg,
        >,
        stream: futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<TcpStream>,
            >,
        >,
    },
}

struct RelayClientConn {
    route_label: String,
    transport: RelaySocketTransport,
    e2ee: E2eeSession,
    client_device_id: Uuid,
    target_device_id: Uuid,
    client_x25519_pub_b64: String,
}

impl RelayClientConn {
    async fn connect(
        client_id: &StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        cfg: &SmartRelayFallbackConfig,
    ) -> Result<Self> {
        let e2ee = E2eeSession::derive(
            &client_id.x25519_secret()?,
            &cfg.peer_x25519_pub_b64,
            true,
        )?;
        let gen = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let bind = RelayWireFrame::AuthBind {
            ticket: cfg.relay_ticket.clone(),
            lane: TrafficLane::ControlP0,
            generation: gen.max(1),
        };

        if let Some(ref raw_addr) = cfg.raw_tcp_addr {
            if let Ok(Ok(stream)) = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                TcpStream::connect(raw_addr),
            )
            .await
            {
                let _ = stream.set_nodelay(true);
                let (mut reader, mut writer) = stream.into_split();
                writer.write_all(&bind.encode_length_delimited()?).await?;
                let ack = read_raw_frame(&mut reader).await?;
                if matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }) {
                    return Ok(Self {
                        route_label: "relay_raw_tcp".into(),
                        transport: RelaySocketTransport::RawTcp { reader, writer },
                        e2ee,
                        client_device_id,
                        target_device_id,
                        client_x25519_pub_b64: client_id.x25519_public_b64.clone(),
                    });
                }
            }
        }

        if let Some(ref wss_url) = cfg.wss_url {
            let (ws_stream, _) = connect_async(wss_url).await?;
            let (mut sink, mut stream) = ws_stream.split();
            sink.send(WsMsg::Text(serde_json::to_string(&bind)?.into()))
                .await?;
            if let Some(Ok(WsMsg::Text(txt))) = stream.next().await {
                let ack: RelayWireFrame = serde_json::from_str(&txt)?;
                if matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }) {
                    return Ok(Self {
                        route_label: "relay_wss".into(),
                        transport: RelaySocketTransport::Wss { sink, stream },
                        e2ee,
                        client_device_id,
                        target_device_id,
                        client_x25519_pub_b64: client_id.x25519_public_b64.clone(),
                    });
                }
            }
        }

        anyhow::bail!("no reachable relay fallback endpoint")
    }

    async fn request(&mut self, msg: &PeerMessage, lane: TrafficLane) -> Result<PeerMessage> {
        let expected_req_id = msg.request_id().map(|s| s.to_string());
        let sealed = self.e2ee.seal_message(msg)?;
        let pkt = RelayWireFrame::RoutePacket {
            src_device_id: self.client_device_id,
            dst_device_id: self.target_device_id,
            lane,
            e2ee: sealed,
            sender_x25519_pub_b64: Some(self.client_x25519_pub_b64.clone()),
        };

        match &mut self.transport {
            RelaySocketTransport::RawTcp { reader, writer } => {
                writer.write_all(&pkt.encode_length_delimited()?).await?;
                loop {
                    let frame = read_raw_frame(reader).await?;
                    match frame {
                        RelayWireFrame::RoutePacket { e2ee, .. } => {
                            let opened = self.e2ee.open_message(&e2ee)?;
                            if matches!(
                                opened,
                                PeerMessage::HerdrLiveEvent { .. }
                                    | PeerMessage::PtyOutputChunk { .. }
                            ) {
                                continue;
                            }
                            if let Some(ref want) = expected_req_id {
                                if opened.response_req_id() != Some(want.as_str()) {
                                    continue;
                                }
                            }
                            return Ok(opened);
                        }
                        RelayWireFrame::Error { code, message } => {
                            anyhow::bail!("relay error ({code}): {message}");
                        }
                        _ => {}
                    }
                }
            }
            RelaySocketTransport::Wss { sink, stream } => {
                sink.send(WsMsg::Text(serde_json::to_string(&pkt)?.into()))
                    .await?;
                while let Some(Ok(msg)) = stream.next().await {
                    if let WsMsg::Text(t) = msg {
                        if let Ok(frame) = serde_json::from_str::<RelayWireFrame>(&t) {
                            match frame {
                                RelayWireFrame::RoutePacket { e2ee, .. } => {
                                    let opened = self.e2ee.open_message(&e2ee)?;
                                    if matches!(
                                        opened,
                                        PeerMessage::HerdrLiveEvent { .. }
                                            | PeerMessage::PtyOutputChunk { .. }
                                    ) {
                                        continue;
                                    }
                                    if let Some(ref want) = expected_req_id {
                                        if opened.response_req_id() != Some(want.as_str()) {
                                            continue;
                                        }
                                    }
                                    return Ok(opened);
                                }
                                RelayWireFrame::Error { code, message } => {
                                    anyhow::bail!("relay error ({code}): {message}");
                                }
                                _ => {}
                            }
                        }
                    }
                }
                anyhow::bail!("relay WSS stream closed before reply")
            }
        }
    }
}

/// Automatically negotiates the fastest path between Direct LAN/IPv6 P2P and Cloud Relay,
/// maintaining separate E2EE sessions per leg for lossless hot-upgrade and relay failover.
pub struct SmartPeerClient {
    client_id: StoredDeviceIdentity,
    client_device_id: Uuid,
    target_device_id: Uuid,
    endpoints: Vec<NetworkEndpointCandidate>,
    udp_probe_addr: Option<SocketAddr>,
    relay_cfg: Option<SmartRelayFallbackConfig>,
    direct: Option<DirectP2pClient>,
    relay_leg: Option<RelayClientConn>,
}

impl SmartPeerClient {
    pub async fn connect_auto(
        client_id: StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        endpoints: Vec<NetworkEndpointCandidate>,
        udp_probe_addr: Option<SocketAddr>,
        relay_cfg: Option<SmartRelayFallbackConfig>,
    ) -> Result<Self> {
        let mut me = Self {
            client_id,
            client_device_id,
            target_device_id,
            endpoints,
            udp_probe_addr,
            relay_cfg,
            direct: None,
            relay_leg: None,
        };

        if me.try_upgrade_to_direct().await {
            return Ok(me);
        }

        if let Some(ref rcfg) = me.relay_cfg {
            me.relay_leg = Some(
                RelayClientConn::connect(&me.client_id, client_device_id, target_device_id, rcfg)
                    .await?,
            );
            return Ok(me);
        }

        anyhow::bail!("neither Direct LAN/IPv6 P2P nor Relay could connect to {target_device_id}")
    }

    pub fn active_route(&self) -> &str {
        if let Some(ref p2p) = self.direct {
            &p2p.path_kind
        } else if let Some(ref relay) = self.relay_leg {
            &relay.route_label
        } else {
            "disconnected"
        }
    }

    pub fn is_direct_p2p(&self) -> bool {
        self.direct.is_some()
    }

    pub async fn try_upgrade_to_direct(&mut self) -> bool {
        if self.direct.is_some() {
            return true;
        }
        let mut candidates: Vec<String> = Vec::new();
        if let Some(probe_addr) = self.udp_probe_addr {
            if let Some(found) = probe_lan_udp_peer(self.target_device_id, probe_addr, 120).await {
                candidates.push(found.to_string());
            }
        }
        for ep in &self.endpoints {
            if matches!(ep.kind.as_str(), "lan_ipv4" | "cn_public_ipv6")
                && !candidates.contains(&ep.addr)
            {
                candidates.push(ep.addr.clone());
            }
        }
        for addr in candidates {
            if let Ok(p2p) = DirectP2pClient::connect_direct(
                &addr,
                &self.client_id,
                self.client_device_id,
                self.target_device_id,
            )
            .await
            {
                self.direct = Some(p2p);
                return true;
            }
        }
        false
    }

    pub async fn request(&mut self, msg: &PeerMessage, lane: TrafficLane) -> Result<PeerMessage> {
        if let Some(p2p) = self.direct.as_mut() {
            let direct_res = match p2p.send_message(msg, lane).await {
                Ok(()) => tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    p2p.recv_message(),
                )
                .await
                .map_err(|_| anyhow!("direct P2P recv timed out"))
                .and_then(|r| r),
                Err(e) => Err(e),
            };
            match direct_res {
                Ok(reply) => return Ok(reply),
                Err(e) if self.relay_cfg.is_some() => {
                    warn!("Direct P2P leg dropped ({e}); failing over seamlessly to Relay");
                    self.direct = None;
                }
                Err(e) => return Err(e),
            }
        }

        if self.relay_leg.is_none() {
            let rcfg = self
                .relay_cfg
                .as_ref()
                .ok_or_else(|| anyhow!("no relay fallback configured"))?;
            self.relay_leg = Some(
                RelayClientConn::connect(
                    &self.client_id,
                    self.client_device_id,
                    self.target_device_id,
                    rcfg,
                )
                .await?,
            );
        }

        self.relay_leg
            .as_mut()
            .expect("relay leg initialized")
            .request(msg, lane)
            .await
    }
}

async fn try_connect_direct_auto(
    client_id: &StoredDeviceIdentity,
    client_device_id: Uuid,
    target_device_id: Uuid,
    endpoints: &[NetworkEndpointCandidate],
    udp_probe_addr: Option<SocketAddr>,
) -> Option<DirectP2pClient> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(probe_addr) = udp_probe_addr {
        if let Some(found) = probe_lan_udp_peer(target_device_id, probe_addr, 120).await {
            candidates.push(found.to_string());
        }
    }
    for ep in endpoints {
        if matches!(ep.kind.as_str(), "lan_ipv4" | "cn_public_ipv6")
            && !candidates.contains(&ep.addr)
        {
            candidates.push(ep.addr.clone());
        }
    }
    for addr in candidates {
        if let Ok(p2p) =
            DirectP2pClient::connect_direct(&addr, client_id, client_device_id, target_device_id)
                .await
        {
            return Some(p2p);
        }
    }
    None
}

// ============================================================================
// Full-Duplex Persistent Multiplexed Peer Session (`LivePeerSession`)
// ============================================================================

#[derive(Debug, Clone)]
pub enum PeerAsyncEvent {
    PtyOutputChunk {
        terminal: String,
        seq: u64,
        data_b64: String,
    },
    HerdrLiveEvent {
        seq: u64,
        event_kind: String,
        terminal: Option<HerdrTerminalSummary>,
        raw_event: serde_json::Value,
    },
    Disconnected(String),
}

enum PeerReaderHalf {
    RawTcp(OwnedReadHalf),
    Wss(
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
        >,
    ),
}

enum PeerWriterHalf {
    RawTcp(OwnedWriteHalf),
    Wss(
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
            WsMsg,
        >,
    ),
}

/// Full-duplex, single-writer/single-reader E2EE client session supporting concurrent
/// correlated RPC requests, one-way low-latency PTY input, and async push event streams.
pub struct LivePeerSession {
    route_label: String,
    is_direct: bool,
    outbound_tx: mpsc::Sender<(PeerMessage, TrafficLane)>,
    pending: Arc<StdMutex<HashMap<String, oneshot::Sender<PeerMessage>>>>,
    events_tx: broadcast::Sender<PeerAsyncEvent>,
    reader_abort: tokio::task::AbortHandle,
    writer_abort: tokio::task::AbortHandle,
}

impl Drop for LivePeerSession {
    fn drop(&mut self) {
        self.reader_abort.abort();
        self.writer_abort.abort();
    }
}

impl LivePeerSession {
    pub async fn connect_auto(
        client_id: StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        endpoints: Vec<NetworkEndpointCandidate>,
        udp_probe_addr: Option<SocketAddr>,
        relay_cfg: Option<SmartRelayFallbackConfig>,
    ) -> Result<Self> {
        let client_x25519_pub_b64 = client_id.x25519_public_b64.clone();
        let (reader_half, writer_half, e2ee, route_label, is_direct) =
            if let Some(p2p) = try_connect_direct_auto(
                &client_id,
                client_device_id,
                target_device_id,
                &endpoints,
                udp_probe_addr,
            )
            .await
            {
                (
                    PeerReaderHalf::RawTcp(p2p.reader),
                    PeerWriterHalf::RawTcp(p2p.writer),
                    p2p.e2ee,
                    p2p.path_kind,
                    true,
                )
            } else if let Some(ref rcfg) = relay_cfg {
                let relay = RelayClientConn::connect(
                    &client_id,
                    client_device_id,
                    target_device_id,
                    rcfg,
                )
                .await?;
                let (r, w) = match relay.transport {
                    RelaySocketTransport::RawTcp { reader, writer } => {
                        (PeerReaderHalf::RawTcp(reader), PeerWriterHalf::RawTcp(writer))
                    }
                    RelaySocketTransport::Wss { sink, stream } => {
                        (PeerReaderHalf::Wss(stream), PeerWriterHalf::Wss(sink))
                    }
                };
                (r, w, relay.e2ee, relay.route_label, false)
            } else {
                anyhow::bail!(
                    "neither Direct LAN/IPv6 P2P nor Relay could connect to {target_device_id}"
                );
            };

        let e2ee_state = Arc::new(StdMutex::new(e2ee));
        let pending: Arc<StdMutex<HashMap<String, oneshot::Sender<PeerMessage>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<(PeerMessage, TrafficLane)>(512);
        let (events_tx, _) = broadcast::channel::<PeerAsyncEvent>(1024);

        let e2ee_w = Arc::clone(&e2ee_state);
        let mut writer = writer_half;
        let dh_pub_w = client_x25519_pub_b64;
        let writer_handle = tokio::spawn(async move {
            while let Some((msg, lane)) = outbound_rx.recv().await {
                let sealed = {
                    let mut guard = e2ee_w.lock().expect("e2ee mutex poisoned");
                    guard.seal_message(&msg)
                };
                let Ok(e2ee) = sealed else {
                    break;
                };
                let frame = RelayWireFrame::RoutePacket {
                    src_device_id: client_device_id,
                    dst_device_id: target_device_id,
                    lane,
                    e2ee,
                    sender_x25519_pub_b64: Some(dh_pub_w.clone()),
                };
                let send_ok = match &mut writer {
                    PeerWriterHalf::RawTcp(w) => match frame.encode_length_delimited() {
                        Ok(bytes) => w.write_all(&bytes).await.is_ok(),
                        Err(_) => false,
                    },
                    PeerWriterHalf::Wss(sink) => match serde_json::to_string(&frame) {
                        Ok(txt) => sink.send(WsMsg::Text(txt.into())).await.is_ok(),
                        Err(_) => false,
                    },
                };
                if !send_ok {
                    break;
                }
            }
        });
        let writer_abort = writer_handle.abort_handle();

        let e2ee_r = Arc::clone(&e2ee_state);
        let pending_r = Arc::clone(&pending);
        let events_tx_r = events_tx.clone();
        let writer_abort_for_reader = writer_abort.clone();
        let mut reader = reader_half;
        let reader_handle = tokio::spawn(async move {
            let mut disconnect_reason = "peer stream closed".to_string();
            loop {
                let next_frame: Option<RelayWireFrame> = match &mut reader {
                    PeerReaderHalf::RawTcp(r) => read_raw_frame(r).await.ok(),
                    PeerReaderHalf::Wss(stream) => loop {
                        match stream.next().await {
                            Some(Ok(WsMsg::Text(t))) => {
                                if let Ok(f) = serde_json::from_str::<RelayWireFrame>(&t) {
                                    break Some(f);
                                }
                            }
                            Some(Ok(WsMsg::Binary(b))) => {
                                if let Ok(f) = serde_json::from_slice::<RelayWireFrame>(&b) {
                                    break Some(f);
                                }
                            }
                            Some(Ok(WsMsg::Ping(_))) | Some(Ok(WsMsg::Pong(_))) => continue,
                            Some(Ok(WsMsg::Close(_))) | Some(Err(_)) | None => break None,
                            _ => continue,
                        }
                    },
                };

                let Some(frame) = next_frame else {
                    break;
                };

                match frame {
                    RelayWireFrame::RoutePacket { e2ee, .. } => {
                        let opened = {
                            let mut guard = e2ee_r.lock().expect("e2ee mutex poisoned");
                            guard.open_message(&e2ee)
                        };
                        let Ok(msg) = opened else {
                            continue;
                        };
                        if let Some(req_id) = msg.response_req_id() {
                            let waiter = {
                                let mut map = pending_r.lock().expect("pending mutex poisoned");
                                map.remove(req_id)
                            };
                            if let Some(tx) = waiter {
                                let _ = tx.send(msg);
                                continue;
                            }
                        }
                        match msg {
                            PeerMessage::PtyOutputChunk {
                                terminal,
                                seq,
                                data_b64,
                            } => {
                                let _ = events_tx_r.send(PeerAsyncEvent::PtyOutputChunk {
                                    terminal,
                                    seq,
                                    data_b64,
                                });
                            }
                            PeerMessage::HerdrLiveEvent {
                                seq,
                                event_kind,
                                terminal,
                                raw_event,
                            } => {
                                let _ = events_tx_r.send(PeerAsyncEvent::HerdrLiveEvent {
                                    seq,
                                    event_kind,
                                    terminal,
                                    raw_event,
                                });
                            }
                            _ => {}
                        }
                    }
                    RelayWireFrame::Error { code, message } => {
                        disconnect_reason = format!("relay error ({code}): {message}");
                        break;
                    }
                    _ => {}
                }
            }

            {
                let mut map = pending_r.lock().expect("pending mutex poisoned");
                map.clear();
            }
            let _ = events_tx_r.send(PeerAsyncEvent::Disconnected(disconnect_reason));
            writer_abort_for_reader.abort();
        });
        let reader_abort = reader_handle.abort_handle();

        Ok(Self {
            route_label,
            is_direct,
            outbound_tx,
            pending,
            events_tx,
            reader_abort,
            writer_abort,
        })
    }

    pub fn route_label(&self) -> &str {
        &self.route_label
    }

    pub fn is_direct_p2p(&self) -> bool {
        self.is_direct
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<PeerAsyncEvent> {
        self.events_tx.subscribe()
    }

    pub async fn send_oneway(&self, msg: PeerMessage, lane: TrafficLane) -> Result<()> {
        self.outbound_tx
            .send((msg, lane))
            .await
            .map_err(|_| anyhow!("live peer session closed"))
    }

    pub async fn request(
        &self,
        msg: PeerMessage,
        lane: TrafficLane,
        timeout: std::time::Duration,
    ) -> Result<PeerMessage> {
        let req_id = msg
            .request_id()
            .ok_or_else(|| anyhow!("PeerMessage has no req_id for correlated RPC"))?
            .to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().expect("pending mutex poisoned");
            map.insert(req_id.clone(), tx);
        }
        if let Err(e) = self.outbound_tx.send((msg, lane)).await {
            self.pending
                .lock()
                .expect("pending mutex poisoned")
                .remove(&req_id);
            return Err(anyhow!("live peer session closed while sending: {e}"));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err(anyhow!("live peer session closed before response for {req_id}")),
            Err(_) => {
                self.pending
                    .lock()
                    .expect("pending mutex poisoned")
                    .remove(&req_id);
                Err(anyhow!("live peer RPC `{req_id}` timed out"))
            }
        }
    }

    pub fn close(&self) {
        self.reader_abort.abort();
        self.writer_abort.abort();
    }
}
