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
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex, OnceLock,
    },
};

use anyhow::{anyhow, Context, Result};
use base64::{
    engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64_URL},
    Engine as _,
};
use coda_herdr::{
    build_augmented_path, ensure_or_install_hc_binary, scan_git_workspaces,
    spawn_native_shell_pty, spawn_pty_attach, HerdrClient, PtyBridgeHandle, PtyOutputEvent,
};
use coda_protocol::{
    current_unix_ms, is_timestamp_fresh, p2p_ack_v2_signing_bytes, p2p_hello_v2_signing_bytes,
    p2p_hello_v2_signing_bytes_with_zstd, relay_ticket_renew_signing_bytes,
    validate_ed25519_pub_b64, validate_x25519_pub_b64, AccountLiveEvent, BranchWorktreeItem,
    E2eeSession, HerdrTerminalSummary, NetworkEndpointCandidate, PeerMessage, RelaySessionAuth,
    RelayWireFrame, RemoteFileItem, RepoWorkspaceSummary, StoredDeviceIdentity, TrafficLane,
    TrustedPeerEntry, VoiceModelConfig, HANDSHAKE_MAX_SKEW_MS, MAX_AUTH_FRAME_BYTES,
    MAX_WIRE_FRAME_BYTES,
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
    sync::{broadcast, mpsc, oneshot, watch, Mutex, RwLock, Semaphore},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config, connect_async_with_config,
    tungstenite::{
        handshake::server::{ErrorResponse, Request as WsRequest, Response as WsResponse},
        http::StatusCode as WsStatusCode,
        protocol::WebSocketConfig,
        Message as WsMsg,
    },
};
use tracing::{info, warn};
use uuid::Uuid;

/// Default Direct P2P port for LAN IPv4 & Mainland China Public IPv6 zero-relay hole-punching.
pub const DEFAULT_DIRECT_P2P_PORT: u16 = 24444;

const MAX_CONCURRENT_PEER_RPCS: usize = 32;
const MAX_DIRECT_P2P_CONNS: usize = 64;
const MAX_SESSIONS_PER_PEER: usize = 8;
const MAX_HTTP_PREVIEW_BYTES: usize = 4 * 1024 * 1024;
const MAX_UPLOAD_FILE_BYTES: usize = 8 * 1024 * 1024;
const WS_KEEPALIVE_SECS: u64 = 25;
const HANDSHAKE_REPLAY_MAX_ENTRIES: usize = 4096;
const TICKET_RENEWAL_LEAD_SECS: i64 = 7 * 86_400;

static LAST_RELAY_GEN: AtomicU64 = AtomicU64::new(0);
static ATOMIC_SAVE_COUNTER: AtomicU64 = AtomicU64::new(0);
static PREVIEW_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static DAEMON_HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn next_monotonic_generation() -> u64 {
    let now = current_unix_ms().max(1);
    let mut prev = LAST_RELAY_GEN.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev.saturating_add(1)).max(1);
        match LAST_RELAY_GEN.compare_exchange_weak(
            prev,
            next,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

fn daemon_http_client() -> &'static reqwest::Client {
    DAEMON_HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

fn preview_http_client() -> &'static reqwest::Client {
    PREVIEW_HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

fn bounded_ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_WIRE_FRAME_BYTES))
        .max_frame_size(Some(MAX_WIRE_FRAME_BYTES))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonRelayEntry {
    pub id: String,
    pub mode: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerIdentityPin {
    pub ed25519_pub_b64: String,
    pub x25519_pub_b64: String,
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
    #[serde(default)]
    pub ed25519_pubkey: String,
    pub x25519_pubkey: String,
    #[serde(default)]
    pub endpoints: Vec<NetworkEndpointCandidate>,
    pub is_online: bool,
}

impl RemoteHostSummary {
    pub fn identity_pin(&self) -> PeerIdentityPin {
        PeerIdentityPin {
            ed25519_pub_b64: self.ed25519_pubkey.clone(),
            x25519_pub_b64: self.x25519_pubkey.clone(),
        }
    }
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

fn atomic_write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }

    let seq = ATOMIC_SAVE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("coda.tmp");
    let tmp_path = parent.join(format!(".{fname}.tmp.{}.{seq}", std::process::id()));

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp_path, bytes)?;
    }

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    Ok(())
}

/// Exclusive non-blocking OS file lock preventing concurrent `coda-daemon` and embedded desktop host startup.
pub struct DaemonProcessLock {
    #[cfg(unix)]
    _file: std::fs::File,
    #[cfg(not(unix))]
    _marker: (),
}

/// Attempts to acquire the exclusive daemon lock next to `cfg_path`.
/// Returns `Ok(None)` if another process already holds the lock.
pub fn try_acquire_daemon_lock(cfg_path: &Path) -> Result<Option<DaemonProcessLock>> {
    let lock_path = DaemonConfig::lock_path(cfg_path);
    let parent = lock_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(&lock_path)?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK)
                || err.raw_os_error() == Some(libc::EAGAIN)
            {
                return Ok(None);
            }
            return Err(err.into());
        }
        let _ = file.set_len(0);
        let _ = writeln!(file, "{}", std::process::id());
        let _ = file.sync_all();
        Ok(Some(DaemonProcessLock { _file: file }))
    }
    #[cfg(not(unix))]
    {
        let _ = lock_path;
        Ok(Some(DaemonProcessLock { _marker: () }))
    }
}

/// Returns `Some(pid)` if another live process currently holds `coda-daemon.lock`.
pub fn active_daemon_lock_pid(cfg_path: &Path) -> Option<u32> {
    let lock_path = DaemonConfig::lock_path(cfg_path);
    if !lock_path.exists() {
        return None;
    }
    match try_acquire_daemon_lock(cfg_path) {
        Ok(Some(lock)) => {
            drop(lock);
            None
        }
        Ok(None) => std::fs::read_to_string(&lock_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .or(Some(0)),
        Err(_) => None,
    }
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

    pub fn lock_path(cfg_path: &Path) -> PathBuf {
        cfg_path.with_file_name("coda-daemon.lock")
    }

    pub fn trusted_peers_path(cfg_path: &Path) -> PathBuf {
        cfg_path.with_file_name("trusted_peers.json")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let body = serde_json::to_string_pretty(self)?;
        atomic_write_private_file(path, body.as_bytes())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let txt = std::fs::read_to_string(path)
            .with_context(|| format!("daemon config not found at {}", path.display()))?;
        Ok(serde_json::from_str(&txt)?)
    }
}

#[derive(Default)]
struct TimedReplayCache {
    seen: HashMap<String, tokio::time::Instant>,
}

impl TimedReplayCache {
    fn contains_fresh(&mut self, key: &str, ttl: std::time::Duration) -> bool {
        let now = tokio::time::Instant::now();
        self.seen.retain(|_, ts| now.duration_since(*ts) <= ttl);
        self.seen.contains_key(key)
    }

    fn insert(&mut self, key: String, ttl: std::time::Duration) {
        let now = tokio::time::Instant::now();
        self.seen.retain(|_, ts| now.duration_since(*ts) <= ttl);
        if self.seen.len() >= HANDSHAKE_REPLAY_MAX_ENTRIES {
            let remove_count = HANDSHAKE_REPLAY_MAX_ENTRIES / 8;
            let keys: Vec<String> = self.seen.keys().take(remove_count).cloned().collect();
            for k in keys {
                self.seen.remove(&k);
            }
        }
        self.seen.insert(key, now);
    }

    fn check_and_insert(&mut self, key: String, ttl: std::time::Duration) -> bool {
        if self.contains_fresh(&key, ttl) {
            return false;
        }
        self.insert(key, ttl);
        true
    }
}

#[derive(Clone)]
struct ServerPeerSyncConfig {
    server_url: String,
    session_token: Option<String>,
    relay_ticket: String,
}

/// Thread-safe, disk-cached store of same-account trusted peer identities (`Ed25519` + `X25519`).
pub struct TrustedPeersStore {
    peers: RwLock<HashMap<Uuid, TrustedPeerEntry>>,
    disk_path: Option<PathBuf>,
    server: RwLock<Option<ServerPeerSyncConfig>>,
    refresh_state: Mutex<(f64, tokio::time::Instant)>,
    miss_cache: Mutex<HashMap<Uuid, tokio::time::Instant>>,
    updates_tx: broadcast::Sender<()>,
}

impl TrustedPeersStore {
    pub fn new_with_peers(entries: Vec<TrustedPeerEntry>) -> Arc<Self> {
        let map = entries
            .into_iter()
            .filter(|e| {
                validate_ed25519_pub_b64(&e.ed25519_pub_b64).is_ok()
                    && validate_x25519_pub_b64(&e.x25519_pub_b64).is_ok()
            })
            .map(|e| (e.device_id, e))
            .collect();
        let (updates_tx, _) = broadcast::channel(32);
        Arc::new(Self {
            peers: RwLock::new(map),
            disk_path: None,
            server: RwLock::new(None),
            refresh_state: Mutex::new((4.0, tokio::time::Instant::now())),
            miss_cache: Mutex::new(HashMap::new()),
            updates_tx,
        })
    }

    pub fn from_daemon_config(cfg: &DaemonConfig, disk_path: Option<PathBuf>) -> Arc<Self> {
        let mut map = HashMap::new();
        if let Some(ref p) = disk_path {
            if let Ok(raw) = std::fs::read_to_string(p) {
                if let Ok(list) = serde_json::from_str::<Vec<TrustedPeerEntry>>(&raw) {
                    for entry in list {
                        if validate_ed25519_pub_b64(&entry.ed25519_pub_b64).is_ok()
                            && validate_x25519_pub_b64(&entry.x25519_pub_b64).is_ok()
                        {
                            map.insert(entry.device_id, entry);
                        }
                    }
                }
            }
        }
        let server_url = cfg.server_url.trim().trim_end_matches('/').to_string();
        let server = if !server_url.is_empty() {
            Some(ServerPeerSyncConfig {
                server_url,
                session_token: cfg.session_token.clone(),
                relay_ticket: cfg.daemon_relay_ticket.clone(),
            })
        } else {
            None
        };
        let (updates_tx, _) = broadcast::channel(32);
        Arc::new(Self {
            peers: RwLock::new(map),
            disk_path,
            server: RwLock::new(server),
            refresh_state: Mutex::new((4.0, tokio::time::Instant::now())),
            miss_cache: Mutex::new(HashMap::new()),
            updates_tx,
        })
    }

    pub fn subscribe_updates(&self) -> broadcast::Receiver<()> {
        self.updates_tx.subscribe()
    }

    pub async fn update_relay_ticket(&self, new_ticket: String) {
        if let Some(ref mut srv) = *self.server.write().await {
            srv.relay_ticket = new_ticket;
        }
    }

    pub async fn replace_all(&self, entries: Vec<TrustedPeerEntry>) {
        let valid: Vec<TrustedPeerEntry> = entries
            .into_iter()
            .filter(|e| {
                validate_ed25519_pub_b64(&e.ed25519_pub_b64).is_ok()
                    && validate_x25519_pub_b64(&e.x25519_pub_b64).is_ok()
            })
            .collect();
        if let Some(ref p) = self.disk_path {
            if let Ok(json_bytes) = serde_json::to_vec_pretty(&valid) {
                let _ = atomic_write_private_file(p, &json_bytes);
            }
        }
        let map: HashMap<Uuid, TrustedPeerEntry> =
            valid.into_iter().map(|e| (e.device_id, e)).collect();
        let changed = {
            let mut guard = self.peers.write().await;
            let is_diff = guard.len() != map.len()
                || map.iter().any(|(k, v)| {
                    guard
                        .get(k)
                        .map(|o| {
                            o.ed25519_pub_b64 == v.ed25519_pub_b64
                                && o.x25519_pub_b64 == v.x25519_pub_b64
                                && o.role == v.role
                        })
                        != Some(true)
                });
            *guard = map;
            is_diff
        };
        self.miss_cache.lock().await.clear();
        if changed {
            let _ = self.updates_tx.send(());
        }
    }

    pub async fn get(&self, device_id: &Uuid) -> Option<TrustedPeerEntry> {
        self.peers.read().await.get(device_id).cloned()
    }

    pub async fn contains(&self, device_id: &Uuid) -> bool {
        self.peers.read().await.contains_key(device_id)
    }

    pub async fn refresh_from_server(&self, force: bool) -> Result<bool> {
        let srv = {
            let guard = self.server.read().await;
            match guard.as_ref() {
                Some(s) => s.clone(),
                None => return Ok(false),
            }
        };
        {
            let mut bucket = self.refresh_state.lock().await;
            let now = tokio::time::Instant::now();
            let elapsed = now.duration_since(bucket.1).as_secs_f64();
            bucket.0 = (bucket.0 + elapsed * 2.0).min(4.0);
            bucket.1 = now;
            if !force {
                if bucket.0 < 1.0 {
                    return Ok(false);
                }
                bucket.0 -= 1.0;
            }
        }

        let url = format!("{}/api/v1/relay/trusted-peers", srv.server_url);
        let resp = if let Some(ref tok) = srv.session_token.as_deref().filter(|s| !s.trim().is_empty()) {
            let first = daemon_http_client()
                .get(&url)
                .bearer_auth(tok)
                .send()
                .await?;
            // #46: if session_token expired (401/403), fall back automatically to X-Coda-Relay-Ticket
            if matches!(
                first.status(),
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
            ) && !srv.relay_ticket.trim().is_empty()
            {
                daemon_http_client()
                    .get(&url)
                    .header("X-Coda-Relay-Ticket", &srv.relay_ticket)
                    .send()
                    .await?
            } else {
                first
            }
        } else {
            daemon_http_client()
                .get(&url)
                .header("X-Coda-Relay-Ticket", &srv.relay_ticket)
                .send()
                .await?
        };
        let list = resp
            .error_for_status()
            .context("GET /api/v1/relay/trusted-peers rejected")?
            .json::<Vec<TrustedPeerEntry>>()
            .await?;
        self.replace_all(list).await;
        Ok(true)
    }

    pub async fn resolve_verified_peer(
        &self,
        device_id: Uuid,
        claimed_ed25519: &str,
        claimed_x25519: &str,
    ) -> Result<TrustedPeerEntry> {
        if let Some(existing) = self.get(&device_id).await {
            if existing.ed25519_pub_b64 == claimed_ed25519
                && existing.x25519_pub_b64 == claimed_x25519
            {
                return Ok(existing);
            }
        }

        let miss_recent = {
            let mut misses = self.miss_cache.lock().await;
            let now = tokio::time::Instant::now();
            misses.retain(|_, ts| now.duration_since(*ts) < std::time::Duration::from_secs(2));
            misses.contains_key(&device_id)
        };
        let has_server = self.server.read().await.is_some();
        let mut refreshed = false;
        if !miss_recent && has_server {
            refreshed = self.refresh_from_server(false).await.unwrap_or(false);
        }

        if let Some(found) = self.get(&device_id).await {
            if found.ed25519_pub_b64 != claimed_ed25519 {
                anyhow::bail!(
                    "untrusted Ed25519 key for device {device_id} (expected {}, got {claimed_ed25519})",
                    found.ed25519_pub_b64
                );
            }
            if found.x25519_pub_b64 != claimed_x25519 {
                anyhow::bail!(
                    "untrusted X25519 key for device {device_id} (expected {}, got {claimed_x25519})",
                    found.x25519_pub_b64
                );
            }
            return Ok(found);
        }

        if refreshed {
            let mut misses = self.miss_cache.lock().await;
            if misses.len() < 1024 {
                misses.insert(device_id, tokio::time::Instant::now());
            }
        }
        anyhow::bail!("peer device {device_id} is not in account TrustedPeersStore")
    }
}

/// Decodes the unverified `exp_unix` claim from a Coda relay ticket (`<b64_json>.<hmac>`).
pub fn parse_relay_ticket_exp_unix(ticket: &str) -> Option<i64> {
    let payload_b64 = ticket.trim().split('.').next()?;
    let bytes = B64
        .decode(payload_b64)
        .or_else(|_| B64_URL.decode(payload_b64))
        .ok()?;
    let val: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    val.get("exp_unix").and_then(|v| v.as_i64())
}

/// Returns true when the relay ticket is expired or expires within `lead_secs`.
pub fn ticket_needs_renewal(ticket: &str, lead_secs: i64) -> bool {
    let Some(exp) = parse_relay_ticket_exp_unix(ticket) else {
        return false;
    };
    let now = (current_unix_ms() / 1000) as i64;
    exp.saturating_sub(now) <= lead_secs
}

/// Renews a 30-day daemon relay ticket using the device's Ed25519 private key (`#28`).
pub async fn renew_daemon_relay_ticket(
    cfg: &DaemonConfig,
) -> Result<(String, Vec<DaemonRelayEntry>)> {
    let clean_server = cfg.server_url.trim().trim_end_matches('/');
    if clean_server.is_empty() {
        anyhow::bail!("missing server_url for ticket renewal");
    }
    let ts_ms = current_unix_ms();
    let msg = relay_ticket_renew_signing_bytes(cfg.device_id, ts_ms);
    let sig = cfg.identity.sign(&msg)?;
    let resp = daemon_http_client()
        .post(format!("{clean_server}/api/v1/relay/renew-ticket"))
        .json(&json!({
            "device_id": cfg.device_id,
            "timestamp_ms": ts_ms,
            "signature_b64": sig,
        }))
        .send()
        .await?
        .error_for_status()
        .context("POST /api/v1/relay/renew-ticket rejected")?
        .json::<serde_json::Value>()
        .await?;
    let ticket = resp["daemon_relay_ticket"]
        .as_str()
        .ok_or_else(|| anyhow!("missing daemon_relay_ticket in renewal response"))?
        .to_string();
    let relays = parse_relays_value(&resp["relays"]);
    Ok((ticket, relays))
}

/// Refreshes the daemon relay ticket on disk if within the renewal lead window (`7d` or `force`).
pub async fn maybe_renew_daemon_config(
    cfg: &mut DaemonConfig,
    cfg_path: Option<&Path>,
    peers_store: &TrustedPeersStore,
    force: bool,
) -> bool {
    if !force && !ticket_needs_renewal(&cfg.daemon_relay_ticket, TICKET_RENEWAL_LEAD_SECS) {
        return false;
    }
    match renew_daemon_relay_ticket(cfg).await {
        Ok((new_ticket, new_relays)) => {
            cfg.daemon_relay_ticket = new_ticket.clone();
            if !new_relays.is_empty() {
                cfg.relays = new_relays;
            }
            if let Some(p) = cfg_path {
                if let Err(e) = cfg.save(p) {
                    warn!("failed persisting renewed relay ticket to {}: {e}", p.display());
                }
            }
            peers_store.update_relay_ticket(new_ticket).await;
            info!("Successfully renewed 30-day daemon relay ticket for {}", cfg.device_id);
            true
        }
        Err(e) => {
            warn!("relay ticket renewal failed (will retry later): {e}");
            false
        }
    }
}

/// Binds a TCP listener supporting dual-stack IPv6 + IPv4 (`[::]:port` with `IPV6_V6ONLY = false`)
/// whenever an unspecified wildcard address (`0.0.0.0` or `::`) is requested (`#20`).
pub async fn bind_dual_stack_tcp_listener(bind_addr: SocketAddr) -> Result<TcpListener> {
    if bind_addr.ip().is_unspecified() {
        let try_dual = || -> Result<TcpListener> {
            let sock = socket2::Socket::new(
                socket2::Domain::IPV6,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            let _ = sock.set_reuse_address(true);
            sock.set_only_v6(false)?;
            let v6_addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), bind_addr.port());
            sock.bind(&v6_addr.into())?;
            sock.listen(128)?;
            sock.set_nonblocking(true)?;
            let std_listener: std::net::TcpListener = sock.into();
            Ok(TcpListener::from_std(std_listener)?)
        };
        if let Ok(listener) = try_dual() {
            return Ok(listener);
        }
    }
    Ok(TcpListener::bind(bind_addr).await?)
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
    pair_daemon_with_code_and_identity(server_url, code, custom_name, None).await
}

