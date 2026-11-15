pub mod db;
pub mod relay;

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use axum::{
    extract::{ConnectInfo, Extension, Path, Query, RawQuery, Request, State, WebSocketUpgrade},
    http::{
        header::{self, HeaderName, HeaderValue},
        HeaderMap, Method, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use coda_protocol::{
    validate_ed25519_pub_b64, validate_x25519_pub_b64, RelayTicketClaims, StoredDeviceIdentity,
    TrustedPeerEntry, MAX_WIRE_FRAME_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};
use uuid::Uuid;

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, (u32, Instant)>>,
}

impl RateLimiter {
    async fn check(&self, key: &str, max_requests: u32, window: Duration) -> Result<(), ApiError> {
        let now = Instant::now();
        let mut map = self.buckets.lock().await;
        if map.len() > 2048 {
            map.retain(|_, (_, started)| now.duration_since(*started) < window * 2);
        }
        let entry = map.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= window {
            *entry = (1, now);
            return Ok(());
        }
        if entry.0 >= max_requests {
            return Err(ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                message: "rate limit exceeded; please retry shortly".into(),
            });
        }
        entry.0 += 1;
        Ok(())
    }
}

use crate::{
    db::{
        Db, DeviceAuthPollState, DeviceAuthPublicInfo, DeviceRecord, InviteCodeRecord,
        RelayNodeRecord, UserRecord,
    },
    relay::RelayHub,
};

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub open_registration: bool,
    pub raw_tcp_public_endpoint: Option<String>,
    pub release_version: String,
    pub github_repo: String,
    pub ios_beta_url: Option<String>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            open_registration: false,
            raw_tcp_public_endpoint: None,
            release_version: format!("v{}", env!("CARGO_PKG_VERSION")),
            github_repo: "Annihilater/coda".to_string(),
            ios_beta_url: None,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub relay_hub: Arc<RelayHub>,
    pub relay_secret: Vec<u8>,
    pub public_base_url: String,
    pub has_explicit_public_url: bool,
    pub raw_tcp_public_endpoint: String,
    pub raw_tcp_active: bool,
    pub server_pubkey_b64: String,
    pub open_registration: bool,
    pub release_version: String,
    pub github_repo: String,
    pub ios_beta_url: Option<String>,
    pub http_no_redirect: reqwest::Client,
    pub latest_release_cache: Arc<RwLock<Option<(String, Instant)>>>,
    pub rate_limiter: Arc<RateLimiter>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub handle: String,
    pub display_name: String,
    pub password: String,
    pub invite_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email_or_handle: String,
    pub password: String,
    #[serde(default = "default_client_label")]
    pub client_label: String,
}

