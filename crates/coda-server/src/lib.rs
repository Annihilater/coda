pub mod db;
pub mod relay;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{Query, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use coda_protocol::{RelayTicketClaims, StoredDeviceIdentity};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};
use uuid::Uuid;

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

pub fn build_router(state: AppState, web_dist_dir: Option<PathBuf>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/public/site-info", get(public_site_info_handler))
        .route("/api/v1/auth/register", post(register_handler))
        .route("/api/v1/auth/login", post(login_handler))
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
        .route("/install.sh", get(install_script_handler))
        .route("/v1/tunnel/ws", get(ws_tunnel_upgrade_handler))
        .with_state(state);

    let app = if let Some(dir) = web_dist_dir.filter(|p| p.exists()) {
        let index = dir.join("index.html");
        api.fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)))
    } else {
        api
    };

    app.layer(cors)
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
    let db = Db::connect_and_migrate(database_url).await?;
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
    };

    let router = build_router(state.clone(), web_dist_dir);
    tokio::spawn(async move {
        let _ = axum::serve(http_listener, router).await;
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

fn effective_public_base_url(st: &AppState, headers: &HeaderMap) -> String {
    if st.has_explicit_public_url {
        return st.public_base_url.clone();
    }
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(host) = host else {
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
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| {
            if cf_https || (!is_loopback_host_or_endpoint(host) && !host.contains(':')) {
                "https".to_string()
            } else {
                "http".to_string()
            }
        });
    format!("{proto}://{host}")
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

async fn health_handler(State(st): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "coda-server",
        "version": st.release_version,
        "open_registration": st.open_registration,
        "zero_knowledge_e2ee": true,
        "raw_tcp_active": st.raw_tcp_active,
        "raw_tcp_no_sni_endpoint": st.raw_tcp_public_endpoint,
        "server_pubkey": st.server_pubkey_b64,
    }))
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
    let dl_base = format!("https://github.com/{}/releases/download/{tag}", st.github_repo);
    let releases_page = format!("https://github.com/{}/releases/tag/{tag}", st.github_repo);

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
            "daemon_linux_x64": format!("{dl_base}/coda-daemon-linux-x86_64-{tag}.tar.gz"),
            "daemon_linux_arm64": format!("{dl_base}/coda-daemon-linux-aarch64-{tag}.tar.gz"),
            "daemon_macos_arm64": format!("{dl_base}/coda-daemon-macos-aarch64-{tag}.tar.gz"),
            "server_linux_x64": format!("{dl_base}/coda-server-linux-x86_64-{tag}.tar.gz"),
            "server_linux_arm64": format!("{dl_base}/coda-server-linux-aarch64-{tag}.tar.gz"),
            "server_macos_arm64": format!("{dl_base}/coda-macos-aarch64-{tag}.tar.gz"),
            "desktop_macos_dmg": format!("{dl_base}/Coda-Desktop-macos-aarch64-{tag}.dmg"),
            "desktop_macos_tar": format!("{dl_base}/coda-macos-aarch64-{tag}.tar.gz"),
            "deploy_bundle": format!("{dl_base}/coda-deploy-bundle-{tag}.tar.gz"),
            "ios_beta_url": st.ios_beta_url,
            "releases_page": releases_page,
        }
    })))
}

async fn register_handler(
    State(st): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
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
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let (user, token) = st
        .db
        .login_user(&req.email_or_handle, &req.password, &req.client_label)
        .await
        .map_err(ApiError::unauthorized)?;
    Ok(Json(AuthResponse { user, token }))
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
    headers: HeaderMap,
    Json(req): Json<StartDeviceAuthRequest>,
) -> Result<Json<StartDeviceAuthResponse>, ApiError> {
    let role = match req.role.as_str() {
        "daemon" | "client" | "hybrid" => req.role.as_str(),
        _ => {
            return Err(ApiError::bad_request(anyhow::anyhow!(
                "invalid device role; expected daemon, client, or hybrid"
            )))
        }
    };
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
    headers: HeaderMap,
    Json(req): Json<ApproveDeviceAuthRequest>,
) -> Result<Json<DeviceRecord>, ApiError> {
    let user = require_auth(&st, &headers).await?;
    let device = st
        .db
        .approve_device_auth(user.id, &req.user_code, req.device_name.as_deref())
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(device))
}

async fn poll_device_auth_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PollDeviceAuthRequest>,
) -> Result<Json<PollDeviceAuthResponse>, ApiError> {
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

    Ok(Json(ClaimPairResponse {
        device,
        owner_handle: user.handle,
        daemon_relay_ticket,
        relays,
    }))
}

async fn claim_pair_handler(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ClaimPairRequest>,
) -> Result<Json<ClaimPairResponse>, ApiError> {
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
    ws.on_upgrade(move |socket| hub.handle_ws_socket(socket))
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