/// Claims a 6-digit pairing code while optionally reusing an existing persisted `StoredDeviceIdentity`.
pub async fn pair_daemon_with_code_and_identity(
    server_url: &str,
    code: &str,
    custom_name: Option<String>,
    existing_identity: Option<StoredDeviceIdentity>,
) -> Result<DaemonConfig> {
    let hostname = custom_name.unwrap_or_else(detect_hostname);
    let mut identity =
        existing_identity.unwrap_or_else(|| StoredDeviceIdentity::generate(&hostname));
    identity.name = hostname.clone();
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
    login_with_password_and_enroll_with_identity(
        server_url,
        email_or_handle,
        password,
        role,
        custom_name,
        None,
    )
    .await
}

/// Logs in with email/handle + password and enrolls/updates this device reusing `existing_identity` if given.
pub async fn login_with_password_and_enroll_with_identity(
    server_url: &str,
    email_or_handle: &str,
    password: &str,
    role: &str,
    custom_name: Option<String>,
    existing_identity: Option<StoredDeviceIdentity>,
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
    let mut identity =
        existing_identity.unwrap_or_else(|| StoredDeviceIdentity::generate(&hostname));
    identity.name = hostname.clone();
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
    let resp = daemon_http_client()
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

/// Revokes the active account session on `coda-server` (`POST /api/v1/auth/logout`).
pub async fn logout_account_session(server_url: &str, session_token: &str) -> Result<()> {
    let clean_server = server_url.trim().trim_end_matches('/');
    if clean_server.is_empty() || session_token.trim().is_empty() {
        return Ok(());
    }
    daemon_http_client()
        .post(format!("{clean_server}/api/v1/auth/logout"))
        .bearer_auth(session_token)
        .send()
        .await?
        .error_for_status()
        .context("logout failed")?;
    Ok(())
}

/// Renames an enrolled device under the active account (`PATCH /api/v1/devices/{id}`).
pub async fn rename_account_device(
    server_url: &str,
    session_token: &str,
    device_id: Uuid,
    new_name: &str,
) -> Result<()> {
    let clean_server = server_url.trim().trim_end_matches('/');
    daemon_http_client()
        .patch(format!("{clean_server}/api/v1/devices/{device_id}"))
        .bearer_auth(session_token)
        .json(&json!({ "name": new_name.trim() }))
        .send()
        .await?
        .error_for_status()
        .context("device rename failed")?;
    Ok(())
}

/// Deletes/revokes a device under the active account (`DELETE /api/v1/devices/{id}`).
pub async fn delete_account_device(
    server_url: &str,
    session_token: &str,
    device_id: Uuid,
) -> Result<()> {
    let clean_server = server_url.trim().trim_end_matches('/');
    daemon_http_client()
        .delete(format!("{clean_server}/api/v1/devices/{device_id}"))
        .bearer_auth(session_token)
        .send()
        .await?
        .error_for_status()
        .context("device delete failed")?;
    Ok(())
}

/// Updates the signed-in account's password (`POST /api/v1/auth/password`), returning the rotated token if issued.
pub async fn change_account_password(
    server_url: &str,
    session_token: &str,
    current_password: &str,
    new_password: &str,
) -> Result<Option<String>> {
    let clean_server = server_url.trim().trim_end_matches('/');
    let resp = daemon_http_client()
        .post(format!("{clean_server}/api/v1/auth/password"))
        .bearer_auth(session_token)
        .json(&json!({
            "current_password": current_password,
            "new_password": new_password,
        }))
        .send()
        .await?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        let msg = body["error"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        anyhow::bail!("{msg}");
    }
    Ok(body["token"].as_str().map(|s| s.to_string()))
}

/// Fetches all remote controllable hosts (`daemon` and `hybrid`) under this account.
pub async fn list_account_hosts(
    server_url: &str,
    session_token: &str,
) -> Result<Vec<RemoteHostSummary>> {
    let clean_server = server_url.trim().trim_end_matches('/');
    let raw = daemon_http_client()
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
            ed25519_pubkey: item["ed25519_pubkey"].as_str().unwrap_or("").to_string(),
            x25519_pubkey: item["x25519_pubkey"].as_str().unwrap_or("").to_string(),
            endpoints,
            is_online: item["is_online"].as_bool().unwrap_or(false),
        });
    }
    Ok(out)
}

/// Connects to `coda-server` `/api/v1/events/ws` for real-time same-account device & presence pushes.
/// Authenticates strictly via `Authorization: Bearer` header (never URL query parameter).
pub async fn subscribe_account_events_ws(
    server_url: &str,
    session_token: &str,
) -> Result<mpsc::Receiver<AccountLiveEvent>> {
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue};

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
    let url = format!("{ws_base}/api/v1/events/ws");
    let mut req = url
        .into_client_request()
        .context("invalid /api/v1/events/ws request URL")?;
    let hdr = HeaderValue::from_str(&format!("Bearer {}", session_token.trim()))
        .context("invalid session token header value")?;
    req.headers_mut().insert("Authorization", hdr);

    let (ws_stream, _) = connect_async_with_config(req, Some(bounded_ws_config()), false)
        .await
        .context("failed connecting to /api/v1/events/ws")?;
    let (mut sink, mut stream) = ws_stream.split();
    let (tx, rx) = mpsc::channel::<AccountLiveEvent>(128);

    tokio::spawn(async move {
        let mut ping_tick =
            tokio::time::interval(std::time::Duration::from_secs(WS_KEEPALIVE_SECS));
        ping_tick.tick().await;
        loop {
            tokio::select! {
                _ = ping_tick.tick() => {
                    if sink.send(WsMsg::Ping(Vec::new().into())).await.is_err()
                        || sink.flush().await.is_err()
                    {
                        break;
                    }
                }
                next_msg = stream.next() => {
                    let Some(Ok(msg)) = next_msg else {
                        break;
                    };
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
                            if sink.send(WsMsg::Pong(payload)).await.is_err()
                                || sink.flush().await.is_err()
                            {
                                break;
                            }
                        }
                        WsMsg::Pong(_) => {}
                        WsMsg::Close(_) => break,
                        _ => {}
                    }
                }
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
    peer_ed25519_pub_b64: &str,
    peer_x25519_pub_b64: &str,
) -> Result<SmartRelayFallbackConfig> {
    let clean_server = server_url.trim().trim_end_matches('/');
    let remote_server = clean_server.starts_with("https://")
        || !is_loopback_endpoint(clean_server.trim_start_matches("http://"));
    let resp = daemon_http_client()
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
        peer_ed25519_pub_b64: peer_ed25519_pub_b64.to_string(),
        peer_x25519_pub_b64: peer_x25519_pub_b64.to_string(),
    })
}

type SharedHerdr = Arc<RwLock<Option<Arc<HerdrClient>>>>;
type PeerSessionKey = (Uuid, String);