fn default_client_label() -> String {
    "apple-or-web".into()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthResponse {
    pub user: UserRecord,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateInviteRequest {
    #[serde(default)]
    pub note: String,
    #[serde(default = "default_invite_uses")]
    pub max_uses: i32,
    #[serde(default = "default_invite_days")]
    pub valid_days: i64,
}
fn default_invite_uses() -> i32 {
    5
}
fn default_invite_days() -> i64 {
    30
}

#[derive(Debug, Deserialize)]
pub struct CreatePairTokenRequest {
    pub suggested_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PairTokenResponse {
    pub code: String,
    pub expires_at: String,
    pub one_liner_command: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimPairRequest {
    pub code: String,
    pub name: String,
    pub platform: String,
    pub ed25519_pubkey: String,
    pub x25519_pubkey: String,
    /// Signature over `code.as_bytes()` proving possession of `ed25519_pubkey`
    pub signature_b64: String,
    #[serde(default)]
    pub endpoints: Value,
    pub herdr_core_id: Option<String>,
    pub herdr_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimPairResponse {
    pub device: DeviceRecord,
    pub owner_handle: String,
    pub daemon_relay_ticket: String,
    pub relays: Vec<RelayNodeRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterClientDeviceRequest {
    pub name: String,
    pub platform: String,
    pub ed25519_pubkey: String,
    pub x25519_pubkey: String,
    #[serde(default)]
    pub endpoints: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MintRelayTicketRequest {
    pub device_id: Uuid,
    pub peer_device_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MintRelayTicketResponse {
    pub ticket: String,
    pub expires_in_secs: i64,
    pub relays: Vec<RelayNodeRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartDeviceAuthRequest {
    pub device_name: String,
    #[serde(default = "default_daemon_role")]
    pub role: String,
    #[serde(default = "default_linux_platform")]
    pub platform: String,
    pub ed25519_pubkey: String,
    pub x25519_pubkey: String,
    #[serde(default)]
    pub endpoints: Value,
    pub herdr_core_id: Option<String>,
    pub herdr_version: Option<String>,
}

fn default_daemon_role() -> String {
    "daemon".into()
}
fn default_linux_platform() -> String {
    "linux".into()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartDeviceAuthResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in_secs: i64,
    pub interval_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct DeviceAuthInfoQuery {
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct ApproveDeviceAuthRequest {
    pub user_code: String,
    pub device_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PollDeviceAuthRequest {
    pub device_code: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PollDeviceAuthResponse {
    pub state: String,
    pub expires_in_secs: Option<i64>,
    pub session_token: Option<String>,
    pub user: Option<UserRecord>,
    pub owner_handle: Option<String>,
    pub device: Option<DeviceRecord>,
    pub daemon_relay_ticket: Option<String>,
    pub relays: Option<Vec<RelayNodeRecord>>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollHostDeviceRequest {
    pub name: String,
    #[serde(default = "default_daemon_role")]
    pub role: String,
    #[serde(default = "default_linux_platform")]
    pub platform: String,
    pub ed25519_pubkey: String,
    pub x25519_pubkey: String,
    #[serde(default)]
    pub endpoints: Value,
    pub herdr_core_id: Option<String>,
    pub herdr_version: Option<String>,
}

async fn cache_control_middleware(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let is_hashed_bundle = path.starts_with("/app.")
        && path.ends_with(".js")
        && path.len() > 8
        && path[5..path.len() - 3]
            .chars()
            .all(|c| c.is_ascii_hexdigit());
    let is_static_asset = path.starts_with("/assets/") && !path.contains("..");
    let mut resp = next.run(req).await;
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        return resp;
    }
    let is_ok_hashed = is_hashed_bundle && resp.status() == StatusCode::OK;
    let is_ok_asset = is_static_asset && resp.status() == StatusCode::OK;
    let headers = resp.headers_mut();
    if is_ok_hashed {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else if is_ok_asset {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=86400"),
        );
    } else {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        );
        headers.insert(
            HeaderName::from_static("cdn-cache-control"),
            HeaderValue::from_static("no-store"),
        );
    }
    resp
}

pub fn build_router(state: AppState, web_dist_dir: Option<PathBuf>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
        ]);

    let api = Router::new()
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/public/site-info", get(public_site_info_handler))
        .route("/api/v1/auth/register", post(register_handler))
        .route("/api/v1/auth/login", post(login_handler))
        .route("/api/v1/auth/logout", post(logout_handler))
        .route("/api/v1/auth/me", get(me_handler))
        .route("/api/v1/auth/device/start", post(start_device_auth_handler))
        .route("/api/v1/auth/device/info", get(device_auth_info_handler))
        .route(
            "/api/v1/auth/device/approve",
            post(approve_device_auth_handler),
        )
        .route("/api/v1/auth/device/poll", post(poll_device_auth_handler))
        .route(
            "/api/v1/invites",
            get(list_invites_handler).post(create_invite_handler),
        )
        .route("/api/v1/devices", get(list_devices_handler))
        .route("/api/v1/devices/{id}", delete(delete_device_handler))
        .route("/api/v1/devices/pair-token", post(create_pair_token_handler))
        .route("/api/v1/devices/claim-pair", post(claim_pair_handler))
        .route(
            "/api/v1/devices/register-client",
            post(register_client_device_handler),
        )
        .route(
            "/api/v1/devices/enroll-host",
            post(enroll_host_device_handler),
        )
        .route("/api/v1/relay/ticket", post(mint_relay_ticket_handler))
        .route("/api/v1/relay/trusted-peers", get(trusted_peers_handler))
        .route("/download/{artifact}", get(download_latest_artifact_handler))
        .route("/install.sh", get(install_script_handler))
        .route("/v1/tunnel/ws", get(ws_tunnel_upgrade_handler))
        .route("/api/v1/events/ws", get(ws_account_events_handler))
        .with_state(state);

    let app = if let Some(dir) = web_dist_dir.filter(|p| p.exists()) {
        let index = dir.join("index.html");
        api.route_service("/login", ServeFile::new(&index))
            .route_service("/register", ServeFile::new(&index))
            .route_service("/activate", ServeFile::new(&index))
            .route_service("/console", ServeFile::new(&index))
            .fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)))
    } else {
        api
    };

    app.layer(cors)
        .layer(middleware::from_fn(cache_control_middleware))
}

pub struct RunningServer {
    pub http_addr: SocketAddr,
    pub raw_tcp_addr: SocketAddr,
    pub state: AppState,
}

/// Starts both the HTTP/CDN-WSS Coordinator and the Domestic No-ICP Raw TCP Relay Listener.
pub async fn start_full_server(
    database_url: &str,
    http_bind: SocketAddr,
    raw_tcp_bind: SocketAddr,
    relay_secret: Vec<u8>,
    public_base_url: Option<String>,
    web_dist_dir: Option<PathBuf>,
) -> Result<RunningServer> {
    start_full_server_with_options(
        database_url,
        http_bind,
        raw_tcp_bind,
        relay_secret,
        public_base_url,
        web_dist_dir,
        ServerOptions::default(),
    )
    .await
}

pub async fn start_full_server_with_options(
    database_url: &str,
    http_bind: SocketAddr,
    raw_tcp_bind: SocketAddr,
    relay_secret: Vec<u8>,
    public_base_url: Option<String>,
    web_dist_dir: Option<PathBuf>,
    options: ServerOptions,
) -> Result<RunningServer> {
    let db = Db::connect_and_migrate_with_secret(database_url, &relay_secret).await?;
    let relay_hub = RelayHub::new(
        relay_secret.clone(),
        "Coda-Hybrid-Edge",
        Some(db.clone()),
    );

    let raw_tcp_addr = relay_hub.spawn_raw_tcp_listener(raw_tcp_bind).await?;
    let http_listener = tokio::net::TcpListener::bind(http_bind).await?;
    let http_addr = http_listener.local_addr()?;

    let explicit_public = public_base_url
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());
    let has_explicit_public_url = explicit_public.is_some();
    let base_url = explicit_public.unwrap_or_else(|| format!("http://{http_addr}"));
    let wss_url = http_to_ws_url(&base_url) + "/v1/tunnel/ws";

    let explicit_raw_tcp = options
        .raw_tcp_public_endpoint
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let raw_tcp_active = if explicit_raw_tcp.is_some() {
        true
    } else {
        !is_remote_public_url(&base_url)
    };
    let raw_tcp_endpoint =
        explicit_raw_tcp.unwrap_or_else(|| format!("127.0.0.1:{}", raw_tcp_addr.port()));

    let node_id = StoredDeviceIdentity::generate("coda-relay-node");
    db.seed_smart_relays(
        &raw_tcp_endpoint,
        raw_tcp_active,
        &wss_url,
        &node_id.ed25519_public_b64,
    )
    .await?;

    let http_no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(4))
        .user_agent("coda-server")
        .build()?;

    let state = AppState {
        db,
        relay_hub,
        relay_secret,
        public_base_url: base_url,
        has_explicit_public_url,
        raw_tcp_public_endpoint: raw_tcp_endpoint,
        raw_tcp_active,
        server_pubkey_b64: node_id.ed25519_public_b64,
        open_registration: options.open_registration,
        release_version: options.release_version,
        github_repo: options.github_repo,
        ios_beta_url: options.ios_beta_url.filter(|s| !s.trim().is_empty()),
        http_no_redirect,
        latest_release_cache: Arc::new(RwLock::new(None)),
        rate_limiter: Arc::new(RateLimiter::default()),
    };

    let router = build_router(state.clone(), web_dist_dir);
    tokio::spawn(async move {
        let _ = axum::serve(
            http_listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    Ok(RunningServer {
        http_addr,
        raw_tcp_addr,
        state,
    })
}

fn http_to_ws_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("wss://{trimmed}")
    }
}

fn is_loopback_host_or_endpoint(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    let without_scheme = v
        .strip_prefix("https://")
        .or_else(|| v.strip_prefix("http://"))
        .or_else(|| v.strip_prefix("wss://"))
        .or_else(|| v.strip_prefix("ws://"))
        .unwrap_or(&v);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host = if host_port.starts_with('[') {
        host_port
            .split(']')
            .next()
            .unwrap_or(host_port)
            .trim_start_matches('[')
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    matches!(host, "127.0.0.1" | "localhost" | "0.0.0.0" | "::1")
}

fn is_remote_public_url(base_url: &str) -> bool {
    base_url.starts_with("https://") || !is_loopback_host_or_endpoint(base_url)
}

fn is_valid_http_host_authority(host: &str) -> bool {
    let h = host.trim();
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    if h.bytes().any(|b| {
        b.is_ascii_control()
            || matches!(
                b,
                b' ' | b'\t'
                    | b'/'
                    | b'\\'
                    | b'\''
                    | b'"'
                    | b'`'
                    | b'$'
                    | b';'
                    | b'?'
                    | b'#'
                    | b'@'
                    | b'%'
                    | b'<'
                    | b'>'
            )
    }) {
        return false;
    }
    if let Some(rest) = h.strip_prefix('[') {
        let Some((ipv6, tail)) = rest.split_once(']') else {
            return false;
        };
        if ipv6.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        if tail.is_empty() {
            return true;
        }
        if let Some(port) = tail.strip_prefix(':') {
            return !port.is_empty() && port.parse::<u16>().is_ok();
        }
        return false;
    }
    let (hostname, port_opt) = match h.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') => (name, Some(port)),
        _ => (h, None),
    };
    if let Some(port) = port_opt {
        if port.is_empty() || port.parse::<u16>().is_err() {
            return false;
        }
    }
    !hostname.is_empty()
        && !hostname.starts_with('.')
        && !hostname.ends_with('.')
        && hostname
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

fn is_shell_safe_base_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    !rest.contains('/') && is_valid_http_host_authority(rest)
}

fn effective_public_base_url(st: &AppState, headers: &HeaderMap) -> String {
    if st.has_explicit_public_url {
        return st.public_base_url.clone();
    }
    let host_raw = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .or_else(|| headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(host) = host_raw.filter(|h| is_valid_http_host_authority(h)) else {
        return st.public_base_url.clone();
    };

    let cf_https = headers
        .get("cf-visitor")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("https"))
        .unwrap_or(false);
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s == "http" || s == "https")
        .unwrap_or_else(|| {
            if cf_https || (!is_loopback_host_or_endpoint(host) && !host.contains(':')) {
                "https".to_string()
            } else {
                "http".to_string()
            }
        });
    let candidate = format!("{proto}://{host}");
    if is_shell_safe_base_url(&candidate) {
        candidate
    } else {
        st.public_base_url.clone()
    }
}

fn client_rate_key(
    conn: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: &HeaderMap,
) -> String {
    if let Some(Extension(ConnectInfo(addr))) = conn {
        return addr.ip().to_string();
    }
    headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

async fn effective_relays_for_request(
    st: &AppState,
    headers: &HeaderMap,
) -> Result<Vec<RelayNodeRecord>, ApiError> {
    let base_url = effective_public_base_url(st, headers);
    let remote = is_remote_public_url(&base_url);
    let expected_wss = http_to_ws_url(&base_url) + "/v1/tunnel/ws";
    let mut relays = st
        .db
        .list_active_relays()
        .await
        .map_err(ApiError::internal)?;

    if remote {
        relays.retain(|r| {
            !(r.mode == "raw_tcp_no_sni" && is_loopback_host_or_endpoint(&r.endpoint))
        });
        for r in &mut relays {
            if r.id == "global-cdn-wss" && !st.has_explicit_public_url {
                r.endpoint = expected_wss.clone();
            }
        }
    }
    Ok(relays)
}

// ============================================================================
// HTTP Handlers
// ============================================================================

async fn health_handler(State(st): State<AppState>) -> Response {
    if let Err(e) = st.db.ping().await {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "service": "coda-server",
                "version": st.release_version,
                "error": format!("database_unreachable: {e}"),
            })),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "service": "coda-server",
            "version": st.release_version,
            "open_registration": st.open_registration,
            "zero_knowledge_e2ee": true,
            "raw_tcp_active": st.raw_tcp_active,
            "raw_tcp_no_sni_endpoint": st.raw_tcp_public_endpoint,
            "server_pubkey": st.server_pubkey_b64,
        })),
    )
        .into_response()
}

