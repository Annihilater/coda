pub mod db;
pub mod relay;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::{
    extract::{State, WebSocketUpgrade},
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
    db::{Db, DeviceRecord, InviteCodeRecord, RelayNodeRecord, UserRecord},
    relay::RelayHub,
};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub relay_hub: Arc<RelayHub>,
    pub relay_secret: Vec<u8>,
    pub public_base_url: String,
    pub raw_tcp_public_endpoint: String,
    pub server_pubkey_b64: String,
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

pub fn build_router(state: AppState, web_dist_dir: Option<PathBuf>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/auth/register", post(register_handler))
        .route("/api/v1/auth/login", post(login_handler))
        .route("/api/v1/auth/me", get(me_handler))
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
    let db = Db::connect_and_migrate(database_url).await?;
    let relay_hub = RelayHub::new(
        relay_secret.clone(),
        "Coda-Hybrid-Edge",
        Some(db.clone()),
    );

    let raw_tcp_addr = relay_hub.spawn_raw_tcp_listener(raw_tcp_bind).await?;
    let http_listener = tokio::net::TcpListener::bind(http_bind).await?;
    let http_addr = http_listener.local_addr()?;

    let base_url = public_base_url.unwrap_or_else(|| format!("http://{http_addr}"));
    let wss_url = base_url
        .replace("https://", "wss://")
        .replace("http://", "ws://")
        + "/v1/tunnel/ws";
    let raw_tcp_endpoint = format!("127.0.0.1:{}", raw_tcp_addr.port());

    let node_id = StoredDeviceIdentity::generate("coda-relay-node");
    db.seed_default_relays(&raw_tcp_endpoint, &wss_url, &node_id.ed25519_public_b64)
        .await?;

    let state = AppState {
        db,
        relay_hub,
        relay_secret,
        public_base_url: base_url,
        raw_tcp_public_endpoint: raw_tcp_endpoint,
        server_pubkey_b64: node_id.ed25519_public_b64,
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

// ============================================================================
// HTTP Handlers
// ============================================================================

async fn health_handler(State(st): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "coda-server",
        "zero_knowledge_e2ee": true,
        "raw_tcp_no_sni_endpoint": st.raw_tcp_public_endpoint,
        "server_pubkey": st.server_pubkey_b64,
    }))
}

async fn register_handler(
    State(st): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let (user, token) = st
        .db
        .register_user(
            &req.email,
            &req.handle,
            &req.display_name,
            &req.password,
            req.invite_code.as_deref(),
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
    let (code, expires_at) = st
        .db
        .create_pairing_token(user.id, req.suggested_name.as_deref())
        .await
        .map_err(ApiError::internal)?;
    let one_liner_command = format!(
        "curl -fsSL {}/install.sh | sh -s -- --server {} --code {}",
        st.public_base_url, st.public_base_url, code
    );
    Ok(Json(PairTokenResponse {
        code,
        expires_at: expires_at.to_rfc3339(),
        one_liner_command,
    }))
}

async fn install_script_handler() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/x-shellscript; charset=utf-8",
        )],
        include_str!("../../../deploy/install-daemon.sh"),
    )
}

async fn claim_pair_handler(
    State(st): State<AppState>,
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
    let relays = st
        .db
        .list_active_relays()
        .await
        .map_err(ApiError::internal)?;

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
    let relays = st
        .db
        .list_active_relays()
        .await
        .map_err(ApiError::internal)?;

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