async fn connect_herdr_with_install() -> Option<Arc<HerdrClient>> {
    match HerdrClient::connect_default().await {
        Ok(hc) => Some(Arc::new(hc)),
        Err(first_err) => {
            // #9: Avoid silent unverified `curl | sh` at runtime unless explicitly opted in.
            let auto_install = std::env::var("CODA_AUTO_INSTALL_HC").as_deref() == Ok("1");
            if !auto_install {
                info!(
                    "herdr-core not installed/active ({first_err}); using built-in native PTY shell fallback (set CODA_AUTO_INSTALL_HC=1 to auto-install)"
                );
                return None;
            }
            warn!("herdr-core missing or inactive ({first_err}); attempting controlled auto-install");
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

async fn resolve_herdr_lazy(shared: &SharedHerdr) -> Option<Arc<HerdrClient>> {
    if let Some(existing) = shared.read().await.as_ref() {
        return Some(Arc::clone(existing));
    }
    // #22: Lazily retry connecting to locally installed `hc` instead of caching `None` forever.
    if let Ok(hc) = HerdrClient::connect_default().await {
        let arc = Arc::new(hc);
        let mut guard = shared.write().await;
        if guard.is_none() {
            info!(
                "Lazily bridged local herdr-core `{}` (v{})",
                arc.info().core_name,
                arc.info().version
            );
            *guard = Some(Arc::clone(&arc));
        }
        return guard.as_ref().map(Arc::clone);
    }
    None
}

// ============================================================================
// Native Unix `openpty` Fallback Shell Manager (#22 when `hc` is unavailable)
// ============================================================================

struct FallbackTerminalEntry {
    summary: HerdrTerminalSummary,
    pty: PtyBridgeHandle,
    output_tx: broadcast::Sender<PtyOutputEvent>,
    screen_tail: Arc<StdMutex<Vec<u8>>>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
struct NativeFallbackShells {
    terminals: HashMap<String, FallbackTerminalEntry>,
    seq: u64,
}

impl NativeFallbackShells {
    fn list(&self, include_finished: bool) -> Vec<HerdrTerminalSummary> {
        self.terminals
            .values()
            .filter_map(|e| {
                let is_alive = e.alive.load(Ordering::Relaxed);
                if !include_finished && !is_alive {
                    return None;
                }
                let mut s = e.summary.clone();
                s.alive = is_alive;
                Some(s)
            })
            .collect()
    }

    fn launch(
        &mut self,
        agent: &str,
        name: Option<&str>,
        cwd: Option<&str>,
        initial_prompt: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<HerdrTerminalSummary> {
        self.seq = self.seq.saturating_add(1);
        let clean_name = name
            .and_then(coda_herdr::sanitize_hc_terminal_name)
            .unwrap_or_else(|| format!("pty-{}", self.seq));
        let address = if self.terminals.contains_key(&clean_name) {
            format!("{clean_name}-{}", self.seq)
        } else {
            clean_name
        };

        let (pty_tx, mut pty_rx) = mpsc::channel::<PtyOutputEvent>(256);
        let (pty, argv, resolved_cwd) = spawn_native_shell_pty(
            &address,
            agent,
            cwd,
            initial_prompt,
            cols,
            rows,
            pty_tx,
        )?;
        let (output_tx, _) = broadcast::channel::<PtyOutputEvent>(256);
        let screen_tail = Arc::new(StdMutex::new(Vec::<u8>::with_capacity(8192)));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

        let out_tx_c = output_tx.clone();
        let tail_c = Arc::clone(&screen_tail);
        let alive_c = Arc::clone(&alive);
        tokio::spawn(async move {
            while let Some(ev) = pty_rx.recv().await {
                if let Ok(raw) = B64.decode(&ev.data_b64) {
                    if let Ok(mut buf) = tail_c.lock() {
                        buf.extend_from_slice(&raw);
                        if buf.len() > 32_768 {
                            let drop_n = buf.len() - 32_768;
                            buf.drain(..drop_n);
                        }
                    }
                }
                let _ = out_tx_c.send(ev);
            }
            alive_c.store(false, Ordering::Relaxed);
        });

        let summary = HerdrTerminalSummary {
            address: address.clone(),
            name: Some(address.clone()),
            argv,
            cwd: resolved_cwd,
            cols: cols.max(20),
            rows: rows.max(8),
            alive: true,
            status: None,
        };
        self.terminals.insert(
            address,
            FallbackTerminalEntry {
                summary: summary.clone(),
                pty,
                output_tx,
                screen_tail,
                alive,
            },
        );
        Ok(summary)
    }

    fn write_input(&self, terminal: &str, bytes: &[u8]) -> Result<()> {
        let entry = self
            .terminals
            .get(terminal)
            .ok_or_else(|| anyhow!("terminal `{terminal}` not found"))?;
        entry.pty.write_raw(bytes)
    }

    fn resize(&mut self, terminal: &str, cols: u16, rows: u16) -> Result<()> {
        let entry = self
            .terminals
            .get_mut(terminal)
            .ok_or_else(|| anyhow!("terminal `{terminal}` not found"))?;
        entry.pty.resize(cols, rows)?;
        entry.summary.cols = cols.max(20);
        entry.summary.rows = rows.max(8);
        Ok(())
    }

    fn attach(
        &mut self,
        terminal: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(Vec<u8>, broadcast::Receiver<PtyOutputEvent>)> {
        let entry = self
            .terminals
            .get_mut(terminal)
            .ok_or_else(|| anyhow!("terminal `{terminal}` not found"))?;
        let _ = entry.pty.resize(cols, rows);
        entry.summary.cols = cols.max(20);
        entry.summary.rows = rows.max(8);
        let snap = entry
            .screen_tail
            .lock()
            .map(|b| b.clone())
            .unwrap_or_default();
        Ok((snap, entry.output_tx.subscribe()))
    }

    fn read_screen(&self, terminal: &str) -> Result<String> {
        let entry = self
            .terminals
            .get(terminal)
            .ok_or_else(|| anyhow!("terminal `{terminal}` not found"))?;
        let snap = entry
            .screen_tail
            .lock()
            .map(|b| b.clone())
            .unwrap_or_default();
        Ok(String::from_utf8_lossy(&snap).to_string())
    }

    fn kill(&mut self, terminal: &str) -> bool {
        self.terminals.remove(terminal).is_some()
    }
}

enum ActivePtySubscription {
    Herdr(PtyBridgeHandle),
    Fallback(tokio::task::AbortHandle),
}

impl Drop for ActivePtySubscription {
    fn drop(&mut self) {
        if let Self::Fallback(h) = self {
            h.abort();
        }
    }
}

struct PeerCryptoSession {
    active_session_nonce: Option<String>,
    e2ee: E2eeSession,
    reply_tx: mpsc::Sender<RelayWireFrame>,
    created_at: tokio::time::Instant,
}

/// Seals and enqueues an outbound peer message while preserving exact monotonic E2EE sequence order.
async fn send_e2ee_peer_message(
    sessions: &Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>>,
    my_device_id: Uuid,
    session_key: &PeerSessionKey,
    msg: &PeerMessage,
    lane: TrafficLane,
) -> bool {
    let reply_tx = {
        let map = sessions.lock().await;
        match map.get(session_key) {
            Some(s) => s.reply_tx.clone(),
            None => return false,
        }
    };
    let Ok(permit) = reply_tx.reserve().await else {
        return false;
    };
    let mut map = sessions.lock().await;
    let Some(sess) = map.get_mut(session_key) else {
        return false;
    };
    let Ok(sealed) = sess.e2ee.seal_message(msg) else {
        return false;
    };
    permit.send(RelayWireFrame::RoutePacket {
        src_device_id: my_device_id,
        dst_device_id: session_key.0,
        lane,
        e2ee: sealed,
        sender_x25519_pub_b64: None,
        session_auth: None,
    });
    true
}

fn spawn_herdr_event_forwarder(
    herdr: SharedHerdr,
    sessions: Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>>,
    my_device_id: Uuid,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut seq: u64 = 0;
        let mut backoff_secs = 1u64;
        loop {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            let Some(hc) = resolve_herdr_lazy(&herdr).await else {
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(15);
                continue;
            };
            let Ok(mut ev_rx) = hc.subscribe_live_events().await else {
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(15);
                continue;
            };
            backoff_secs = 1;
            while let Some(raw_event) = ev_rx.recv().await {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
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
                let peer_keys: Vec<PeerSessionKey> = {
                    let map = sessions.lock().await;
                    map.keys().cloned().collect()
                };
                if peer_keys.is_empty() {
                    continue;
                }
                let msg = PeerMessage::HerdrLiveEvent {
                    seq,
                    event_kind,
                    terminal,
                    raw_event,
                };
                for key in peer_keys {
                    let _ = send_e2ee_peer_message(
                        &sessions,
                        my_device_id,
                        &key,
                        &msg,
                        TrafficLane::ControlP0,
                    )
                    .await;
                }
            }
            // #25: `hc connect` event subscriber exited; reconnect with backoff instead of permanently stopping.
            tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(15);
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

/// Spawns a background synchronizer keeping `TrustedPeersStore` updated and evicting revoked peers.
fn spawn_trusted_peers_sync_task(
    cfg: &DaemonConfig,
    peers_store: Arc<TrustedPeersStore>,
    sessions: Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>>,
    active_ptys: Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
) -> tokio::task::AbortHandle {
    let server_url = cfg.server_url.clone();
    let session_token = cfg.session_token.clone();
    let handle = tokio::spawn(async move {
        let prune_revoked = || async {
            let live_keys: Vec<PeerSessionKey> = {
                let map = sessions.lock().await;
                map.keys().cloned().collect()
            };
            for key in live_keys {
                if !peers_store.contains(&key.0).await {
                    sessions.lock().await.remove(&key);
                    active_ptys
                        .lock()
                        .await
                        .retain(|((dev, _), _), _| *dev != key.0);
                }
            }
        };

        let mut ws_rx = match session_token.as_deref() {
            Some(tok) if !tok.trim().is_empty() => {
                subscribe_account_events_ws(&server_url, tok).await.ok()
            }
            _ => None,
        };
        let mut periodic = tokio::time::interval(std::time::Duration::from_secs(30));
        periodic.tick().await;
        loop {
            tokio::select! {
                _ = periodic.tick() => {
                    if peers_store.refresh_from_server(true).await.is_ok() {
                        prune_revoked().await;
                    }
                    if ws_rx.is_none() {
                        if let Some(tok) = session_token.as_deref().filter(|s| !s.trim().is_empty()) {
                            ws_rx = subscribe_account_events_ws(&server_url, tok).await.ok();
                        }
                    }
                }
                maybe_ev = async {
                    match ws_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending::<Option<AccountLiveEvent>>().await,
                    }
                } => {
                    match maybe_ev {
                        Some(ev) => {
                            if ev.kind != "connected" && peers_store.refresh_from_server(true).await.is_ok() {
                                prune_revoked().await;
                            }
                        }
                        None => {
                            ws_rx = None;
                        }
                    }
                }
            }
        }
    });
    handle.abort_handle()
}

/// Dispatches an authenticated/decrypted `PeerMessage`, isolating slow RPCs from the control loop.
#[allow(clippy::too_many_arguments)]
async fn dispatch_decrypted_peer_message(
    session_key: PeerSessionKey,
    req_msg: PeerMessage,
    herdr: &SharedHerdr,
    fallback_shells: &Arc<Mutex<NativeFallbackShells>>,
    hostname: &str,
    active_ptys: &Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
    sessions: &Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>>,
    my_device_id: Uuid,
    rpc_semaphore: &Arc<Semaphore>,
) {
    if matches!(
        req_msg,
        PeerMessage::PtyInput { .. } | PeerMessage::PtyResize { .. }
    ) {
        let _ = handle_peer_request_full(
            session_key,
            req_msg,
            herdr,
            fallback_shells,
            hostname,
            active_ptys,
            sessions,
            my_device_id,
        )
        .await;
        return;
    }

    let req_id_fallback = req_msg.request_id().unwrap_or("busy").to_string();
    let Ok(permit) = Arc::clone(rpc_semaphore).try_acquire_owned() else {
        let _ = send_e2ee_peer_message(
            sessions,
            my_device_id,
            &session_key,
            &PeerMessage::ActionAck {
                req_id: req_id_fallback,
                ok: false,
                detail: "daemon busy: concurrent RPC limit reached".into(),
            },
            TrafficLane::ControlP0,
        )
        .await;
        return;
    };

    let herdr_c = Arc::clone(herdr);
    let fallback_c = Arc::clone(fallback_shells);
    let hostname_c = hostname.to_string();
    let active_ptys_c = Arc::clone(active_ptys);
    let sessions_c = Arc::clone(sessions);
    tokio::spawn(async move {
        let _permit = permit;
        if let Some((reply_msg, reply_lane)) = handle_peer_request_full(
            session_key.clone(),
            req_msg,
            &herdr_c,
            &fallback_c,
            &hostname_c,
            &active_ptys_c,
            &sessions_c,
            my_device_id,
        )
        .await
        {
            let _ = send_e2ee_peer_message(
                &sessions_c,
                my_device_id,
                &session_key,
                &reply_msg,
                reply_lane,
            )
            .await;
        }
    });
}

enum RelayOpenError {
    SessionResetRequired { nonce: Option<String>, reason: String },
    Rejected(anyhow::Error),
}

#[allow(clippy::too_many_arguments)]
async fn open_and_verify_relay_route_packet(
    src_device_id: Uuid,
    dst_device_id: Uuid,
    my_device_id: Uuid,
    my_identity: &StoredDeviceIdentity,
    e2ee: &coda_protocol::EncryptedPayload,
    session_auth: &Option<RelaySessionAuth>,
    reply_tx: &mpsc::Sender<RelayWireFrame>,
    peers_store: &TrustedPeersStore,
    sessions: &Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>>,
    used_nonces: &Arc<Mutex<TimedReplayCache>>,
    active_ptys: &Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
) -> std::result::Result<(PeerSessionKey, PeerMessage), RelayOpenError> {
    if dst_device_id != my_device_id {
        return Err(RelayOpenError::Rejected(anyhow!(
            "relay packet destination mismatch ({dst_device_id} != {my_device_id})"
        )));
    }

    let pkt_nonce = e2ee.session_nonce_b64.clone().unwrap_or_default();
    let session_key = (src_device_id, pkt_nonce.clone());

    // Fast path: packet belongs to an already-authenticated active Relay v2 session nonce (#19).
    if !pkt_nonce.is_empty() {
        let mut map = sessions.lock().await;
        if let Some(existing) = map.get_mut(&session_key) {
            let matches_active = existing.active_session_nonce.as_deref()
                == Some(pkt_nonce.as_str())
                && match session_auth {
                    Some(auth) => auth.session_nonce_b64 == pkt_nonce,
                    None => true,
                };
            if matches_active {
                existing.reply_tx = reply_tx.clone();
                return match existing.e2ee.open_message(e2ee) {
                    Ok(m) => Ok((session_key, m)),
                    Err(e) => Err(RelayOpenError::Rejected(e)),
                };
            }
        }
    }

    // New or replaced Relay session: strictly require signed RelaySessionAuth v2 (zero seq==1 bypass).
    let Some(auth) = session_auth.as_ref() else {
        // #14: Peer sent encrypted packet for an unknown/expired session (e.g. daemon restarted).
        // Signal structured SessionReset so Web/Apple/Desktop immediately re-handshakes & re-attaches.
        return Err(RelayOpenError::SessionResetRequired {
            nonce: e2ee.session_nonce_b64.clone(),
            reason: "session_reset_required".into(),
        });
    };
    if e2ee.seq > 64 {
        return Err(RelayOpenError::SessionResetRequired {
            nonce: e2ee.session_nonce_b64.clone(),
            reason: "session_reset_required".into(),
        });
    }
    if e2ee.session_nonce_b64.as_deref() != Some(auth.session_nonce_b64.as_str()) {
        return Err(RelayOpenError::Rejected(anyhow!(
            "encrypted payload session_nonce_b64 mismatch"
        )));
    }

    let peer_entry = peers_store
        .resolve_verified_peer(
            src_device_id,
            &auth.sender_ed25519_pub_b64,
            &auth.sender_x25519_pub_b64,
        )
        .await
        .map_err(RelayOpenError::Rejected)?;

    // #1: Pure `daemon` hosts must never initiate remote-control sessions against another host.
    if peer_entry.role == "daemon" {
        return Err(RelayOpenError::Rejected(anyhow!(
            "daemon-role peer {src_device_id} is not permitted to initiate remote control"
        )));
    }

    let now_ms = current_unix_ms();
    auth.verify(
        src_device_id,
        my_device_id,
        &peer_entry.ed25519_pub_b64,
        &peer_entry.x25519_pub_b64,
        now_ms,
        HANDSHAKE_MAX_SKEW_MS,
    )
    .map_err(RelayOpenError::Rejected)?;

    let replay_key = format!("relay:{src_device_id}:{}", auth.session_nonce_b64);
    let replay_ttl = std::time::Duration::from_millis(HANDSHAKE_MAX_SKEW_MS * 2);
    if used_nonces.lock().await.contains_fresh(&replay_key, replay_ttl) {
        return Err(RelayOpenError::Rejected(anyhow!(
            "replayed RelaySessionAuth nonce from {src_device_id}"
        )));
    }

    let my_dh_secret = my_identity
        .x25519_secret()
        .map_err(RelayOpenError::Rejected)?;
    let mut fresh = E2eeSession::derive_relay_session(
        &my_dh_secret,
        &peer_entry.x25519_pub_b64,
        &auth.session_nonce_b64,
        src_device_id,
        my_device_id,
        false,
        auth.supports_zstd,
    )
    .map_err(RelayOpenError::Rejected)?;

    let opened = fresh
        .open_message(e2ee)
        .map_err(RelayOpenError::Rejected)?;

    // Commit anti-replay state and insert session ONLY AFTER AEAD verification succeeds.
    used_nonces.lock().await.insert(replay_key, replay_ttl);
    let evicted_keys: Vec<PeerSessionKey> = {
        let mut map = sessions.lock().await;
        map.insert(
            session_key.clone(),
            PeerCryptoSession {
                active_session_nonce: Some(auth.session_nonce_b64.clone()),
                e2ee: fresh,
                reply_tx: reply_tx.clone(),
                created_at: tokio::time::Instant::now(),
            },
        );
        // Cap concurrent sessions per peer device (#19 multi-tab isolation without unbounded memory).
        let mut peer_entries: Vec<(PeerSessionKey, tokio::time::Instant)> = map
            .iter()
            .filter(|((dev, _), _)| *dev == src_device_id)
            .map(|(k, v)| (k.clone(), v.created_at))
            .collect();
        let mut evicted = Vec::new();
        if peer_entries.len() > MAX_SESSIONS_PER_PEER {
            peer_entries.sort_by_key(|(_, ts)| *ts);
            let excess = peer_entries.len() - MAX_SESSIONS_PER_PEER;
            for (old_k, _) in peer_entries.into_iter().take(excess) {
                if old_k != session_key {
                    map.remove(&old_k);
                    evicted.push(old_k);
                }
            }
        }
        evicted
    };
    if !evicted_keys.is_empty() {
        let mut ptys = active_ptys.lock().await;
        for ek in evicted_keys {
            ptys.retain(|(sk, _), _| sk != &ek);
        }
    }

    Ok((session_key, opened))
}

fn extract_relay_host(entry: &DaemonRelayEntry) -> String {
    let ep = entry.endpoint.trim();
    let without_scheme = ep
        .strip_prefix("wss://")
        .or_else(|| ep.strip_prefix("ws://"))
        .or_else(|| ep.strip_prefix("https://"))
        .or_else(|| ep.strip_prefix("http://"))
        .unwrap_or(ep);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .unwrap_or(authority)
            .trim_start_matches('[')
            .to_ascii_lowercase()
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
            .to_ascii_lowercase()
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "0.0.0.0")
}

/// Shared host execution state across Relay and Direct P2P listeners so fallback shells,
/// `herdr-core` handles, active PTY subscriptions (for cross-transport single-owner eviction),
/// and immediate shutdown signals stay consistent across reconnects and transport hot-switches.
#[derive(Clone)]
pub struct SharedHostRuntime {
    herdr: SharedHerdr,
    fallback_shells: Arc<Mutex<NativeFallbackShells>>,
    active_ptys: Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
    shutdown_tx: watch::Sender<bool>,
}

impl SharedHostRuntime {
    pub async fn new() -> Self {
        let herdr: SharedHerdr = Arc::new(RwLock::new(connect_herdr_with_install().await));
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            herdr,
            fallback_shells: Arc::new(Mutex::new(NativeFallbackShells::default())),
            active_ptys: Arc::new(Mutex::new(HashMap::new())),
            shutdown_tx,
        }
    }

    pub fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub fn trigger_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Connects a single relay leg (`raw_tcp_no_sni` or `cdn_wss`) and spawns its reader/writer tasks.
async fn connect_single_relay_leg(
    chosen: &DaemonRelayEntry,
    ticket: &str,
    in_tx: mpsc::Sender<(mpsc::Sender<RelayWireFrame>, RelayWireFrame)>,
    leg_dead_tx: mpsc::Sender<String>,
    task_guard: &mut TaskAbortGuard,
) -> Result<mpsc::Sender<RelayWireFrame>> {
    let bind_frame = RelayWireFrame::AuthBind {
        ticket: ticket.to_string(),
        lane: TrafficLane::ControlP0,
        generation: next_monotonic_generation(),
    };
    let (out_tx, mut out_rx) = mpsc::channel::<RelayWireFrame>(256);
    let leg_label = format!("{} [{}]", chosen.endpoint, chosen.mode);
    let read_idle_timeout = std::time::Duration::from_secs(WS_KEEPALIVE_SECS * 3);

    if chosen.mode == "raw_tcp_no_sni" {
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            TcpStream::connect(&chosen.endpoint),
        )
        .await
        .map_err(|_| anyhow!("raw TCP relay {} timed out", chosen.endpoint))?
        .with_context(|| format!("raw TCP relay {} connect failed", chosen.endpoint))?;
        let _ = stream.set_nodelay(true);
        let (mut reader, mut writer) = stream.into_split();
        writer
            .write_all(&bind_frame.encode_length_delimited()?)
            .await?;

        let ack = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_raw_frame_limited(&mut reader, MAX_AUTH_FRAME_BYTES),
        )
        .await
        .map_err(|_| anyhow!("raw TCP relay timed out waiting for AuthAck"))??;
        match ack {
            RelayWireFrame::AuthAck { ok: true, .. } => {}
            other => anyhow::bail!("unexpected raw TCP relay auth response: {other:?}"),
        }

        let out_tx_for_in = out_tx.clone();
        let dead_w = leg_dead_tx.clone();
        let label_w = leg_label.clone();
        let tcp_writer = tokio::spawn(async move {
            let mut keepalive =
                tokio::time::interval(std::time::Duration::from_secs(WS_KEEPALIVE_SECS));
            keepalive.tick().await;
            loop {
                tokio::select! {
                    maybe_f = out_rx.recv() => {
                        let Some(f) = maybe_f else { break; };
                        if let Ok(bytes) = f.encode_length_delimited() {
                            if writer.write_all(&bytes).await.is_err() {
                                break;
                            }
                        }
                    }
                    _ = keepalive.tick() => {
                        let ka = RelayWireFrame::KeepAlive { ts_ms: current_unix_ms() };
                        if let Ok(bytes) = ka.encode_length_delimited() {
                            if writer.write_all(&bytes).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
            let _ = dead_w.try_send(format!("{label_w} writer closed"));
        });
        task_guard.0.push(tcp_writer.abort_handle());

        let dead_r = leg_dead_tx;
        let label_r = leg_label;
        let tcp_reader = tokio::spawn(async move {
            loop {
                match tokio::time::timeout(read_idle_timeout, read_raw_frame(&mut reader)).await {
                    Ok(Ok(f)) => {
                        if in_tx.send((out_tx_for_in.clone(), f)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Err(_)) | Err(_) => break,
                }
            }
            let _ = dead_r.try_send(format!("{label_r} reader closed"));
        });
        task_guard.0.push(tcp_reader.abort_handle());
        Ok(out_tx)
    } else {
        let (ws_stream, _) = tokio::time::timeout(
            std::time::Duration::from_secs(6),
            connect_async_with_config(&chosen.endpoint, Some(bounded_ws_config()), false),
        )
        .await
        .map_err(|_| anyhow!("WSS relay {} timed out", chosen.endpoint))?
        .with_context(|| format!("WSS relay {} connect failed", chosen.endpoint))?;
        let (mut sink, mut stream) = ws_stream.split();
        sink.send(WsMsg::Text(serde_json::to_string(&bind_frame)?.into()))
            .await?;
        sink.flush().await?;

        if let Ok(Some(Ok(WsMsg::Text(txt)))) =
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()).await
        {
            let ack: RelayWireFrame = serde_json::from_str(&txt)?;
            if !matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }) {
                anyhow::bail!("unexpected WSS relay auth response: {ack:?}");
            }
        } else {
            anyhow::bail!("WSS relay closed or timed out before AuthAck");
        }

        let out_tx_for_in = out_tx.clone();
        let (pong_tx, mut pong_rx) = mpsc::channel::<Vec<u8>>(16);
        let dead_w = leg_dead_tx.clone();
        let label_w = leg_label.clone();
        let wss_writer = tokio::spawn(async move {
            let mut keepalive =
                tokio::time::interval(std::time::Duration::from_secs(WS_KEEPALIVE_SECS));
            keepalive.tick().await;
            loop {
                tokio::select! {
                    Some(payload) = pong_rx.recv() => {
                        if sink.send(WsMsg::Pong(payload.into())).await.is_err()
                            || sink.flush().await.is_err()
                        {
                            break;
                        }
                    }
                    maybe_f = out_rx.recv() => {
                        let Some(f) = maybe_f else { break; };
                        if let Ok(txt) = serde_json::to_string(&f) {
                            if sink.send(WsMsg::Text(txt.into())).await.is_err()
                                || sink.flush().await.is_err()
                            {
                                break;
                            }
                        }
                    }
                    _ = keepalive.tick() => {
                        if sink.send(WsMsg::Ping(Vec::new().into())).await.is_err()
                            || sink.flush().await.is_err()
                        {
                            break;
                        }
                    }
                }
            }
            let _ = dead_w.try_send(format!("{label_w} writer closed"));
        });
        task_guard.0.push(wss_writer.abort_handle());

        let dead_r = leg_dead_tx;
        let label_r = leg_label;
        let wss_reader = tokio::spawn(async move {
            loop {
                match tokio::time::timeout(read_idle_timeout, stream.next()).await {
                    Ok(Some(Ok(msg))) => match msg {
                        WsMsg::Text(t) => {
                            if let Ok(f) = serde_json::from_str::<RelayWireFrame>(&t) {
                                if in_tx.send((out_tx_for_in.clone(), f)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        WsMsg::Binary(b) => {
                            if let Ok(f) = serde_json::from_slice::<RelayWireFrame>(&b) {
                                if in_tx.send((out_tx_for_in.clone(), f)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        WsMsg::Ping(p) => {
                            let _ = pong_tx.try_send(p.to_vec());
                        }
                        WsMsg::Pong(_) => {}
                        WsMsg::Close(_) => break,
                        _ => {}
                    },
                    _ => break,
                }
            }
            let _ = dead_r.try_send(format!("{label_r} reader closed"));
        });
        task_guard.0.push(wss_reader.abort_handle());
        Ok(out_tx)
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
    let peers_store = TrustedPeersStore::from_daemon_config(cfg, None);
    let _ = peers_store.refresh_from_server(true).await;
    run_daemon_once_with_p2p_and_peers(cfg, prefer_mode, p2p_port, peers_store).await
}

/// Runs a connected Daemon Relay session using a shared `TrustedPeersStore`.
///
/// #27 Multi-Relay Support: When a dedicated domestic `--relay-only` (`raw_tcp_no_sni`) node
/// is configured on a distinct host from the coordinator (`cdn_wss`), connects to BOTH
/// concurrently sharing one session/PTY state so `devices.is_online` stays accurate, Web
/// Console stays reachable, and mobile/desktop clients route via low-latency domestic TCP!
pub async fn run_daemon_once_with_p2p_and_peers(
    cfg: &DaemonConfig,
    prefer_mode: Option<&str>,
    p2p_port: Option<u16>,
    peers_store: Arc<TrustedPeersStore>,
) -> Result<()> {
    let runtime = SharedHostRuntime::new().await;
    run_daemon_once_with_runtime(cfg, prefer_mode, p2p_port, peers_store, runtime).await
}

/// Runs a connected Daemon Relay session using a caller-owned `SharedHostRuntime`,
/// preserving fallback shells and cross-transport PTY ownership across reconnects and P2P hot-switches.
pub async fn run_daemon_once_with_runtime(
    cfg: &DaemonConfig,
    prefer_mode: Option<&str>,
    p2p_port: Option<u16>,
    peers_store: Arc<TrustedPeersStore>,
    runtime: SharedHostRuntime,
) -> Result<()> {
    // Reap any leftover non-P2P PTY subscriptions if a prior relay session was cancelled (e.g. SIGHUP).
    runtime
        .active_ptys
        .lock()
        .await
        .retain(|((_, nonce), _), _| nonce.starts_with("p2p-"));
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

    let _ = peers_store.refresh_from_server(true).await;

    let (in_tx, mut in_rx) = mpsc::channel::<(mpsc::Sender<RelayWireFrame>, RelayWireFrame)>(256);
    let (leg_dead_tx, mut leg_dead_rx) = mpsc::channel::<String>(8);
    let mut _task_guard = TaskAbortGuard(Vec::new());
    let mut active_out_txs: Vec<mpsc::Sender<RelayWireFrame>> = Vec::new();
    let mut connected_hosts: Vec<String> = Vec::new();
    let mut last_err: Option<anyhow::Error> = None;

    for chosen in &candidates {
        let host = extract_relay_host(chosen);
        // Avoid opening two ControlP0 sockets to the exact same RelayHub (which would supersede slot 1).
        if !active_out_txs.is_empty() {
            if prefer_mode.is_some()
                || connected_hosts.iter().any(|h| h == &host || (is_loopback_host(h) && is_loopback_host(&host)))
            {
                continue;
            }
        }
        info!(
            "Connecting Coda Daemon `{}` ({}) via {} [{}]",
            cfg.identity.name, cfg.device_id, chosen.endpoint, chosen.mode
        );
        match connect_single_relay_leg(
            chosen,
            &cfg.daemon_relay_ticket,
            in_tx.clone(),
            leg_dead_tx.clone(),
            &mut _task_guard,
        )
        .await
        {
            Ok(out_tx) => {
                active_out_txs.push(out_tx);
                connected_hosts.push(host);
                // Connect at most one primary raw_tcp + one distinct coordinator cdn_wss concurrently.
                if prefer_mode.is_some() || active_out_txs.len() >= 2 {
                    break;
                }
            }
            Err(e) => {
                warn!("relay {} ({}) failed ({e}); trying next relay", chosen.endpoint, chosen.mode);
                last_err = Some(e);
            }
        }
    }

    drop(in_tx);
    drop(leg_dead_tx);
    if active_out_txs.is_empty() {
        return Err(last_err.unwrap_or_else(|| anyhow!("all relay endpoints failed")));
    }

    let endpoint_watcher = if let Some(port) = p2p_port {
        let initial_eps = discover_local_endpoints_with_port(port);
        for tx in &active_out_txs {
            let _ = tx
                .send(RelayWireFrame::AdvertiseEndpoints {
                    endpoints: initial_eps.clone(),
                })
                .await;
        }
        let watch_txs = active_out_txs.clone();
        let watcher = tokio::spawn(async move {
            let mut last_eps = initial_eps;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            interval.tick().await;
            loop {
                interval.tick().await;
                let fresh = discover_local_endpoints_with_port(port);
                if fresh != last_eps {
                    last_eps = fresh.clone();
                    let mut any_open = false;
                    for tx in &watch_txs {
                        if tx
                            .send(RelayWireFrame::AdvertiseEndpoints {
                                endpoints: fresh.clone(),
                            })
                            .await
                            .is_ok()
                        {
                            any_open = true;
                        }
                    }
                    if !any_open {
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

    let herdr: SharedHerdr = Arc::clone(&runtime.herdr);
    if let Some(ref hc) = *herdr.read().await {
        info!(
            "Bridged local herdr-core `{}` (v{})",
            hc.info().core_name,
            hc.info().version
        );
    } else {
        info!("herdr-core not active yet; native Unix openpty fallback shell manager ready");
    }

    let fallback_shells = Arc::clone(&runtime.fallback_shells);
    let sessions: Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let active_ptys = Arc::clone(&runtime.active_ptys);
    let used_relay_nonces: Arc<Mutex<TimedReplayCache>> =
        Arc::new(Mutex::new(TimedReplayCache::default()));
    let rpc_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PEER_RPCS));
    let _ = cfg.identity.x25519_secret()?;
    let my_device_id = cfg.device_id;
    let hostname = cfg.identity.name.clone();

    let peers_sync_abort = spawn_trusted_peers_sync_task(
        cfg,
        Arc::clone(&peers_store),
        Arc::clone(&sessions),
        Arc::clone(&active_ptys),
    );
    _task_guard.0.push(peers_sync_abort);

    let stop_events = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_forwarder = spawn_herdr_event_forwarder(
        Arc::clone(&herdr),
        Arc::clone(&sessions),
        my_device_id,
        Arc::clone(&stop_events),
    );
    _task_guard.0.push(event_forwarder.abort_handle());
    let mut shutdown_rx = runtime.shutdown_rx();

    loop {
        if *shutdown_rx.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Daemon relay loop stopping on host shutdown signal");
                    break;
                }
            }
            dead = leg_dead_rx.recv() => {
                if let Some(reason) = dead {
                    warn!("relay leg disconnected ({reason}); recycling session to restore all legs");
                }
                break;
            }
            maybe_in = in_rx.recv() => {
                let Some((reply_tx, frame)) = maybe_in else {
                    break;
                };
                match frame {
                    RelayWireFrame::KeepAlive { .. } => {}
                    RelayWireFrame::PeerPresence { device_id, is_online: false, .. } => {
                        sessions.lock().await.retain(|(dev, _), _| *dev != device_id);
                        active_ptys
                            .lock()
                            .await
                            .retain(|((dev, nonce), _), _| !(*dev == device_id && !nonce.starts_with("p2p-")));
                    }
                    RelayWireFrame::RoutePacket {
                        src_device_id,
                        dst_device_id,
                        e2ee,
                        session_auth,
                        ..
                    } => {
                        let (session_key, req_msg) = match open_and_verify_relay_route_packet(
                            src_device_id,
                            dst_device_id,
                            my_device_id,
                            &cfg.identity,
                            &e2ee,
                            &session_auth,
                            &reply_tx,
                            &peers_store,
                            &sessions,
                            &used_relay_nonces,
                            &active_ptys,
                        )
                        .await
                        {
                            Ok(m) => m,
                            Err(RelayOpenError::SessionResetRequired { nonce, reason }) => {
                                warn!(
                                    "unknown/expired relay session from {src_device_id}; requesting SessionReset"
                                );
                                let _ = reply_tx
                                    .try_send(RelayWireFrame::SessionReset {
                                        src_device_id: my_device_id,
                                        dst_device_id: src_device_id,
                                        session_nonce_b64: nonce,
                                        reason,
                                    });
                                continue;
                            }
                            Err(RelayOpenError::Rejected(e)) => {
                                warn!("rejected relay packet from {src_device_id}: {e}");
                                continue;
                            }
                        };

                        dispatch_decrypted_peer_message(
                            session_key,
                            req_msg,
                            &herdr,
                            &fallback_shells,
                            &hostname,
                            &active_ptys,
                            &sessions,
                            my_device_id,
                            &rpc_semaphore,
                        )
                        .await;
                    }
                    _ => {}
                }
            }
        }
    }

    stop_events.store(true, Ordering::Relaxed);
    event_forwarder.abort();
    // Prune ONLY Relay PTY subscriptions; preserve live Direct P2P subscriptions across Relay reconnects.
    active_ptys
        .lock()
        .await
        .retain(|((_, nonce), _), _| nonce.starts_with("p2p-"));
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
    let peers_store = TrustedPeersStore::from_daemon_config(&cfg, None);
    let _ = peers_store.refresh_from_server(true).await;
    spawn_direct_p2p_listener_with_peers(cfg, bind_addr, peers_store).await
}

/// Spawns Direct P2P TCP/WS listener sharing a caller-provided `TrustedPeersStore`.
pub async fn spawn_direct_p2p_listener_with_peers(
    cfg: DaemonConfig,
    bind_addr: SocketAddr,
    peers_store: Arc<TrustedPeersStore>,
) -> Result<SocketAddr> {
    let runtime = SharedHostRuntime::new().await;
    spawn_direct_p2p_listener_with_runtime(cfg, bind_addr, peers_store, runtime)
        .await
        .map(|(addr, _)| addr)
}

/// Spawns Direct P2P TCP/WS listener sharing a caller-provided `SharedHostRuntime`.
pub async fn spawn_direct_p2p_listener_with_runtime(
    cfg: DaemonConfig,
    bind_addr: SocketAddr,
    peers_store: Arc<TrustedPeersStore>,
    runtime: SharedHostRuntime,
) -> Result<(SocketAddr, tokio::task::AbortHandle)> {
    // #20: Dual-stack `[::]:24444` (`IPV6_V6ONLY = false`) with automatic `0.0.0.0:24444` fallback.
    let listener = bind_dual_stack_tcp_listener(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    info!(
        "Direct P2P zero-relay listener active on {} (dual-stack RawTCP + LAN WebSocket, device={})",
        local_addr, cfg.device_id
    );

    let herdr = Arc::clone(&runtime.herdr);
    let fallback_shells = Arc::clone(&runtime.fallback_shells);
    let active_ptys = Arc::clone(&runtime.active_ptys);
    let p2p_replay = Arc::new(Mutex::new(TimedReplayCache::default()));
    let conn_sem = Arc::new(Semaphore::new(MAX_DIRECT_P2P_CONNS));
    let mut accept_shutdown_rx = runtime.shutdown_rx();

    let handle = tokio::spawn(async move {
        loop {
            if *accept_shutdown_rx.borrow() {
                break;
            }
            let accept_res = tokio::select! {
                _ = accept_shutdown_rx.changed() => {
                    if *accept_shutdown_rx.borrow() {
                        break;
                    }
                    continue;
                }
                res = listener.accept() => res,
            };
            let Ok((stream, peer_addr)) = accept_res else {
                break;
            };
            let Ok(permit) = Arc::clone(&conn_sem).try_acquire_owned() else {
                warn!("direct P2P concurrency cap reached; dropping {peer_addr}");
                continue;
            };
            let cfg_clone = cfg.clone();
            let herdr_clone = Arc::clone(&herdr);
            let fallback_clone = Arc::clone(&fallback_shells);
            let active_ptys_clone = Arc::clone(&active_ptys);
            let peers_clone = Arc::clone(&peers_store);
            let replay_clone = Arc::clone(&p2p_replay);
            let conn_shutdown_rx = runtime.shutdown_rx();
            tokio::spawn(async move {
                let _conn_permit = permit;
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
                    handle_direct_p2p_ws_socket(
                        stream,
                        peer_addr,
                        cfg_clone,
                        herdr_clone,
                        fallback_clone,
                        active_ptys_clone,
                        peers_clone,
                        replay_clone,
                        conn_shutdown_rx,
                    )
                    .await
                } else {
                    handle_direct_p2p_socket(
                        stream,
                        peer_addr,
                        cfg_clone,
                        herdr_clone,
                        fallback_clone,
                        active_ptys_clone,
                        peers_clone,
                        replay_clone,
                        conn_shutdown_rx,
                    )
                    .await
                };
                if let Err(e) = res {
                    warn!("direct P2P session from {peer_addr} ended: {e}");
                }
            });
        }
    });

    Ok((local_addr, handle.abort_handle()))
}

struct VerifiedP2pHello {
    client_device_id: Uuid,
    client_static_x25519_pub_b64: String,
    client_ephemeral_x25519_pub_b64: String,
    supports_zstd: bool,
}

async fn verify_direct_p2p_hello(
    cfg: &DaemonConfig,
    peers_store: &TrustedPeersStore,
    replay_cache: &Arc<Mutex<TimedReplayCache>>,
    hello: RelayWireFrame,
) -> Result<VerifiedP2pHello> {
    match hello {
        RelayWireFrame::DirectP2pHello {
            client_device_id,
            target_device_id,
            client_ed25519_pub_b64,
            client_x25519_pub_b64,
            client_ephemeral_x25519_pub_b64,
            timestamp_ms,
            signature_b64,
            supports_zstd,
        } => {
            if target_device_id != cfg.device_id {
                anyhow::bail!("P2P target device mismatch");
            }
            let now_ms = current_unix_ms();
            if !is_timestamp_fresh(timestamp_ms, now_ms, HANDSHAKE_MAX_SKEW_MS) {
                anyhow::bail!("P2P handshake timestamp expired or skewed");
            }
            validate_ed25519_pub_b64(&client_ed25519_pub_b64)?;
            validate_x25519_pub_b64(&client_x25519_pub_b64)?;
            if client_ephemeral_x25519_pub_b64.trim().is_empty() {
                anyhow::bail!("P2P v2 requires non-empty client_ephemeral_x25519_pub_b64");
            }
            validate_x25519_pub_b64(&client_ephemeral_x25519_pub_b64)?;

            let peer_entry = peers_store
                .resolve_verified_peer(
                    client_device_id,
                    &client_ed25519_pub_b64,
                    &client_x25519_pub_b64,
                )
                .await?;

            // #1: Pure `daemon` hosts must never initiate remote-control sessions over Direct P2P.
            if peer_entry.role == "daemon" {
                anyhow::bail!("daemon-role peer {client_device_id} cannot initiate P2P control");
            }

            // #11: Verify signature with `supports_zstd` bound (fallback to v0.4.0 legacy signing bytes).
            let signed_v2_zstd = p2p_hello_v2_signing_bytes_with_zstd(
                client_device_id,
                target_device_id,
                &peer_entry.x25519_pub_b64,
                &client_ephemeral_x25519_pub_b64,
                timestamp_ms,
                supports_zstd,
            );
            let verified_new = StoredDeviceIdentity::verify_signature(
                &peer_entry.ed25519_pub_b64,
                &signed_v2_zstd,
                &signature_b64,
            )
            .is_ok();
            if !verified_new {
                let signed_legacy = p2p_hello_v2_signing_bytes(
                    client_device_id,
                    target_device_id,
                    &peer_entry.x25519_pub_b64,
                    &client_ephemeral_x25519_pub_b64,
                    timestamp_ms,
                );
                StoredDeviceIdentity::verify_signature(
                    &peer_entry.ed25519_pub_b64,
                    &signed_legacy,
                    &signature_b64,
                )?;
            }

            let replay_key = format!(
                "p2p:{client_device_id}:{timestamp_ms}:{client_ephemeral_x25519_pub_b64}:{signature_b64}"
            );
            let ttl = std::time::Duration::from_millis(HANDSHAKE_MAX_SKEW_MS * 2);
            if !replay_cache.lock().await.check_and_insert(replay_key, ttl) {
                anyhow::bail!("replayed DirectP2pHello rejected");
            }

            Ok(VerifiedP2pHello {
                client_device_id,
                client_static_x25519_pub_b64: peer_entry.x25519_pub_b64,
                client_ephemeral_x25519_pub_b64,
                supports_zstd,
            })
        }
        other => anyhow::bail!("expected DirectP2pHello, got {other:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_direct_p2p_frame(
    frame: RelayWireFrame,
    cfg: &DaemonConfig,
    herdr: &SharedHerdr,
    fallback_shells: &Arc<Mutex<NativeFallbackShells>>,
    sessions: &Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>>,
    active_ptys: &Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
    out_tx: &mpsc::Sender<RelayWireFrame>,
    rpc_semaphore: &Arc<Semaphore>,
    p2p_conn_nonce: &str,
) {
    match frame {
        RelayWireFrame::KeepAlive { ts_ms } => {
            let _ = out_tx.try_send(RelayWireFrame::KeepAlive { ts_ms });
        }
        RelayWireFrame::RoutePacket {
            src_device_id,
            dst_device_id,
            e2ee,
            ..
        } => {
            if dst_device_id != cfg.device_id {
                return;
            }
            let session_key = (src_device_id, p2p_conn_nonce.to_string());
            let req_msg = {
                let mut map = sessions.lock().await;
                let Some(s) = map.get_mut(&session_key) else {
                    return;
                };
                match s.e2ee.open_message(&e2ee) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("P2P E2EE open error: {e}");
                        return;
                    }
                }
            };

            dispatch_decrypted_peer_message(
                session_key,
                req_msg,
                herdr,
                fallback_shells,
                &cfg.identity.name,
                active_ptys,
                sessions,
                cfg.device_id,
                rpc_semaphore,
            )
            .await;
        }
        _ => {}
    }
}

fn build_signed_p2p_ack_and_session(
    cfg: &DaemonConfig,
    verified: &VerifiedP2pHello,
    path_kind: &str,
    reply_tx: mpsc::Sender<RelayWireFrame>,
) -> Result<(RelayWireFrame, PeerCryptoSession)> {
    let my_dh_secret = cfg.identity.x25519_secret()?;
    let (daemon_eph_secret, daemon_eph_pub_b64) = StoredDeviceIdentity::generate_ephemeral_x25519();
    let ack_ts = current_unix_ms();
    let ack_signing = p2p_ack_v2_signing_bytes(
        verified.client_device_id,
        cfg.device_id,
        &verified.client_ephemeral_x25519_pub_b64,
        &daemon_eph_pub_b64,
        ack_ts,
    );
    let signature_b64 = cfg.identity.sign(&ack_signing)?;
    let sess = E2eeSession::derive_ephemeral_p2p(
        &my_dh_secret,
        &verified.client_static_x25519_pub_b64,
        &daemon_eph_secret,
        &verified.client_ephemeral_x25519_pub_b64,
        verified.client_device_id,
        cfg.device_id,
        false,
        verified.supports_zstd,
    )?;
    let ack = RelayWireFrame::DirectP2pAck {
        ok: true,
        daemon_device_id: cfg.device_id,
        daemon_ed25519_pub_b64: cfg.identity.ed25519_public_b64.clone(),
        daemon_x25519_pub_b64: cfg.identity.x25519_public_b64.clone(),
        daemon_ephemeral_x25519_pub_b64: daemon_eph_pub_b64,
        timestamp_ms: ack_ts,
        signature_b64,
        path_kind: path_kind.into(),
        error: None,
    };
    Ok((
        ack,
        PeerCryptoSession {
            active_session_nonce: None,
            e2ee: sess,
            reply_tx,
            created_at: tokio::time::Instant::now(),
        },
    ))
}

fn build_quiet_p2p_reject(path_kind: &str) -> RelayWireFrame {
    RelayWireFrame::DirectP2pAck {
        ok: false,
        daemon_device_id: Uuid::nil(),
        daemon_ed25519_pub_b64: String::new(),
        daemon_x25519_pub_b64: String::new(),
        daemon_ephemeral_x25519_pub_b64: String::new(),
        timestamp_ms: current_unix_ms(),
        signature_b64: String::new(),
        path_kind: path_kind.into(),
        error: Some("unauthorized".into()),
    }
}

fn is_allowed_p2p_ws_origin(origin_raw: &str, server_url: &str, request_host: Option<&str>) -> bool {
    let origin = origin_raw.trim().trim_end_matches('/').to_ascii_lowercase();
    if origin.is_empty() {
        return false;
    }
    let srv = server_url.trim().trim_end_matches('/').to_ascii_lowercase();
    if !srv.is_empty() && origin == srv {
        return true;
    }
    if origin.starts_with("http://localhost")
        || origin.starts_with("http://127.0.0.1")
        || origin.starts_with("http://[::1]")
        || origin.starts_with("https://localhost")
        || origin.starts_with("https://127.0.0.1")
    {
        return true;
    }
    if let Some(host) = request_host.map(str::trim).filter(|s| !s.is_empty()) {
        let host_lower = host.to_ascii_lowercase();
        let host_only = host_lower
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(&host_lower);
        if origin == format!("http://{host_lower}")
            || origin == format!("https://{host_lower}")
            || origin.starts_with(&format!("http://{host_only}:"))
            || origin.starts_with(&format!("https://{host_only}:"))
            || origin == format!("http://{host_only}")
            || origin == format!("https://{host_only}")
        {
            return true;
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
async fn handle_direct_p2p_socket(
    stream: TcpStream,
    peer_addr: SocketAddr,
    cfg: DaemonConfig,
    herdr: SharedHerdr,
    fallback_shells: Arc<Mutex<NativeFallbackShells>>,
    active_ptys: Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
    peers_store: Arc<TrustedPeersStore>,
    replay_cache: Arc<Mutex<TimedReplayCache>>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    if *shutdown_rx.borrow() {
        return Ok(());
    }
    let (mut reader, mut writer) = stream.into_split();

    // #5: Pre-auth frame read strictly bounded to MAX_AUTH_FRAME_BYTES (16 KiB).
    let hello = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_raw_frame_limited(&mut reader, MAX_AUTH_FRAME_BYTES),
    )
    .await
    .map_err(|_| anyhow!("direct P2P hello read timed out"))??;

    let path_kind = if peer_addr.is_ipv6() {
        "cn_public_ipv6"
    } else {
        "lan_ipv4"
    };

    let verified = match verify_direct_p2p_hello(&cfg, &peers_store, &replay_cache, hello).await {
        Ok(v) => v,
        Err(e) => {
            let reject = build_quiet_p2p_reject(path_kind);
            if let Ok(bytes) = reject.encode_length_delimited() {
                let _ = writer.write_all(&bytes).await;
            }
            return Err(e);
        }
    };
    if *shutdown_rx.borrow() {
        return Ok(());
    }

    let (out_tx, mut out_rx) = mpsc::channel::<RelayWireFrame>(256);
    let (ack, p2p_sess) =
        build_signed_p2p_ack_and_session(&cfg, &verified, path_kind, out_tx.clone())?;
    writer.write_all(&ack.encode_length_delimited()?).await?;

    let (writer_dead_tx, mut writer_dead_rx) = watch::channel(false);
    let write_task = tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(WS_KEEPALIVE_SECS));
        keepalive.tick().await;
        loop {
            tokio::select! {
                maybe_f = out_rx.recv() => {
                    let Some(f) = maybe_f else { break; };
                    if let Ok(bytes) = f.encode_length_delimited() {
                        if writer.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                }
                _ = keepalive.tick() => {
                    let ka = RelayWireFrame::KeepAlive { ts_ms: current_unix_ms() };
                    if let Ok(bytes) = ka.encode_length_delimited() {
                        if writer.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
        let _ = writer_dead_tx.send(true);
    });

    let p2p_conn_nonce = format!("p2p-{}", Uuid::new_v4().simple());
    let my_session_key = (verified.client_device_id, p2p_conn_nonce.clone());
    let sessions = Arc::new(Mutex::new(HashMap::new()));
    sessions
        .lock()
        .await
        .insert(my_session_key.clone(), p2p_sess);
    let rpc_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PEER_RPCS));
    let stop_events = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_forwarder = spawn_herdr_event_forwarder(
        Arc::clone(&herdr),
        Arc::clone(&sessions),
        cfg.device_id,
        Arc::clone(&stop_events),
    );
    // #3: Subscribe to live TrustedPeersStore updates so revoked devices disconnect immediately.
    let mut peer_updates = peers_store.subscribe_updates();
    let read_idle_timeout = std::time::Duration::from_secs(WS_KEEPALIVE_SECS * 3);
    // Dedicated cancel-safe reader task so `tokio::select!` branches never drop half-read TCP frames.
    let (frame_tx, mut frame_rx) = mpsc::channel::<RelayWireFrame>(64);
    let read_task = tokio::spawn(async move {
        while let Ok(Ok(frame)) = tokio::time::timeout(read_idle_timeout, read_raw_frame(&mut reader)).await {
            if frame_tx.send(frame).await.is_err() {
                break;
            }
        }
    });

    loop {
        if *shutdown_rx.borrow() || *writer_dead_rx.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Direct P2P socket to {} terminated by host shutdown", verified.client_device_id);
                    break;
                }
            }
            _ = writer_dead_rx.changed() => {
                if *writer_dead_rx.borrow() {
                    break;
                }
            }
            upd = peer_updates.recv() => {
                if matches!(upd, Ok(()) | Err(broadcast::error::RecvError::Lagged(_)))
                    && !peers_store.contains(&verified.client_device_id).await
                {
                    warn!("Direct P2P peer {} revoked; terminating socket", verified.client_device_id);
                    break;
                }
            }
            maybe_frame = frame_rx.recv() => {
                let Some(frame) = maybe_frame else { break; };
                dispatch_direct_p2p_frame(
                    frame,
                    &cfg,
                    &herdr,
                    &fallback_shells,
                    &sessions,
                    &active_ptys,
                    &out_tx,
                    &rpc_semaphore,
                    &p2p_conn_nonce,
                )
                .await;
            }
        }
    }

    read_task.abort();
    stop_events.store(true, Ordering::Relaxed);
    event_forwarder.abort();
    active_ptys
        .lock()
        .await
        .retain(|(sk, _), _| sk != &my_session_key);
    write_task.abort();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_direct_p2p_ws_socket(
    stream: TcpStream,
    peer_addr: SocketAddr,
    cfg: DaemonConfig,
    herdr: SharedHerdr,
    fallback_shells: Arc<Mutex<NativeFallbackShells>>,
    active_ptys: Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
    peers_store: Arc<TrustedPeersStore>,
    replay_cache: Arc<Mutex<TimedReplayCache>>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    if *shutdown_rx.borrow() {
        return Ok(());
    }
    let allowed_server_url = cfg.server_url.clone();
    // #6: Reject cross-site WebSocket upgrades from untrusted external browser Origins.
    let ws_stream = accept_hdr_async_with_config(
        stream,
        move |req: &WsRequest, resp: WsResponse| -> std::result::Result<WsResponse, ErrorResponse> {
            if let Some(origin) = req.headers().get("origin").and_then(|v| v.to_str().ok()) {
                let host = req.headers().get("host").and_then(|v| v.to_str().ok());
                if !is_allowed_p2p_ws_origin(origin, &allowed_server_url, host) {
                    let mut err = ErrorResponse::new(Some("Origin not allowed".into()));
                    *err.status_mut() = WsStatusCode::FORBIDDEN;
                    return Err(err);
                }
            }
            Ok(resp)
        },
        Some(bounded_ws_config()),
    )
    .await
    .with_context(|| format!("direct P2P WebSocket upgrade failed for {peer_addr}"))?;
    let (mut sink, mut rx) = ws_stream.split();

    let first_msg = tokio::time::timeout(std::time::Duration::from_secs(10), rx.next())
        .await
        .map_err(|_| anyhow!("direct P2P WebSocket hello read timed out"))?
        .ok_or_else(|| anyhow!("direct P2P WebSocket closed before hello"))?;
    let hello: RelayWireFrame = match first_msg? {
        WsMsg::Text(t) if t.len() <= MAX_AUTH_FRAME_BYTES => serde_json::from_str(&t)?,
        WsMsg::Binary(b) if b.len() <= MAX_AUTH_FRAME_BYTES => serde_json::from_slice(&b)?,
        WsMsg::Text(_) | WsMsg::Binary(_) => {
            anyhow::bail!("direct P2P WebSocket hello exceeds pre-auth limit")
        }
        other => anyhow::bail!("unexpected initial WS message: {other:?}"),
    };

    let path_kind = if peer_addr.is_ipv6() {
        "ipv6_ws"
    } else {
        "lan_ws"
    };
    let verified = match verify_direct_p2p_hello(&cfg, &peers_store, &replay_cache, hello).await {
        Ok(v) => v,
        Err(e) => {
            let reject = build_quiet_p2p_reject(path_kind);
            if let Ok(txt) = serde_json::to_string(&reject) {
                let _ = sink.send(WsMsg::Text(txt.into())).await;
                let _ = sink.flush().await;
            }
            return Err(e);
        }
    };
    if *shutdown_rx.borrow() {
        return Ok(());
    }

    let (out_tx, mut out_rx) = mpsc::channel::<RelayWireFrame>(256);
    let (ack, p2p_sess) =
        build_signed_p2p_ack_and_session(&cfg, &verified, path_kind, out_tx.clone())?;
    sink.send(WsMsg::Text(serde_json::to_string(&ack)?.into()))
        .await?;
    sink.flush().await?;

    let (pong_tx, mut pong_rx) = mpsc::channel::<Vec<u8>>(16);
    let (writer_dead_tx, mut writer_dead_rx) = watch::channel(false);
    let write_task = tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(WS_KEEPALIVE_SECS));
        keepalive.tick().await;
        loop {
            tokio::select! {
                Some(p) = pong_rx.recv() => {
                    if sink.send(WsMsg::Pong(p.into())).await.is_err()
                        || sink.flush().await.is_err()
                    {
                        break;
                    }
                }
                maybe_f = out_rx.recv() => {
                    let Some(f) = maybe_f else { break; };
                    if let Ok(txt) = serde_json::to_string(&f) {
                        if sink.send(WsMsg::Text(txt.into())).await.is_err()
                            || sink.flush().await.is_err()
                        {
                            break;
                        }
                    }
                }
                _ = keepalive.tick() => {
                    if sink.send(WsMsg::Ping(Vec::new().into())).await.is_err()
                        || sink.flush().await.is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = writer_dead_tx.send(true);
    });

    let p2p_conn_nonce = format!("p2p-{}", Uuid::new_v4().simple());
    let my_session_key = (verified.client_device_id, p2p_conn_nonce.clone());
    let sessions = Arc::new(Mutex::new(HashMap::new()));
    sessions
        .lock()
        .await
        .insert(my_session_key.clone(), p2p_sess);
    let rpc_semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PEER_RPCS));
    let stop_events = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event_forwarder = spawn_herdr_event_forwarder(
        Arc::clone(&herdr),
        Arc::clone(&sessions),
        cfg.device_id,
        Arc::clone(&stop_events),
    );
    let mut peer_updates = peers_store.subscribe_updates();
    let read_idle_timeout = std::time::Duration::from_secs(WS_KEEPALIVE_SECS * 3);

    loop {
        if *shutdown_rx.borrow() || *writer_dead_rx.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Direct P2P WS socket to {} terminated by host shutdown", verified.client_device_id);
                    break;
                }
            }
            _ = writer_dead_rx.changed() => {
                if *writer_dead_rx.borrow() {
                    break;
                }
            }
            upd = peer_updates.recv() => {
                if matches!(upd, Ok(()) | Err(broadcast::error::RecvError::Lagged(_)))
                    && !peers_store.contains(&verified.client_device_id).await
                {
                    warn!("Direct P2P WS peer {} revoked; terminating socket", verified.client_device_id);
                    break;
                }
            }
            maybe_msg = tokio::time::timeout(read_idle_timeout, rx.next()) => {
                let Ok(Some(Ok(msg))) = maybe_msg else { break; };
                match msg {
                    WsMsg::Text(t) => {
                        if let Ok(frame) = serde_json::from_str::<RelayWireFrame>(&t) {
                            dispatch_direct_p2p_frame(
                                frame,
                                &cfg,
                                &herdr,
                                &fallback_shells,
                                &sessions,
                                &active_ptys,
                                &out_tx,
                                &rpc_semaphore,
                                &p2p_conn_nonce,
                            )
                            .await;
                        }
                    }
                    WsMsg::Binary(b) => {
                        if let Ok(frame) = serde_json::from_slice::<RelayWireFrame>(&b) {
                            dispatch_direct_p2p_frame(
                                frame,
                                &cfg,
                                &herdr,
                                &fallback_shells,
                                &sessions,
                                &active_ptys,
                                &out_tx,
                                &rpc_semaphore,
                                &p2p_conn_nonce,
                            )
                            .await;
                        }
                    }
                    WsMsg::Ping(p) => {
                        let _ = pong_tx.try_send(p.to_vec());
                    }
                    WsMsg::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    stop_events.store(true, Ordering::Relaxed);
    event_forwarder.abort();
    active_ptys
        .lock()
        .await
        .retain(|(sk, _), _| sk != &my_session_key);
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
    spawn_lan_udp_beacon_with_abort(device_id, tcp_p2p_port, bind_addr)
        .await
        .map(|(addr, _)| addr)
}

async fn spawn_lan_udp_beacon_with_abort(
    device_id: Uuid,
    tcp_p2p_port: u16,
    bind_addr: SocketAddr,
) -> Result<(SocketAddr, tokio::task::AbortHandle)> {
    let sock = TokioUdpSocket::bind(bind_addr).await?;
    let local_addr = sock.local_addr()?;
    info!(
        "Local Wi-Fi UDP discovery beacon listening on {} (tcp_p2p_port={})",
        local_addr, tcp_p2p_port
    );

    let handle = tokio::spawn(async move {
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

    Ok((local_addr, handle.abort_handle()))
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

/// Shutdown handle for an embedded desktop host P2P/UDP/relay runtime.
#[derive(Debug, Clone)]
pub struct EmbeddedHostHandle {
    pub local_addr: SocketAddr,
    abort_handles: Arc<Vec<tokio::task::AbortHandle>>,
    shutdown_tx: watch::Sender<bool>,
}

impl EmbeddedHostHandle {
    /// Terminates the embedded relay loop, P2P listener, active P2P sockets, and UDP beacon
    /// and releases the daemon lock immediately.
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
        for handle in self.abort_handles.iter() {
            handle.abort();
        }
    }
}

/// Starts embedded host P2P + UDP LAN beacon + Relay uplink inside desktop apps (e.g. `coda-desktop`)
/// if `cfg_path` exists and no external `coda-daemon` holds `coda-daemon.lock`.
pub async fn start_embedded_host_if_configured(cfg_path: &Path) -> Option<SocketAddr> {
    start_embedded_host_with_handle(cfg_path)
        .await
        .map(|h| h.local_addr)
}

/// Starts the embedded desktop host and returns a handle that cleanly aborts its tasks on sign-out.
pub async fn start_embedded_host_with_handle(cfg_path: &Path) -> Option<EmbeddedHostHandle> {
    let lock = try_acquire_daemon_lock(cfg_path).ok().flatten()?;
    let mut cfg = DaemonConfig::load(cfg_path).ok()?;
    let peers_store = TrustedPeersStore::from_daemon_config(
        &cfg,
        Some(DaemonConfig::trusted_peers_path(cfg_path)),
    );
    let _ = maybe_renew_daemon_config(&mut cfg, Some(cfg_path), &peers_store, false).await;
    let _ = peers_store.refresh_from_server(true).await;

    let runtime = SharedHostRuntime::new().await;
    let shutdown_tx = runtime.shutdown_tx.clone();
    let mut abort_handles = Vec::new();
    let p2p_bind = SocketAddr::from(([0, 0, 0, 0], DEFAULT_DIRECT_P2P_PORT));
    let (p2p_addr, p2p_abort) = match spawn_direct_p2p_listener_with_runtime(
        cfg.clone(),
        p2p_bind,
        Arc::clone(&peers_store),
        runtime.clone(),
    )
    .await
    {
        Ok(res) => res,
        Err(_) => {
            let fallback = SocketAddr::from(([0, 0, 0, 0], 0));
            spawn_direct_p2p_listener_with_runtime(
                cfg.clone(),
                fallback,
                Arc::clone(&peers_store),
                runtime.clone(),
            )
            .await
            .ok()?
        }
    };
    abort_handles.push(p2p_abort);
    if let Ok((_, udp_abort)) =
        spawn_lan_udp_beacon_with_abort(cfg.device_id, p2p_addr.port(), p2p_bind).await
    {
        abort_handles.push(udp_abort);
    }

    let port = p2p_addr.port();
    let cfg_path_buf = cfg_path.to_path_buf();
    let relay_runtime = runtime.clone();
    let relay_task = tokio::spawn(async move {
        let _held_lock = lock;
        let mut backoff_secs = 1u64;
        let shutdown_rx = relay_runtime.shutdown_rx();
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            let _ = maybe_renew_daemon_config(&mut cfg, Some(&cfg_path_buf), &peers_store, false)
                .await;
            let started = tokio::time::Instant::now();
            match run_daemon_once_with_runtime(
                &cfg,
                None,
                Some(port),
                Arc::clone(&peers_store),
                relay_runtime.clone(),
            )
            .await
            {
                Ok(()) => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    if started.elapsed() >= std::time::Duration::from_secs(10) {
                        backoff_secs = 1;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(30);
                }
                Err(e) => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                    let msg = e.to_string().to_ascii_lowercase();
                    if msg.contains("expired") || msg.contains("ticket") || msg.contains("unauthorized") {
                        let _ = maybe_renew_daemon_config(
                            &mut cfg,
                            Some(&cfg_path_buf),
                            &peers_store,
                            true,
                        )
                        .await;
                    }
                    if started.elapsed() >= std::time::Duration::from_secs(10) {
                        backoff_secs = 1;
                    }
                    warn!(
                        "embedded Mac host relay disconnected ({e}); reconnecting in {backoff_secs}s..."
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(30);
                }
            }
        }
    });
    abort_handles.push(relay_task.abort_handle());

    Some(EmbeddedHostHandle {
        local_addr: p2p_addr,
        abort_handles: Arc::new(abort_handles),
        shutdown_tx,
    })
}

fn sanitize_upload_filename(raw: &str) -> Result<String> {
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

fn redact_voice_config(cfg: VoiceModelConfig) -> VoiceModelConfig {
    VoiceModelConfig {
        provider: cfg.provider,
        base_url: cfg.base_url,
        // #10: Never broadcast plaintext API key in WorkspacesSnapshot.
        api_key: String::new(),
        model: cfg.model,
    }
}

fn key_name_to_terminal_bytes(key: &str) -> Option<Vec<u8>> {
    match key {
        "Enter" | "Return" => Some(b"\r".to_vec()),
        "Tab" => Some(b"\t".to_vec()),
        "Shift+Tab" | "BackTab" | "BTab" => Some(b"\x1b[Z".to_vec()),
        "Escape" | "Esc" => Some(b"\x1b".to_vec()),
        "Backspace" | "BSpace" => Some(b"\x7f".to_vec()),
        "Up" => Some(b"\x1b[A".to_vec()),
        "Down" => Some(b"\x1b[B".to_vec()),
        "Right" => Some(b"\x1b[C".to_vec()),
        "Left" => Some(b"\x1b[D".to_vec()),
        "Home" => Some(b"\x1b[H".to_vec()),
        "End" => Some(b"\x1b[F".to_vec()),
        "PageUp" => Some(b"\x1b[5~".to_vec()),
        "PageDown" => Some(b"\x1b[6~".to_vec()),
        "C-c" | "Ctrl+C" => Some(vec![0x03]),
        "C-d" | "Ctrl+D" => Some(vec![0x04]),
        "C-l" | "Ctrl+L" => Some(vec![0x0c]),
        "C-z" | "Ctrl+Z" => Some(vec![0x1a]),
        other if other.len() == 1 => Some(other.as_bytes().to_vec()),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_peer_request_full(
    session_key: PeerSessionKey,
    msg: PeerMessage,
    herdr: &SharedHerdr,
    fallback_shells: &Arc<Mutex<NativeFallbackShells>>,
    hostname: &str,
    active_ptys: &Arc<Mutex<HashMap<(PeerSessionKey, String), ActivePtySubscription>>>,
    sessions: &Arc<Mutex<HashMap<PeerSessionKey, PeerCryptoSession>>>,
    my_device_id: Uuid,
) -> Option<(PeerMessage, TrafficLane)> {
    let resolved_herdr = resolve_herdr_lazy(herdr).await;
    match msg {
        PeerMessage::ListWorkspaces { req_id, root_dirs } => {
            let mut terminals = match resolved_herdr.as_deref() {
                Some(hc) => hc.list_terminals(false).await.unwrap_or_default(),
                None => vec![],
            };
            let fb_terms = fallback_shells.lock().await.list(false);
            terminals.extend(fb_terms);

            let mut repos = scan_git_workspaces(hostname, &root_dirs, &terminals).await;
            // #47: On hosts with zero Git repositories, expose a real `$HOME` workspace so
            // file browsing and `+` agent launching work out of the box.
            if repos.is_empty() {
                let home_dir = std::env::var("HOME")
                    .ok()
                    .filter(|s| !s.trim().is_empty() && Path::new(s).is_dir())
                    .or_else(|| {
                        std::env::current_dir()
                            .ok()
                            .map(|p| p.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "/".to_string());
                let live_terms: Vec<HerdrTerminalSummary> =
                    terminals.into_iter().filter(|t| t.alive).collect();
                let status_summary = if live_terms.is_empty() {
                    "暂无终端".into()
                } else {
                    format!("{} 个终端", live_terms.len())
                };
                repos.push(RepoWorkspaceSummary {
                    repo_name: "~".into(),
                    root_path: home_dir.clone(),
                    machine_name: hostname.to_string(),
                    branches: vec![BranchWorktreeItem {
                        branch: "home".into(),
                        worktree_path: home_dir,
                        diff_added: 0,
                        diff_deleted: 0,
                        active_terminals: live_terms,
                        status_summary,
                    }],
                });
            }
            let voice_config = match resolved_herdr.as_deref() {
                Some(hc) => hc
                    .get_voice_config()
                    .await
                    .ok()
                    .flatten()
                    .map(redact_voice_config),
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
            // Check native fallback shells first (if terminal was spawned in fallback mode)
            let fallback_attach = {
                let mut fb = fallback_shells.lock().await;
                if fb.terminals.contains_key(&terminal) {
                    Some(fb.attach(&terminal, cols, rows))
                } else {
                    None
                }
            };
            if let Some(attach_res) = fallback_attach {
                match attach_res {
                    Ok((initial_snap, mut brx)) => {
                        let sessions_clone = Arc::clone(sessions);
                        let sk_clone = session_key.clone();
                        let term_clone = terminal.clone();
                        let forward = tokio::spawn(async move {
                            let mut seq = 1u64;
                            if !initial_snap.is_empty() {
                                let mut snap_payload = Vec::with_capacity(initial_snap.len() + 7);
                                snap_payload.extend_from_slice(b"\x1b[2J\x1b[H");
                                snap_payload.extend_from_slice(&initial_snap);
                                let snap_msg = PeerMessage::PtyOutputChunk {
                                    terminal: term_clone.clone(),
                                    seq,
                                    data_b64: B64.encode(&snap_payload),
                                };
                                seq += 1;
                                if !send_e2ee_peer_message(
                                    &sessions_clone,
                                    my_device_id,
                                    &sk_clone,
                                    &snap_msg,
                                    TrafficLane::ControlP0,
                                )
                                .await
                                {
                                    return;
                                }
                            }
                            loop {
                                match brx.recv().await {
                                    Ok(ev) => {
                                        let chunk_msg = PeerMessage::PtyOutputChunk {
                                            terminal: ev.terminal,
                                            seq,
                                            data_b64: ev.data_b64,
                                        };
                                        seq += 1;
                                        if !send_e2ee_peer_message(
                                            &sessions_clone,
                                            my_device_id,
                                            &sk_clone,
                                            &chunk_msg,
                                            TrafficLane::ControlP0,
                                        )
                                        .await
                                        {
                                            break;
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                    Err(broadcast::error::RecvError::Closed) => break,
                                }
                            }
                        });
                        {
                            let mut ptys = active_ptys.lock().await;
                            // V3-01: Single-owner cross-transport eviction per (client_device_id, terminal)
                            ptys.retain(|((dev_id, _), term), _| {
                                !(*dev_id == session_key.0 && term == &terminal)
                            });
                            ptys.insert(
                                (session_key, terminal.clone()),
                                ActivePtySubscription::Fallback(forward.abort_handle()),
                            );
                        }
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

            let Some(hc) = resolved_herdr.as_deref() else {
                return Some((
                    PeerMessage::PtyAttached {
                        req_id,
                        terminal,
                        ok: false,
                        error: Some("terminal not found on host".into()),
                    },
                    TrafficLane::ControlP0,
                ));
            };

            let (pty_tx, mut pty_rx) = mpsc::channel::<PtyOutputEvent>(256);
            match spawn_pty_attach(hc.hc_path(), &terminal, cols, rows, pty_tx) {
                Ok(handle) => {
                    {
                        let mut ptys = active_ptys.lock().await;
                        // V3-01: Single-owner cross-transport eviction per (client_device_id, terminal)
                        ptys.retain(|((dev_id, _), term), _| {
                            !(*dev_id == session_key.0 && term == &terminal)
                        });
                        ptys.insert(
                            (session_key.clone(), terminal.clone()),
                            ActivePtySubscription::Herdr(handle),
                        );
                    }

                    let sessions_clone = Arc::clone(sessions);
                    let sk_clone = session_key;
                    tokio::spawn(async move {
                        while let Some(ev) = pty_rx.recv().await {
                            let chunk_msg = PeerMessage::PtyOutputChunk {
                                terminal: ev.terminal,
                                seq: ev.seq,
                                data_b64: ev.data_b64,
                            };
                            if !send_e2ee_peer_message(
                                &sessions_clone,
                                my_device_id,
                                &sk_clone,
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
            let Ok(bytes) = B64.decode(&data_b64) else {
                return None;
            };
            {
                let fb = fallback_shells.lock().await;
                if fb.terminals.contains_key(&terminal) {
                    let _ = fb.write_input(&terminal, &bytes);
                    return None;
                }
            }
            {
                let map = active_ptys.lock().await;
                if let Some(ActivePtySubscription::Herdr(pty)) =
                    map.get(&(session_key, terminal.clone()))
                {
                    let _ = pty.write_raw(&bytes);
                    return None;
                }
            }
            // #25: Never inject unintended Enter before PtyAttach completes.
            // Only forward via `hc.send_text` if the input explicitly ended with CR/LF.
            if let Some(hc) = resolved_herdr.as_deref() {
                if bytes.ends_with(b"\r") || bytes.ends_with(b"\n") {
                    let stripped = String::from_utf8_lossy(&bytes)
                        .trim_end_matches(&['\r', '\n'][..])
                        .to_string();
                    if !stripped.is_empty() {
                        let _ = hc.send_text(&terminal, &stripped, true).await;
                    } else {
                        let _ = hc.send_keys(&terminal, &["Enter".to_string()]).await;
                    }
                }
            }
            return None;
        }
        PeerMessage::PtyResize {
            terminal,
            cols,
            rows,
        } => {
            {
                let mut fb = fallback_shells.lock().await;
                if fb.terminals.contains_key(&terminal) {
                    let _ = fb.resize(&terminal, cols, rows);
                    return None;
                }
            }
            let map = active_ptys.lock().await;
            if let Some(ActivePtySubscription::Herdr(pty)) = map.get(&(session_key, terminal)) {
                let _ = pty.resize(cols, rows);
            }
            return None;
        }
        PeerMessage::PtyDetach { req_id, terminal } => {
            active_ptys.lock().await.remove(&(session_key, terminal));
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
            let clean_name = match sanitize_upload_filename(&filename) {
                Ok(n) => n,
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
            };
            let decoded_bytes = match B64.decode(&data_b64) {
                Ok(b) if b.len() <= MAX_UPLOAD_FILE_BYTES => b,
                Ok(_) => {
                    return Some((
                        PeerMessage::UploadTerminalFileResult {
                            req_id,
                            terminal,
                            ok: false,
                            remote_path: None,
                            bytes: 0,
                            error: Some("uploaded file exceeds 8 MiB limit".into()),
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
                            error: Some(format!("invalid base64 payload: {e}")),
                        },
                        TrafficLane::ControlP0,
                    ));
                }
            };
            let _ = decoded_bytes;

            let Some(hc) = resolved_herdr.as_deref() else {
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
            match hc.put_file(&terminal, &clean_name, &data_b64).await {
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
        PeerMessage::SetVoiceConfig { req_id, mut config } => {
            let ok = match resolved_herdr.as_deref() {
                Some(hc) => {
                    if config.api_key.trim().is_empty() {
                        if let Ok(Some(prev)) = hc.get_voice_config().await {
                            config.api_key = prev.api_key;
                        }
                    }
                    hc.set_voice_config(&config).await.is_ok()
                }
                None => false,
            };
            return Some((
                PeerMessage::VoiceConfigAck {
                    req_id,
                    ok,
                    config: redact_voice_config(config),
                },
                TrafficLane::ControlP0,
            ));
        }
        PeerMessage::KillTerminal { req_id, terminal } => {
            {
                let mut map = active_ptys.lock().await;
                map.retain(|(_, term), _| term != &terminal);
            }
            if fallback_shells.lock().await.kill(&terminal) {
                return Some((
                    PeerMessage::ActionAck {
                        req_id,
                        ok: true,
                        detail: format!("killed:{terminal}"),
                    },
                    TrafficLane::ControlP0,
                ));
            }
            let (ok, detail) = match resolved_herdr.as_deref() {
                Some(hc) => match hc.kill_and_remove(&terminal).await {
                    Ok(()) => (true, format!("killed:{terminal}")),
                    Err(e) => (false, format!("kill_failed:{terminal}:{e}")),
                },
                None => (false, "terminal not found on host".into()),
            };
            return Some((
                PeerMessage::ActionAck { req_id, ok, detail },
                TrafficLane::ControlP0,
            ));
        }
        other => Some(handle_peer_request(other, resolved_herdr.as_deref(), fallback_shells, hostname).await),
    }
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

fn validate_local_preview_target(port: u16, path: &str) -> Result<reqwest::Url, String> {
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

fn lossy_utf8_prefix(bytes: &[u8], max_bytes: usize) -> String {
    let slice = if bytes.len() <= max_bytes {
        bytes
    } else {
        // Trim back up to 3 bytes if slicing landed inside a multi-byte UTF-8 sequence (#25).
        let mut end = max_bytes;
        let floor = max_bytes.saturating_sub(3);
        while end > floor && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end -= 1;
        }
        &bytes[..end]
    };
    String::from_utf8_lossy(slice).to_string()
}

async fn handle_peer_request(
    msg: PeerMessage,
    herdr: Option<&HerdrClient>,
    fallback_shells: &Arc<Mutex<NativeFallbackShells>>,
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
                    .unwrap_or_else(|| "native-pty-fallback".into()),
            },
            TrafficLane::ControlP0,
        ),
        PeerMessage::ListTerminals {
            req_id,
            include_finished,
        } => {
            let mut fb_terms = fallback_shells.lock().await.list(include_finished);
            if let Some(hc) = herdr {
                match hc.list_terminals(include_finished).await {
                    Ok(mut terminals) => {
                        terminals.append(&mut fb_terms);
                        (
                            PeerMessage::TerminalsSnapshot {
                                req_id,
                                core_name: hc.info().core_name.clone(),
                                core_id: hc.info().core_id.clone(),
                                terminals,
                            },
                            TrafficLane::ControlP0,
                        )
                    }
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
                        core_id: "native-pty".into(),
                        terminals: fb_terms,
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
            if let Some(hc) = herdr {
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
                    Ok(term) => {
                        return (
                            PeerMessage::LaunchAgentResult {
                                req_id,
                                ok: true,
                                terminal: Some(term),
                                error: None,
                            },
                            TrafficLane::ControlP0,
                        )
                    }
                    Err(e) => {
                        return (
                            PeerMessage::LaunchAgentResult {
                                req_id,
                                ok: false,
                                terminal: None,
                                error: Some(e.to_string()),
                            },
                            TrafficLane::ControlP0,
                        )
                    }
                }
            }
            // #22: Built-in native Unix openpty fallback when `herdr-core` is not installed.
            let res = fallback_shells.lock().await.launch(
                &agent,
                name.as_deref(),
                cwd.as_deref(),
                initial_prompt.as_deref(),
                cols,
                rows,
            );
            match res {
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
            {
                let fb = fallback_shells.lock().await;
                if fb.terminals.contains_key(&terminal) {
                    let mut payload = text.into_bytes();
                    if !payload.ends_with(b"\r") && !payload.ends_with(b"\n") {
                        payload.push(b'\r');
                    }
                    let res = fb.write_input(&terminal, &payload);
                    return (
                        PeerMessage::ActionAck {
                            req_id,
                            ok: res.is_ok(),
                            detail: res.err().map(|e| e.to_string()).unwrap_or_else(|| "sent".into()),
                        },
                        TrafficLane::ControlP0,
                    );
                }
            }
            let res = match herdr {
                Some(hc) => hc.send_text(&terminal, &text, true).await,
                None => Err(anyhow!("terminal not found on host")),
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
            {
                let fb = fallback_shells.lock().await;
                if fb.terminals.contains_key(&terminal) {
                    let mut bytes = Vec::new();
                    for k in &keys {
                        if let Some(seq) = key_name_to_terminal_bytes(k) {
                            bytes.extend_from_slice(&seq);
                        }
                    }
                    let res = fb.write_input(&terminal, &bytes);
                    return (
                        PeerMessage::ActionAck {
                            req_id,
                            ok: res.is_ok(),
                            detail: res
                                .err()
                                .map(|e| e.to_string())
                                .unwrap_or_else(|| "keys_delivered".into()),
                        },
                        TrafficLane::ControlP0,
                    );
                }
            }
            let res = match herdr {
                Some(hc) => hc.send_keys(&terminal, &keys).await,
                None => Err(anyhow!("terminal not found on host")),
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
            {
                let fb = fallback_shells.lock().await;
                if fb.terminals.contains_key(&terminal) {
                    let res = fb.write_input(&terminal, &[0x03]);
                    return (
                        PeerMessage::ActionAck {
                            req_id,
                            ok: res.is_ok(),
                            detail: res
                                .err()
                                .map(|e| e.to_string())
                                .unwrap_or_else(|| "interrupted".into()),
                        },
                        TrafficLane::ControlP0,
                    );
                }
            }
            let res = match herdr {
                Some(hc) => hc.interrupt(&terminal).await,
                None => Err(anyhow!("terminal not found on host")),
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
            {
                let fb = fallback_shells.lock().await;
                if let Ok(text) = fb.read_screen(&terminal) {
                    return (
                        PeerMessage::TerminalScreenSnapshot {
                            req_id,
                            terminal,
                            text,
                            status: None,
                        },
                        TrafficLane::BulkP1,
                    );
                }
            }
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
            let safe_cwd = tokio::fs::canonicalize(&cwd)
                .await
                .ok()
                .filter(|p| !is_sensitive_path(p));
            let Some(canon_cwd) = safe_cwd else {
                return (
                    PeerMessage::GitDiffResponse {
                        req_id,
                        branch: String::new(),
                        status_short: String::new(),
                        diff_unified: String::new(),
                    },
                    TrafficLane::BulkP1,
                );
            };
            let cwd_str = canon_cwd.to_string_lossy().to_string();
            let branch = run_cmd_capture(
                "git",
                &[
                    "-C",
                    &cwd_str,
                    "-c",
                    "core.quotePath=false",
                    "-c",
                    "core.fsmonitor=false",
                    "rev-parse",
                    "--abbrev-ref",
                    "HEAD",
                ],
            )
            .await
            .unwrap_or_default();
            let status_short = run_cmd_capture(
                "git",
                &[
                    "-C",
                    &cwd_str,
                    "-c",
                    "core.quotePath=false",
                    "-c",
                    "core.fsmonitor=false",
                    "status",
                    "--short",
                ],
            )
            .await
            .unwrap_or_default();
            // #41: Diff against HEAD (falls back to working tree diff on unborn repos) so staged changes show too.
            let diff_unified = run_cmd_capture(
                "git",
                &[
                    "-C",
                    &cwd_str,
                    "-c",
                    "core.quotePath=false",
                    "-c",
                    "core.fsmonitor=false",
                    "diff",
                    "--no-color",
                    "HEAD",
                ],
            )
            .await
            .filter(|s| !s.is_empty())
            .or(run_cmd_capture(
                "git",
                &[
                    "-C",
                    &cwd_str,
                    "-c",
                    "core.quotePath=false",
                    "-c",
                    "core.fsmonitor=false",
                    "diff",
                    "--no-color",
                ],
            )
            .await)
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
            let target_url = match validate_local_preview_target(port, &path) {
                Ok(u) => u,
                Err(msg) => {
                    return (
                        PeerMessage::HttpLocalPreviewResponse {
                            req_id,
                            status_code: 403,
                            content_type: "text/plain".into(),
                            body_b64: B64.encode(msg),
                        },
                        TrafficLane::BulkP1,
                    );
                }
            };
            match preview_http_client().get(target_url).send().await {
                Ok(mut resp) => {
                    let status_code = resp.status().as_u16();
                    let content_type = resp
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("text/html; charset=utf-8")
                        .to_string();
                    if resp
                        .content_length()
                        .is_some_and(|n| n > MAX_HTTP_PREVIEW_BYTES as u64)
                    {
                        return (
                            PeerMessage::HttpLocalPreviewResponse {
                                req_id,
                                status_code: 413,
                                content_type: "text/plain".into(),
                                body_b64: B64.encode("Local preview response exceeds 4 MiB limit"),
                            },
                            TrafficLane::BulkP1,
                        );
                    }
                    let mut body = Vec::new();
                    while let Ok(Some(chunk)) = resp.chunk().await {
                        if body.len().saturating_add(chunk.len()) > MAX_HTTP_PREVIEW_BYTES {
                            return (
                                PeerMessage::HttpLocalPreviewResponse {
                                    req_id,
                                    status_code: 413,
                                    content_type: "text/plain".into(),
                                    body_b64: B64.encode("Local preview body exceeded 4 MiB limit"),
                                },
                                TrafficLane::BulkP1,
                            );
                        }
                        body.extend_from_slice(&chunk);
                    }
                    (
                        PeerMessage::HttpLocalPreviewResponse {
                            req_id,
                            status_code,
                            content_type,
                            body_b64: B64.encode(&body),
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
            let bad_sub = std::path::Path::new(clean_sub).components().any(|c| {
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
                for comp in std::path::Path::new(clean_sub).components() {
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
        PeerMessage::ReadFilePreview {
            req_id,
            path,
            max_bytes,
        } => {
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
    const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        Command::new(bin)
            .env("PATH", build_augmented_path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(args)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(lossy_utf8_prefix(&out.stdout, MAX_CAPTURE_BYTES))
}

async fn read_raw_frame_limited<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<RelayWireFrame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > max_bytes {
        anyhow::bail!("invalid frame length {len} (max {max_bytes})");
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

async fn read_raw_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<RelayWireFrame> {
    read_raw_frame_limited(reader, MAX_WIRE_FRAME_BYTES).await
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
}

impl DirectP2pClient {
    /// Races a 250ms Happy Eyeballs probe against a direct P2P endpoint (`lan_ipv4` or `cn_public_ipv6`),
    /// verifying both peers' static Ed25519 signatures and deriving forward-secret ephemeral X25519 keys.
    pub async fn connect_direct(
        endpoint: &str,
        client_id: &StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        expected_peer: &PeerIdentityPin,
    ) -> Result<Self> {
        validate_ed25519_pub_b64(&expected_peer.ed25519_pub_b64)?;
        validate_x25519_pub_b64(&expected_peer.x25519_pub_b64)?;

        let stream = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            TcpStream::connect(endpoint),
        )
        .await
        .context("direct P2P Happy Eyeballs probe timed out (250ms)")??;
        let _ = stream.set_nodelay(true);
        let (mut reader, mut writer) = stream.into_split();

        let (client_eph_secret, client_eph_pub_b64) =
            StoredDeviceIdentity::generate_ephemeral_x25519();
        let timestamp_ms = current_unix_ms();
        let signing = p2p_hello_v2_signing_bytes_with_zstd(
            client_device_id,
            target_device_id,
            &client_id.x25519_public_b64,
            &client_eph_pub_b64,
            timestamp_ms,
            true,
        );
        let signature_b64 = client_id.sign(&signing)?;

        let hello = RelayWireFrame::DirectP2pHello {
            client_device_id,
            target_device_id,
            client_ed25519_pub_b64: client_id.ed25519_public_b64.clone(),
            client_x25519_pub_b64: client_id.x25519_public_b64.clone(),
            client_ephemeral_x25519_pub_b64: client_eph_pub_b64.clone(),
            timestamp_ms,
            signature_b64,
            supports_zstd: true,
        };
        writer.write_all(&hello.encode_length_delimited()?).await?;

        let ack = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_raw_frame(&mut reader),
        )
        .await
        .map_err(|_| anyhow!("direct P2P handshake timed out waiting for DirectP2pAck"))??;

        let (daemon_eph_pub_b64, path_kind) = match ack {
            RelayWireFrame::DirectP2pAck {
                ok: true,
                daemon_device_id,
                daemon_ed25519_pub_b64,
                daemon_x25519_pub_b64,
                daemon_ephemeral_x25519_pub_b64,
                timestamp_ms: ack_ts_ms,
                signature_b64: ack_sig_b64,
                path_kind,
                ..
            } => {
                if daemon_device_id != target_device_id {
                    anyhow::bail!("direct P2P daemon_device_id mismatch");
                }
                if daemon_ed25519_pub_b64 != expected_peer.ed25519_pub_b64 {
                    anyhow::bail!("direct P2P daemon Ed25519 key mismatch against account pin");
                }
                if daemon_x25519_pub_b64 != expected_peer.x25519_pub_b64 {
                    anyhow::bail!("direct P25519 key mismatch against account pin");
                }
                if daemon_ephemeral_x25519_pub_b64.trim().is_empty() {
                    anyhow::bail!("direct P2P v2 requires non-empty daemon_ephemeral_x25519_pub_b64");
                }
                validate_x25519_pub_b64(&daemon_ephemeral_x25519_pub_b64)?;
                if !is_timestamp_fresh(ack_ts_ms, current_unix_ms(), HANDSHAKE_MAX_SKEW_MS) {
                    anyhow::bail!("direct P2P DirectP2pAck timestamp skewed or expired");
                }
                let ack_signed = p2p_ack_v2_signing_bytes(
                    client_device_id,
                    daemon_device_id,
                    &client_eph_pub_b64,
                    &daemon_ephemeral_x25519_pub_b64,
                    ack_ts_ms,
                );
                StoredDeviceIdentity::verify_signature(
                    &expected_peer.ed25519_pub_b64,
                    &ack_signed,
                    &ack_sig_b64,
                )?;
                (daemon_ephemeral_x25519_pub_b64, path_kind)
            }
            RelayWireFrame::DirectP2pAck {
                ok: false, error, ..
            } => {
                anyhow::bail!(
                    "direct P2P handshake rejected: {}",
                    error.unwrap_or_else(|| "unauthorized".into())
                );
            }
            other => anyhow::bail!("direct P2P handshake rejected: {other:?}"),
        };

        let e2ee = E2eeSession::derive_ephemeral_p2p(
            &client_id.x25519_secret()?,
            &expected_peer.x25519_pub_b64,
            &client_eph_secret,
            &daemon_eph_pub_b64,
            client_device_id,
            target_device_id,
            true,
            true,
        )?;

        Ok(Self {
            client_device_id,
            target_device_id,
            path_kind,
            writer,
            reader,
            e2ee,
        })
    }

    pub async fn send_message(&mut self, msg: &PeerMessage, lane: TrafficLane) -> Result<()> {
        let sealed = self.e2ee.seal_message(msg)?;
        let pkt = RelayWireFrame::RoutePacket {
            src_device_id: self.client_device_id,
            dst_device_id: self.target_device_id,
            lane,
            e2ee: sealed,
            sender_x25519_pub_b64: None,
            session_auth: None,
        };
        self.writer
            .write_all(&pkt.encode_length_delimited()?)
            .await?;
        Ok(())
    }

    pub async fn recv_message(&mut self) -> Result<PeerMessage> {
        loop {
            let frame = read_raw_frame(&mut self.reader).await?;
            if let RelayWireFrame::RoutePacket {
                src_device_id,
                dst_device_id,
                e2ee,
                ..
            } = frame
            {
                if src_device_id != self.target_device_id
                    || dst_device_id != self.client_device_id
                {
                    continue;
                }
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
    pub peer_ed25519_pub_b64: String,
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
    session_auth: RelaySessionAuth,
    auth_confirmed: bool,
    client_device_id: Uuid,
    target_device_id: Uuid,
}

impl RelayClientConn {
    async fn connect(
        client_id: &StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        cfg: &SmartRelayFallbackConfig,
    ) -> Result<Self> {
        validate_ed25519_pub_b64(&cfg.peer_ed25519_pub_b64)?;
        validate_x25519_pub_b64(&cfg.peer_x25519_pub_b64)?;

        let session_auth = RelaySessionAuth::create(
            client_id,
            target_device_id,
            true,
            current_unix_ms(),
        )?;
        let e2ee = E2eeSession::derive_relay_session(
            &client_id.x25519_secret()?,
            &cfg.peer_x25519_pub_b64,
            &session_auth.session_nonce_b64,
            client_device_id,
            target_device_id,
            true,
            true,
        )?;
        let bind = RelayWireFrame::AuthBind {
            ticket: cfg.relay_ticket.clone(),
            lane: TrafficLane::ControlP0,
            generation: next_monotonic_generation(),
        };

        if let Some(ref raw_addr) = cfg.raw_tcp_addr {
            let raw_attempt: Result<RelaySocketTransport> = async {
                let stream = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    TcpStream::connect(raw_addr),
                )
                .await
                .map_err(|_| anyhow!("raw TCP connect timed out"))??;
                let _ = stream.set_nodelay(true);
                let (mut reader, mut writer) = stream.into_split();
                writer.write_all(&bind.encode_length_delimited()?).await?;
                let ack = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    read_raw_frame(&mut reader),
                )
                .await
                .map_err(|_| anyhow!("raw TCP relay timed out waiting for AuthAck"))??;
                if matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }) {
                    Ok(RelaySocketTransport::RawTcp { reader, writer })
                } else {
                    anyhow::bail!("raw TCP relay rejected AuthBind")
                }
            }
            .await;

            match raw_attempt {
                Ok(transport) => {
                    return Ok(Self {
                        route_label: "relay_raw_tcp".into(),
                        transport,
                        e2ee,
                        session_auth,
                        auth_confirmed: false,
                        client_device_id,
                        target_device_id,
                    });
                }
                Err(e) => {
                    warn!("raw TCP relay {raw_addr} failed ({e}); falling back to WSS if configured");
                }
            }
        }

        if let Some(ref wss_url) = cfg.wss_url {
            let (ws_stream, _) =
                connect_async_with_config(wss_url, Some(bounded_ws_config()), false).await?;
            let (mut sink, mut stream) = ws_stream.split();
            sink.send(WsMsg::Text(serde_json::to_string(&bind)?.into()))
                .await?;
            sink.flush().await?;
            if let Ok(Some(Ok(WsMsg::Text(txt)))) =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()).await
            {
                let ack: RelayWireFrame = serde_json::from_str(&txt)?;
                if matches!(ack, RelayWireFrame::AuthAck { ok: true, .. }) {
                    return Ok(Self {
                        route_label: "relay_wss".into(),
                        transport: RelaySocketTransport::Wss { sink, stream },
                        e2ee,
                        session_auth,
                        auth_confirmed: false,
                        client_device_id,
                        target_device_id,
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
            sender_x25519_pub_b64: None,
            session_auth: if self.auth_confirmed {
                None
            } else {
                Some(self.session_auth.clone())
            },
        };

        match &mut self.transport {
            RelaySocketTransport::RawTcp { reader, writer } => {
                writer.write_all(&pkt.encode_length_delimited()?).await?;
                loop {
                    let frame = read_raw_frame(reader).await?;
                    match frame {
                        RelayWireFrame::RoutePacket {
                            src_device_id,
                            dst_device_id,
                            e2ee,
                            ..
                        } => {
                            if src_device_id != self.target_device_id
                                || dst_device_id != self.client_device_id
                            {
                                continue;
                            }
                            let opened = self.e2ee.open_message(&e2ee)?;
                            self.auth_confirmed = true;
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
                sink.flush().await?;
                while let Some(Ok(msg)) = stream.next().await {
                    match msg {
                        WsMsg::Ping(p) => {
                            let _ = sink.send(WsMsg::Pong(p)).await;
                            let _ = sink.flush().await;
                        }
                        WsMsg::Close(_) => break,
                        WsMsg::Text(t) => {
                            if let Ok(frame) = serde_json::from_str::<RelayWireFrame>(&t) {
                                match frame {
                                    RelayWireFrame::RoutePacket {
                                        src_device_id,
                                        dst_device_id,
                                        e2ee,
                                        ..
                                    } => {
                                        if src_device_id != self.target_device_id
                                            || dst_device_id != self.client_device_id
                                        {
                                            continue;
                                        }
                                        let opened = self.e2ee.open_message(&e2ee)?;
                                        self.auth_confirmed = true;
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
                        _ => {}
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
    expected_peer: PeerIdentityPin,
    endpoints: Vec<NetworkEndpointCandidate>,
    udp_probe_addr: Option<SocketAddr>,
    relay_cfg: Option<SmartRelayFallbackConfig>,
    direct: Option<DirectP2pClient>,
    relay_leg: Option<RelayClientConn>,
}

impl SmartPeerClient {
    #[allow(clippy::too_many_arguments)]
    pub async fn connect_auto(
        client_id: StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        expected_peer: PeerIdentityPin,
        endpoints: Vec<NetworkEndpointCandidate>,
        udp_probe_addr: Option<SocketAddr>,
        relay_cfg: Option<SmartRelayFallbackConfig>,
    ) -> Result<Self> {
        validate_ed25519_pub_b64(&expected_peer.ed25519_pub_b64)?;
        validate_x25519_pub_b64(&expected_peer.x25519_pub_b64)?;

        let mut me = Self {
            client_id,
            client_device_id,
            target_device_id,
            expected_peer,
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
                &self.expected_peer,
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
    expected_peer: &PeerIdentityPin,
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
        if let Ok(p2p) = DirectP2pClient::connect_direct(
            &addr,
            client_id,
            client_device_id,
            target_device_id,
            expected_peer,
        )
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
    #[allow(clippy::too_many_arguments)]
    pub async fn connect_auto(
        client_id: StoredDeviceIdentity,
        client_device_id: Uuid,
        target_device_id: Uuid,
        expected_peer: PeerIdentityPin,
        endpoints: Vec<NetworkEndpointCandidate>,
        udp_probe_addr: Option<SocketAddr>,
        relay_cfg: Option<SmartRelayFallbackConfig>,
    ) -> Result<Self> {
        validate_ed25519_pub_b64(&expected_peer.ed25519_pub_b64)?;
        validate_x25519_pub_b64(&expected_peer.x25519_pub_b64)?;

        let (reader_half, writer_half, e2ee, relay_session_auth, route_label, is_direct) =
            if let Some(p2p) = try_connect_direct_auto(
                &client_id,
                client_device_id,
                target_device_id,
                &expected_peer,
                &endpoints,
                udp_probe_addr,
            )
            .await
            {
                (
                    PeerReaderHalf::RawTcp(p2p.reader),
                    PeerWriterHalf::RawTcp(p2p.writer),
                    p2p.e2ee,
                    None,
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
                (
                    r,
                    w,
                    relay.e2ee,
                    Some(relay.session_auth),
                    relay.route_label,
                    false,
                )
            } else {
                anyhow::bail!(
                    "neither Direct LAN/IPv6 P2P nor Relay could connect to {target_device_id}"
                );
            };

        let e2ee_state = Arc::new(StdMutex::new(e2ee));
        let pending: Arc<StdMutex<HashMap<String, oneshot::Sender<PeerMessage>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<(PeerMessage, TrafficLane)>(512);
        let (pong_tx, mut pong_rx) = mpsc::channel::<Vec<u8>>(16);
        let (events_tx, _) = broadcast::channel::<PeerAsyncEvent>(1024);

        let e2ee_w = Arc::clone(&e2ee_state);
        let relay_auth_confirmed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let relay_auth_confirmed_w = Arc::clone(&relay_auth_confirmed);
        let mut writer = writer_half;
        let (writer_dead_tx, mut writer_dead_rx) = watch::channel(false);
        let writer_handle = tokio::spawn(async move {
            let mut keepalive =
                tokio::time::interval(std::time::Duration::from_secs(WS_KEEPALIVE_SECS));
            keepalive.tick().await;
            loop {
                tokio::select! {
                    Some(p) = pong_rx.recv() => {
                        if let PeerWriterHalf::Wss(sink) = &mut writer {
                            if sink.send(WsMsg::Pong(p.into())).await.is_err()
                                || sink.flush().await.is_err()
                            {
                                break;
                            }
                        }
                    }
                    maybe_item = outbound_rx.recv() => {
                        let Some((msg, lane)) = maybe_item else { break; };
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
                            sender_x25519_pub_b64: None,
                            session_auth: if relay_auth_confirmed_w.load(Ordering::Acquire) {
                                None
                            } else {
                                relay_session_auth.clone()
                            },
                        };
                        let send_ok = match &mut writer {
                            PeerWriterHalf::RawTcp(w) => match frame.encode_length_delimited() {
                                Ok(bytes) => w.write_all(&bytes).await.is_ok(),
                                Err(_) => false,
                            },
                            PeerWriterHalf::Wss(sink) => match serde_json::to_string(&frame) {
                                Ok(txt) => sink.send(WsMsg::Text(txt.into())).await.is_ok()
                                    && sink.flush().await.is_ok(),
                                Err(_) => false,
                            },
                        };
                        if !send_ok {
                            break;
                        }
                    }
                    _ = keepalive.tick() => {
                        let send_ok = match &mut writer {
                            PeerWriterHalf::RawTcp(w) => {
                                let ka = RelayWireFrame::KeepAlive { ts_ms: current_unix_ms() };
                                match ka.encode_length_delimited() {
                                    Ok(bytes) => w.write_all(&bytes).await.is_ok(),
                                    Err(_) => false,
                                }
                            }
                            PeerWriterHalf::Wss(sink) => {
                                sink.send(WsMsg::Ping(Vec::new().into())).await.is_ok()
                                    && sink.flush().await.is_ok()
                            }
                        };
                        if !send_ok {
                            break;
                        }
                    }
                }
            }
            let _ = writer_dead_tx.send(true);
        });
        let writer_abort = writer_handle.abort_handle();

        let e2ee_r = Arc::clone(&e2ee_state);
        let pending_r = Arc::clone(&pending);
        let events_tx_r = events_tx.clone();
        let writer_abort_for_reader = writer_abort.clone();
        let mut reader = reader_half;
        let read_idle_timeout = std::time::Duration::from_secs(WS_KEEPALIVE_SECS * 3);
        let reader_handle = tokio::spawn(async move {
            let mut disconnect_reason = "peer stream closed".to_string();
            loop {
                if *writer_dead_rx.borrow() {
                    disconnect_reason = "peer writer closed".to_string();
                    break;
                }
                let next_frame: Option<RelayWireFrame> = tokio::select! {
                    _ = writer_dead_rx.changed() => {
                        if *writer_dead_rx.borrow() {
                            disconnect_reason = "peer writer closed".to_string();
                            break;
                        }
                        continue;
                    }
                    res = tokio::time::timeout(read_idle_timeout, async {
                        match &mut reader {
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
                                    Some(Ok(WsMsg::Ping(p))) => {
                                        let _ = pong_tx.try_send(p.to_vec());
                                        break Some(RelayWireFrame::KeepAlive { ts_ms: 0 });
                                    }
                                    Some(Ok(WsMsg::Pong(_))) => {
                                        break Some(RelayWireFrame::KeepAlive { ts_ms: 0 });
                                    }
                                    Some(Ok(WsMsg::Close(_))) | Some(Err(_)) | None => break None,
                                    _ => continue,
                                }
                            },
                        }
                    }) => {
                        match res {
                            Ok(f) => f,
                            Err(_) => {
                                disconnect_reason = "peer read idle timeout".to_string();
                                None
                            }
                        }
                    }
                };

                let Some(frame) = next_frame else {
                    break;
                };

                match frame {
                    RelayWireFrame::RoutePacket {
                        src_device_id,
                        dst_device_id,
                        e2ee,
                        ..
                    } => {
                        if src_device_id != target_device_id
                            || dst_device_id != client_device_id
                        {
                            continue;
                        }
                        let opened = {
                            let mut guard = e2ee_r.lock().expect("e2ee mutex poisoned");
                            guard.open_message(&e2ee)
                        };
                        let Ok(msg) = opened else {
                            continue;
                        };
                        relay_auth_confirmed.store(true, Ordering::Release);
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
                    RelayWireFrame::SessionReset {
                        src_device_id,
                        reason,
                        ..
                    } => {
                        if src_device_id == target_device_id {
                            disconnect_reason = format!("session_reset_required: {reason}");
                            break;
                        }
                    }
                    RelayWireFrame::Error { code, message } => {
                        if code == "peer_backpressure" || code == "rate_limited" {
                            warn!("transient relay notice ({code}): {message}");
                            continue;
                        }
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

    /// Non-blocking FIFO enqueue onto the outbound writer channel (preserving exact keystroke order
    /// from UI threads without per-keystroke async task scheduling).
    pub fn try_send_oneway(&self, msg: PeerMessage, lane: TrafficLane) -> Result<()> {
        self.outbound_tx
            .try_send((msg, lane))
            .map_err(|e| anyhow!("live peer session try_send failed: {e}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitive_path_blocks_credentials_and_allows_code() {
        assert!(is_sensitive_path(Path::new("/home/alice/.ssh/id_ed25519")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.config/coda/daemon.json")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.config/coda/desktop_client_identity.json")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.claude/credentials.json")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.codex/auth.json")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.grok/auth.json")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.antigravity/settings.json")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.gemini/settings.json")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.config/gh/hosts.yml")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.cargo/credentials.toml")));
        assert!(is_sensitive_path(Path::new("/Users/alice/.zsh_history")));
        assert!(is_sensitive_path(Path::new("/Users/alice/infra/terraform.tfstate")));
        assert!(is_sensitive_path(Path::new("/Users/alice/certs/server.key")));
        assert!(is_sensitive_path(Path::new("/Users/alice/project/.env")));
        assert!(is_sensitive_path(Path::new("/etc/shadow")));
        assert!(!is_sensitive_path(Path::new("/Users/alice/project/.env.example")));
        assert!(!is_sensitive_path(Path::new("/Users/alice/project/src/main.rs")));
    }

    #[test]
    fn test_validate_local_preview_target_blocks_privileged_and_host_escape() {
        assert!(validate_local_preview_target(22, "/").is_err());
        assert!(validate_local_preview_target(80, "/").is_err());
        assert!(validate_local_preview_target(5432, "/").is_err());
        assert!(validate_local_preview_target(6379, "/").is_err());
        assert!(validate_local_preview_target(24444, "/").is_err());
        assert!(validate_local_preview_target(3000, "@evil.com/").is_err());
        assert!(validate_local_preview_target(3000, "//evil.com/").is_err());
        let ok = validate_local_preview_target(3000, "api/health").expect("valid");
        assert_eq!(ok.as_str(), "http://127.0.0.1:3000/api/health");
    }

    #[test]
    fn test_upload_filename_and_p2p_ws_origin_security() {
        assert!(sanitize_upload_filename("report_2026.png").is_ok());
        assert!(sanitize_upload_filename("../.bashrc").is_err());
        assert!(sanitize_upload_filename(".env").is_err());
        assert!(sanitize_upload_filename("a/b.txt").is_err());
        assert!(sanitize_upload_filename("a\\b.txt").is_err());

        assert!(is_allowed_p2p_ws_origin("https://codahub.dev", "https://codahub.dev", Some("192.168.1.5:24444")));
        assert!(is_allowed_p2p_ws_origin("http://127.0.0.1:8080", "https://codahub.dev", Some("127.0.0.1:24444")));
        assert!(!is_allowed_p2p_ws_origin("https://evil.example", "https://codahub.dev", Some("192.168.1.5:24444")));
    }

    #[test]
    fn test_utf8_prefix_does_not_split_multibyte_chars() {
        let s = "你好世界abc";
        let bytes = s.as_bytes();
        // Cut right inside the second CJK char (bytes 3..6)
        let prefix = lossy_utf8_prefix(bytes, 4);
        assert_eq!(prefix, "你");
        assert!(!prefix.contains('\u{FFFD}'));
    }

    #[test]
    fn test_atomic_save_and_exclusive_daemon_lock() {
        let dir = std::env::temp_dir().join(format!("coda-daemon-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("daemon.json");

        atomic_write_private_file(&cfg_path, b"{\"ok\":true}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cfg_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let lock1 = try_acquire_daemon_lock(&cfg_path).unwrap();
        assert!(lock1.is_some());
        let lock2 = try_acquire_daemon_lock(&cfg_path).unwrap();
        assert!(lock2.is_none());
        drop(lock1);
        let lock3 = try_acquire_daemon_lock(&cfg_path).unwrap();
        assert!(lock3.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_relay_ticket_exp_unix_standard_base64_and_renewal() {
        let claims = coda_protocol::RelayTicketClaims {
            user_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            peer_device_id: None,
            role: "daemon".into(),
            quota_mbps: 100,
            exp_unix: ((current_unix_ms() / 1000) as i64) + 3 * 86_400,
        };
        let signed = claims
            .sign(b"0123456789abcdef0123456789abcdef")
            .expect("sign ticket");
        // Verify STANDARD base64 (`+`, `/`, `=`) decodes cleanly and triggers 7-day lead renewal
        assert_eq!(parse_relay_ticket_exp_unix(&signed), Some(claims.exp_unix));
        assert!(ticket_needs_renewal(&signed, 7 * 86_400));
        assert!(!ticket_needs_renewal(&signed, 24 * 3600));
    }
}