const LATEST_RELEASE_CACHE_TTL: Duration = Duration::from_secs(300);

fn extract_release_tag_from_location(location: &str) -> Option<String> {
    let (_, after) = location.split_once("/releases/tag/")?;
    let raw_tag = after
        .split(['?', '#'])
        .next()
        .unwrap_or(after)
        .trim_end_matches('/');
    if raw_tag.is_empty()
        || !raw_tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    {
        return None;
    }
    Some(raw_tag.to_string())
}

fn release_asset_filename(artifact: &str, tag: &str) -> Option<String> {
    let filename = match artifact {
        "desktop-macos.dmg" => format!("Coda-Desktop-macos-aarch64-{tag}.dmg"),
        "desktop-macos.tar.gz" => format!("coda-macos-aarch64-{tag}.tar.gz"),
        "daemon-linux-x86_64.tar.gz" => format!("coda-daemon-linux-x86_64-{tag}.tar.gz"),
        "daemon-linux-aarch64.tar.gz" => format!("coda-daemon-linux-aarch64-{tag}.tar.gz"),
        "daemon-macos-aarch64.tar.gz" => format!("coda-daemon-macos-aarch64-{tag}.tar.gz"),
        "server-linux-x86_64.tar.gz" => format!("coda-server-linux-x86_64-{tag}.tar.gz"),
        "server-linux-aarch64.tar.gz" => format!("coda-server-linux-aarch64-{tag}.tar.gz"),
        "server-macos-aarch64.tar.gz" => format!("coda-macos-aarch64-{tag}.tar.gz"),
        "deploy-bundle.tar.gz" => format!("coda-deploy-bundle-{tag}.tar.gz"),
        "checksums.txt" => format!("SHA256SUMS-{tag}.txt"),
        _ => return None,
    };
    Some(filename)
}

async fn resolve_latest_github_tag(st: &AppState) -> Option<String> {
    {
        let cached = st.latest_release_cache.read().await;
        if let Some((tag, fetched_at)) = cached.as_ref() {
            if fetched_at.elapsed() < LATEST_RELEASE_CACHE_TTL {
                return Some(tag.clone());
            }
        }
    }

    let latest_url = format!("https://github.com/{}/releases/latest", st.github_repo);
    let mut resolved = None;

    if let Ok(resp) = st.http_no_redirect.head(&latest_url).send().await {
        if let Some(loc) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
        {
            resolved = extract_release_tag_from_location(loc);
        }
    }

    if resolved.is_none() {
        if let Ok(resp) = st.http_no_redirect.get(&latest_url).send().await {
            if let Some(loc) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            {
                resolved = extract_release_tag_from_location(loc);
            }
        }
    }

    if let Some(tag) = resolved {
        let mut cached = st.latest_release_cache.write().await;
        *cached = Some((tag.clone(), Instant::now()));
        return Some(tag);
    }

    let cached = st.latest_release_cache.read().await;
    cached.as_ref().map(|(tag, _)| tag.clone())
}

async fn download_latest_artifact_handler(
    State(st): State<AppState>,
    Path(artifact): Path<String>,
) -> Response {
    if release_asset_filename(&artifact, "v0.0.0").is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "unknown download artifact" })),
        )
            .into_response();
    }

    let fallback_tag = if st.release_version.starts_with('v') {
        st.release_version.clone()
    } else {
        format!("v{}", st.release_version)
    };
    let tag = resolve_latest_github_tag(&st).await.unwrap_or(fallback_tag);
    let filename = release_asset_filename(&artifact, &tag).expect("validated artifact key");
    let target_url = format!(
        "https://github.com/{}/releases/download/{tag}/{filename}",
        st.github_repo
    );

    (StatusCode::FOUND, [(header::LOCATION, target_url)]).into_response()
}

async fn public_site_info_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let base_url = effective_public_base_url(&st, &headers);
    let has_any_users = st.db.has_any_users().await.map_err(ApiError::internal)?;
    let relays = effective_relays_for_request(&st, &headers).await?;
    let tag = if st.release_version.starts_with('v') {
        st.release_version.clone()
    } else {
        format!("v{}", st.release_version)
    };
    let dl_root = format!("{base_url}/download");
    let releases_page = format!("https://github.com/{}/releases/latest", st.github_repo);

    Ok(Json(json!({
        "service": "coda-server",
        "version": tag,
        "open_registration": st.open_registration,
        "has_any_users": has_any_users,
        "public_base_url": base_url,
        "zero_knowledge_e2ee": true,
        "install_command": format!("curl -fsSL {base_url}/install.sh | sh"),
        "relays": relays,
        "downloads": {
            "daemon_linux_x64": format!("{dl_root}/daemon-linux-x86_64.tar.gz"),
            "daemon_linux_arm64": format!("{dl_root}/daemon-linux-aarch64.tar.gz"),
            "daemon_macos_arm64": format!("{dl_root}/daemon-macos-aarch64.tar.gz"),
            "server_linux_x64": format!("{dl_root}/server-linux-x86_64.tar.gz"),
            "server_linux_arm64": format!("{dl_root}/server-linux-aarch64.tar.gz"),
            "server_macos_arm64": format!("{dl_root}/server-macos-aarch64.tar.gz"),
            "desktop_macos_dmg": format!("{dl_root}/desktop-macos.dmg"),
            "desktop_macos_tar": format!("{dl_root}/desktop-macos.tar.gz"),
            "deploy_bundle": format!("{dl_root}/deploy-bundle.tar.gz"),
            "ios_beta_url": st.ios_beta_url,
            "releases_page": releases_page,
        }
    })))
}

async fn register_handler(
    State(st): State<AppState>,
    conn: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let ip = client_rate_key(conn, &headers);
    st.rate_limiter
        .check(&format!("register:{ip}"), 20, Duration::from_secs(60))
        .await?;
    let (user, token) = st
        .db
        .register_user_with_policy(
            &req.email,
            &req.handle,
            &req.display_name,
            &req.password,
            req.invite_code.as_deref(),
            st.open_registration,
        )
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(AuthResponse { user, token }))
}

async fn login_handler(
    State(st): State<AppState>,
    conn: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let ip = client_rate_key(conn, &headers);
    st.rate_limiter
        .check(&format!("login:{ip}"), 30, Duration::from_secs(60))
        .await?;
    let (user, token) = st
        .db
        .login_user(&req.email_or_handle, &req.password, &req.client_label)
        .await
        .map_err(ApiError::unauthorized)?;
    Ok(Json(AuthResponse { user, token }))
}

async fn logout_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    if let Some(auth_hdr) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let token = auth_hdr.strip_prefix("Bearer ").unwrap_or(auth_hdr);
        let _ = st.db.delete_session(token).await;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn me_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<UserRecord>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    Ok(Json(user))
}

async fn create_invite_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateInviteRequest>,
) -> Result<Json<InviteCodeRecord>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let rec = st
        .db
        .create_invite_code(&user, &req.note, req.max_uses, req.valid_days)
        .await
        .map_err(ApiError::forbidden)?;
    Ok(Json(rec))
}

async fn list_invites_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<InviteCodeRecord>>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let list = st
        .db
        .list_invite_codes(&user)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(list))
}

async fn list_devices_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceRecord>>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let devices = st
        .db
        .list_user_devices(user.id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(devices))
}

async fn delete_device_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(device_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let deleted = st
        .db
        .delete_device(user.id, device_id)
        .await
        .map_err(ApiError::internal)?;
    if !deleted {
        return Err(ApiError {
            status: StatusCode::NOT_FOUND,
            message: "device not found".into(),
        });
    }
    st.relay_hub.disconnect_device(user.id, device_id).await;
    st.relay_hub
        .notify_device_enrolled(user.id, device_id, "", "device", Some(false))
        .await;
    Ok(Json(json!({ "ok": true, "deleted": true, "device_id": device_id })))
}

async fn trusted_peers_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TrustedPeerEntry>>, ApiError> {
    let user_id = if headers.contains_key(axum::http::header::AUTHORIZATION) {
        require_auth(&st, &headers).await?.id
    } else if let Some(ticket) = headers
        .get("x-coda-relay-ticket")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let claims = RelayTicketClaims::verify(ticket, &st.relay_secret, Utc::now().timestamp())
            .map_err(ApiError::unauthorized)?;
        let dev = st
            .db
            .get_device_by_id(claims.device_id)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::unauthorized(anyhow::anyhow!("device revoked")))?;
        if dev.user_id != claims.user_id {
            return Err(ApiError::unauthorized(anyhow::anyhow!(
                "device tenant mismatch"
            )));
        }
        claims.user_id
    } else {
        return Err(ApiError::unauthorized(anyhow::anyhow!(
            "missing Authorization header or X-Coda-Relay-Ticket"
        )));
    };
    let peers = st
        .db
        .list_trusted_peers(user_id)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(peers))
}

async fn create_pair_token_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreatePairTokenRequest>,
) -> Result<Json<PairTokenResponse>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let base_url = effective_public_base_url(&st, &headers);
    let (code, expires_at) = st
        .db
        .create_pairing_token(user.id, req.suggested_name.as_deref())
        .await
        .map_err(ApiError::internal)?;
    let one_liner_command = format!(
        "curl -fsSL {}/install.sh | sh -s -- --server {} --code {}",
        base_url, base_url, code
    );
    Ok(Json(PairTokenResponse {
        code,
        expires_at: expires_at.to_rfc3339(),
        one_liner_command,
    }))
}

async fn install_script_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let base_url = effective_public_base_url(&st, &headers);
    let script = include_str!("../../../deploy/install-daemon.sh")
        .replace("__CODA_DEFAULT_SERVER_URL__", &base_url);
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/x-shellscript; charset=utf-8",
        )],
        script,
    )
}

async fn start_device_auth_handler(
    State(st): State<AppState>,
    conn: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(req): Json<StartDeviceAuthRequest>,
) -> Result<Json<StartDeviceAuthResponse>, ApiError> {
    let ip = client_rate_key(conn, &headers);
    st.rate_limiter
        .check(&format!("dev_start:{ip}"), 30, Duration::from_secs(60))
        .await?;
    let role = match req.role.as_str() {
        "daemon" | "client" | "hybrid" => req.role.as_str(),
        _ => {
            return Err(ApiError::bad_request(anyhow::anyhow!(
                "invalid device role; expected daemon, client, or hybrid"
            )))
        }
    };
    validate_ed25519_pub_b64(&req.ed25519_pubkey).map_err(ApiError::bad_request)?;
    validate_x25519_pub_b64(&req.x25519_pubkey).map_err(ApiError::bad_request)?;
    let endpoints = if req.endpoints.is_null() {
        json!([])
    } else {
        req.endpoints
    };
    let ttl_secs = 600i64;
    let (device_code, user_code, _expires_at) = st
        .db
        .create_device_auth_request(
            &req.device_name,
            role,
            &req.platform,
            &req.ed25519_pubkey,
            &req.x25519_pubkey,
            &endpoints,
            req.herdr_core_id.as_deref().unwrap_or(""),
            req.herdr_version.as_deref().unwrap_or(""),
            ttl_secs,
        )
        .await
        .map_err(ApiError::bad_request)?;

    let base_url = effective_public_base_url(&st, &headers);
    let verification_uri = format!("{base_url}/activate");
    let verification_uri_complete = format!("{verification_uri}?code={user_code}");
    Ok(Json(StartDeviceAuthResponse {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in_secs: ttl_secs,
        interval_secs: 2,
    }))
}

async fn device_auth_info_handler(
    State(st): State<AppState>,
    Query(q): Query<DeviceAuthInfoQuery>,
) -> Result<Json<DeviceAuthPublicInfo>, ApiError> {
    let info = st
        .db
        .get_device_auth_info(&q.code)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::bad_request(anyhow::anyhow!("authorization code not found")))?;
    Ok(Json(info))
}

async fn approve_device_auth_handler(
    State(st): State<AppState>,
    conn: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(req): Json<ApproveDeviceAuthRequest>,
) -> Result<Json<DeviceRecord>, ApiError> {
    let ip = client_rate_key(conn, &headers);
    st.rate_limiter
        .check(&format!("dev_approve:{ip}"), 30, Duration::from_secs(60))
        .await?;
    let user = require_auth(&st, &headers).await?;
    let device = st
        .db
        .approve_device_auth(user.id, &req.user_code, req.device_name.as_deref())
        .await
        .map_err(ApiError::bad_request)?;
    st.relay_hub
        .notify_device_enrolled(
            user.id,
            device.id,
            &device.name,
            &device.role,
            Some(device.is_online),
        )
        .await;
    Ok(Json(device))
}

async fn poll_device_auth_handler(
    State(st): State<AppState>,
    conn: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(req): Json<PollDeviceAuthRequest>,
) -> Result<Json<PollDeviceAuthResponse>, ApiError> {
    let ip = client_rate_key(conn, &headers);
    st.rate_limiter
        .check(&format!("dev_poll:{ip}"), 90, Duration::from_secs(60))
        .await?;
    let poll = st
        .db
        .poll_device_auth(&req.device_code)
        .await
        .map_err(ApiError::internal)?;

    match poll {
        DeviceAuthPollState::Pending { expires_in_secs } => Ok(Json(PollDeviceAuthResponse {
            state: "pending".into(),
            expires_in_secs: Some(expires_in_secs),
            session_token: None,
            user: None,
            owner_handle: None,
            device: None,
            daemon_relay_ticket: None,
            relays: None,
        })),
        DeviceAuthPollState::Expired => Ok(Json(PollDeviceAuthResponse {
            state: "expired".into(),
            expires_in_secs: Some(0),
            session_token: None,
            user: None,
            owner_handle: None,
            device: None,
            daemon_relay_ticket: None,
            relays: None,
        })),
        DeviceAuthPollState::Approved {
            session_token,
            user,
            device,
        } => {
            let exp_unix = Utc::now().timestamp() + 30 * 86_400;
            let claims = RelayTicketClaims {
                user_id: user.id,
                device_id: device.id,
                peer_device_id: None,
                role: device.role.clone(),
                quota_mbps: user.bandwidth_quota_mbps.max(1) as u32,
                exp_unix,
            };
            let daemon_relay_ticket = claims
                .sign(&st.relay_secret)
                .map_err(ApiError::internal)?;
            let relays = effective_relays_for_request(&st, &headers).await?;
            let owner_handle = user.handle.clone();
            st.relay_hub
                .notify_device_enrolled(
                    user.id,
                    device.id,
                    &device.name,
                    &device.role,
                    Some(device.is_online),
                )
                .await;
            Ok(Json(PollDeviceAuthResponse {
                state: "approved".into(),
                expires_in_secs: None,
                session_token: Some(session_token),
                user: Some(user),
                owner_handle: Some(owner_handle),
                device: Some(device),
                daemon_relay_ticket: Some(daemon_relay_ticket),
                relays: Some(relays),
            }))
        }
    }
}

async fn enroll_host_device_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<EnrollHostDeviceRequest>,
) -> Result<Json<ClaimPairResponse>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let role = match req.role.as_str() {
        "daemon" | "hybrid" => req.role.as_str(),
        _ => {
            return Err(ApiError::bad_request(anyhow::anyhow!(
                "role must be daemon or hybrid"
            )))
        }
    };
    let endpoints = if req.endpoints.is_null() {
        json!([])
    } else {
        req.endpoints
    };
    let device = st
        .db
        .upsert_device(
            user.id,
            &req.name,
            role,
            &req.platform,
            &req.ed25519_pubkey,
            &req.x25519_pubkey,
            &endpoints,
            req.herdr_core_id.as_deref(),
            req.herdr_version.as_deref(),
        )
        .await
        .map_err(ApiError::bad_request)?;

    let exp_unix = Utc::now().timestamp() + 30 * 86_400;
    let claims = RelayTicketClaims {
        user_id: user.id,
        device_id: device.id,
        peer_device_id: None,
        role: device.role.clone(),
        quota_mbps: user.bandwidth_quota_mbps.max(1) as u32,
        exp_unix,
    };
    let daemon_relay_ticket = claims
        .sign(&st.relay_secret)
        .map_err(ApiError::internal)?;
    let relays = effective_relays_for_request(&st, &headers).await?;

    st.relay_hub
        .notify_device_enrolled(
            user.id,
            device.id,
            &device.name,
            &device.role,
            Some(device.is_online),
        )
        .await;

    Ok(Json(ClaimPairResponse {
        device,
        owner_handle: user.handle,
        daemon_relay_ticket,
        relays,
    }))
}

async fn claim_pair_handler(
    State(st): State<AppState>,
    conn: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(req): Json<ClaimPairRequest>,
) -> Result<Json<ClaimPairResponse>, ApiError> {
    let ip = client_rate_key(conn, &headers);
    st.rate_limiter
        .check(&format!("claim_pair:{ip}"), 20, Duration::from_secs(60))
        .await?;
    validate_ed25519_pub_b64(&req.ed25519_pubkey).map_err(ApiError::bad_request)?;
    validate_x25519_pub_b64(&req.x25519_pubkey).map_err(ApiError::bad_request)?;
    StoredDeviceIdentity::verify_signature(
        &req.ed25519_pubkey,
        req.code.trim().as_bytes(),
        &req.signature_b64,
    )
    .map_err(ApiError::bad_request)?;

    let endpoints = if req.endpoints.is_null() {
        json!([])
    } else {
        req.endpoints
    };

    let (device, owner) = st
        .db
        .claim_pairing_token(
            &req.code,
            &req.name,
            &req.platform,
            &req.ed25519_pubkey,
            &req.x25519_pubkey,
            &endpoints,
            req.herdr_core_id.as_deref(),
            req.herdr_version.as_deref(),
        )
        .await
        .map_err(ApiError::bad_request)?;

    // Issue a 30-day daemon ticket so the paired server can stay connected continuously
    let exp_unix = Utc::now().timestamp() + 30 * 86_400;
    let claims = RelayTicketClaims {
        user_id: owner.id,
        device_id: device.id,
        peer_device_id: None,
        role: "daemon".into(),
        quota_mbps: owner.bandwidth_quota_mbps.max(1) as u32,
        exp_unix,
    };
    let daemon_relay_ticket = claims
        .sign(&st.relay_secret)
        .map_err(ApiError::internal)?;
    let relays = effective_relays_for_request(&st, &headers).await?;

    st.relay_hub
        .notify_device_enrolled(
            owner.id,
            device.id,
            &device.name,
            &device.role,
            Some(device.is_online),
        )
        .await;

    Ok(Json(ClaimPairResponse {
        device,
        owner_handle: owner.handle,
        daemon_relay_ticket,
        relays,
    }))
}

async fn register_client_device_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RegisterClientDeviceRequest>,
) -> Result<Json<DeviceRecord>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let endpoints = if req.endpoints.is_null() {
        json!([])
    } else {
        req.endpoints
    };
    let dev = st
        .db
        .upsert_device(
            user.id,
            &req.name,
            "client",
            &req.platform,
            &req.ed25519_pubkey,
            &req.x25519_pubkey,
            &endpoints,
            None,
            None,
        )
        .await
        .map_err(ApiError::bad_request)?;
    st.relay_hub
        .notify_device_enrolled(user.id, dev.id, &dev.name, &dev.role, Some(dev.is_online))
        .await;
    Ok(Json(dev))
}

async fn mint_relay_ticket_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MintRelayTicketRequest>,
) -> Result<Json<MintRelayTicketResponse>, ApiError> {
    let user = require_auth(&st, &headers).await?;

    let src = st
        .db
        .get_device_by_id(req.device_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::forbidden(anyhow::anyhow!("source device not found")))?;
    if src.user_id != user.id {
        return Err(ApiError::forbidden(anyhow::anyhow!(
            "source device belongs to another tenant"
        )));
    }

    if let Some(peer_id) = req.peer_device_id {
        let dst = st
            .db
            .get_device_by_id(peer_id)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| ApiError::forbidden(anyhow::anyhow!("target device not found")))?;
        if dst.user_id != user.id {
            return Err(ApiError::forbidden(anyhow::anyhow!(
                "cross-tenant access denied: target device belongs to another user"
            )));
        }
    }

    let ttl_secs = 3600i64;
    let claims = RelayTicketClaims {
        user_id: user.id,
        device_id: src.id,
        peer_device_id: req.peer_device_id,
        role: src.role,
        quota_mbps: user.bandwidth_quota_mbps.max(1) as u32,
        exp_unix: Utc::now().timestamp() + ttl_secs,
    };
    let ticket = claims
        .sign(&st.relay_secret)
        .map_err(ApiError::internal)?;
    let relays = effective_relays_for_request(&st, &headers).await?;

    Ok(Json(MintRelayTicketResponse {
        ticket,
        expires_in_secs: ttl_secs,
        relays,
    }))
}

async fn ws_tunnel_upgrade_handler(
    ws: WebSocketUpgrade,
    State(st): State<AppState>,
) -> impl IntoResponse {
    let hub = Arc::clone(&st.relay_hub);
    ws.max_message_size(MAX_WIRE_FRAME_BYTES)
        .max_frame_size(MAX_WIRE_FRAME_BYTES)
        .on_upgrade(move |socket| hub.handle_ws_socket(socket))
}

async fn ws_account_events_handler(
    ws: WebSocketUpgrade,
    State(st): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, ApiError> {
    if let Some(q) = raw_query.as_deref() {
        let has_query_token = q.split('&').any(|pair| {
            pair.split_once('=')
                .map(|(k, v)| k == "token" && !v.is_empty())
                .unwrap_or(false)
        });
        if has_query_token {
            return Err(ApiError::unauthorized(anyhow::anyhow!(
                "query parameter token auth is disabled; use Authorization header or WebSocket auth"
            )));
        }
    }

    let header_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or(Some(s)))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let subproto_hdr = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let subproto_token = subproto_hdr
        .split(',')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("coda-bearer."))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let ws = ws
        .max_message_size(MAX_WIRE_FRAME_BYTES)
        .max_frame_size(MAX_WIRE_FRAME_BYTES);
    let ws = if subproto_hdr.split(',').any(|p| p.trim() == "coda-auth") {
        ws.protocols(["coda-auth"])
    } else {
        ws
    };

    if let Some(token) = header_token.or(subproto_token) {
        let user = st
            .db
            .authenticate_token(&token)
            .await
            .map_err(ApiError::unauthorized)?;
        let hub = Arc::clone(&st.relay_hub);
        return Ok(ws.on_upgrade(move |socket| async move {
            hub.handle_account_events_ws(socket, user.id).await
        }));
    }

    // Browser WebSocket without custom headers: authenticate via first JSON frame `{"type":"auth","token":"..."}`
    let hub = Arc::clone(&st.relay_hub);
    let db = st.db.clone();
    Ok(ws.on_upgrade(move |socket| async move {
        hub.handle_account_events_ws_unauthenticated(socket, db).await
    }))
}

async fn require_auth(st: &AppState, headers: &HeaderMap) -> Result<UserRecord, ApiError> {
    let auth_hdr = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized(anyhow::anyhow!("missing Authorization header")))?;
    let token = auth_hdr
        .strip_prefix("Bearer ")
        .unwrap_or(auth_hdr);
    st.db
        .authenticate_token(token)
        .await
        .map_err(ApiError::unauthorized)
}

pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(e: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: e.to_string(),
        }
    }
    fn unauthorized(e: anyhow::Error) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: e.to_string(),
        }
    }
    fn forbidden(e: anyhow::Error) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: e.to_string(),
        }
    }
    fn internal(e: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(json!({
                "ok": false,
                "error": self.message,
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_release_tag_handles_valid_and_invalid_locations() {
        assert_eq!(
            extract_release_tag_from_location("https://github.com/Annihilater/coda/releases/tag/v0.3.5"),
            Some("v0.3.5".to_string())
        );
        assert_eq!(
            extract_release_tag_from_location("/Annihilater/coda/releases/tag/v0.3.6-rc.1/?foo=bar#top"),
            Some("v0.3.6-rc.1".to_string())
        );
        assert_eq!(
            extract_release_tag_from_location("https://github.com/Annihilater/coda/releases/tag/"),
            None
        );
        assert_eq!(
            extract_release_tag_from_location("https://github.com/Annihilater/coda/releases/tag/bad/path"),
            None
        );
    }

    #[test]
    fn release_asset_filename_maps_known_artifacts() {
        assert_eq!(
            release_asset_filename("desktop-macos.dmg", "v0.3.5").as_deref(),
            Some("Coda-Desktop-macos-aarch64-v0.3.5.dmg")
        );
        assert_eq!(
            release_asset_filename("daemon-linux-x86_64.tar.gz", "v0.3.5").as_deref(),
            Some("coda-daemon-linux-x86_64-v0.3.5.tar.gz")
        );
        assert_eq!(
            release_asset_filename("deploy-bundle.tar.gz", "v0.3.5").as_deref(),
            Some("coda-deploy-bundle-v0.3.5.tar.gz")
        );
        assert_eq!(release_asset_filename("unknown.bin", "v0.3.5"), None);
    }

    #[test]
    fn host_authority_sanitizer_rejects_injection() {
        assert!(is_valid_http_host_authority("coda.example.com"));
        assert!(is_valid_http_host_authority("coda.example.com:8443"));
        assert!(is_valid_http_host_authority("[::1]:8080"));
        assert!(is_valid_http_host_authority("127.0.0.1:8080"));

        assert!(!is_valid_http_host_authority("evil.com'; rm -rf / #"));
        assert!(!is_valid_http_host_authority("evil.com/path"));
        assert!(!is_valid_http_host_authority("evil.com?x=1"));
        assert!(!is_valid_http_host_authority("user@evil.com"));
        assert!(!is_valid_http_host_authority("$(whoami).evil.com"));
        assert!(!is_valid_http_host_authority(""));
        assert!(!is_shell_safe_base_url("javascript://evil.com"));
        assert!(!is_shell_safe_base_url("https://evil.com/foo"));
        assert!(is_shell_safe_base_url("https://coda.example.com:8443"));
    }
}
