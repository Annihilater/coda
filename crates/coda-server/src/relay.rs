//! Stateless Zero-Knowledge Relay Engine (`RelayHub`).
//! - Supports BOTH Domestic Cloud No-ICP Raw TCP (`:24443`, zero HTTP/SNI) AND Overseas CDN WSS (`/v1/tunnel/ws`).
//! - Enforces strict tenant isolation (`user_id` bound inside signed HMAC `RelayTicketClaims`).
//! - Implements Dual-Lane Priority Routing (`ControlP0` unthrottled fast-path vs `BulkP1` token-bucket shaped).
//! - Supports 75-second CDN Make-Before-Break seamless connection handoff via monotonic `generation`.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use chrono::Utc;
use coda_protocol::{
    sanitize_network_endpoints, AccountLiveEvent, NetworkEndpointCandidate, RelayTicketClaims,
    RelayWireFrame, TrafficLane, MAX_AUTH_FRAME_BYTES, MAX_WIRE_FRAME_BYTES,
};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{broadcast, mpsc, watch, Mutex, RwLock},
    task::AbortHandle,
    time::timeout,
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::db::Db;

const AUTH_BIND_TIMEOUT: Duration = Duration::from_secs(10);
const TUNNEL_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const STALE_GEN_GRACE_WINDOW: Duration = Duration::from_secs(5);

struct ActiveLaneConn {
    conn_id: u64,
    generation: u64,
    connected_at: Instant,
    tx: mpsc::Sender<Arc<RelayWireFrame>>,
    abort_handle: AbortHandle,
    shutdown_tx: watch::Sender<bool>,
}

impl ActiveLaneConn {
    fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.abort_handle.abort();
    }
}

struct DeviceSlots {
    user_id: Uuid,
    role: String,
    control_p0: Option<ActiveLaneConn>,
    bulk_p1: Option<ActiveLaneConn>,
    endpoints: Vec<NetworkEndpointCandidate>,
}

struct TenantBucket {
    tokens_bytes: f64,
    capacity_bytes: f64,
    refill_bytes_per_sec: f64,
    last_refill: Instant,
}

impl TenantBucket {
    fn new(quota_mbps: u32) -> Self {
        let bytes_per_sec = (quota_mbps.max(1) as f64) * 125_000.0;
        Self {
            tokens_bytes: bytes_per_sec * 2.0, // 2-second burst allowance
            capacity_bytes: bytes_per_sec * 2.0,
            refill_bytes_per_sec: bytes_per_sec,
            last_refill: Instant::now(),
        }
    }

    fn consume_and_delay_ms(&mut self, frame_bytes: usize) -> u64 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens_bytes =
            (self.tokens_bytes + elapsed * self.refill_bytes_per_sec).min(self.capacity_bytes);

        let cost = frame_bytes as f64;
        self.tokens_bytes = (self.tokens_bytes - cost).max(-self.capacity_bytes);
        if self.tokens_bytes >= 0.0 {
            0
        } else {
            ((-self.tokens_bytes / self.refill_bytes_per_sec) * 1000.0).min(1000.0) as u64
        }
    }
}

#[derive(Debug, Clone)]
pub enum SessionRevocation {
    All(Uuid),
    Single(Uuid, String),
    AllExcept(Uuid, String),
}

pub struct RelayHub {
    relay_secret: Vec<u8>,
    region_label: String,
    devices: RwLock<HashMap<Uuid, DeviceSlots>>,
    tenant_buckets: RwLock<HashMap<Uuid, TenantBucket>>,
    account_watchers: RwLock<HashMap<Uuid, broadcast::Sender<AccountLiveEvent>>>,
    session_revocations: broadcast::Sender<SessionRevocation>,
    next_conn_id: AtomicU64,
    online_sync_lock: Mutex<()>,
    /// Optional DB handle when running co-located with `coda-coord` to sync `is_online`.
    /// When deployed as a standalone stateless Domestic Aliyun/Tencent relay (`--relay-only`), `db` is `None`!
    db: Option<Db>,
}

impl RelayHub {
    pub fn new(relay_secret: Vec<u8>, region_label: impl Into<String>, db: Option<Db>) -> Arc<Self> {
        let (rev_tx, _) = broadcast::channel(256);
        Arc::new(Self {
            relay_secret,
            region_label: region_label.into(),
            devices: RwLock::new(HashMap::new()),
            tenant_buckets: RwLock::new(HashMap::new()),
            account_watchers: RwLock::new(HashMap::new()),
            session_revocations: rev_tx,
            next_conn_id: AtomicU64::new(1),
            online_sync_lock: Mutex::new(()),
            db,
        })
    }

    /// Notifies live `/api/v1/events/ws` sockets to terminate when a token or all user sessions are revoked.
    pub fn notify_session_revoked(&self, user_id: Uuid, token_sha256: Option<String>) {
        let ev = match token_sha256 {
            Some(sha) => SessionRevocation::Single(user_id, sha),
            None => SessionRevocation::All(user_id),
        };
        let _ = self.session_revocations.send(ev);
    }

    /// Single-frame revocation broadcast for password changes: revokes every session of `user_id`
    /// EXCEPT `keep_token_sha256`, avoiding O(N) broadcast channel overflow.
    pub fn notify_sessions_revoked_except(&self, user_id: Uuid, keep_token_sha256: String) {
        let _ = self
            .session_revocations
            .send(SessionRevocation::AllExcept(user_id, keep_token_sha256));
    }

    pub async fn subscribe_account_events(
        &self,
        user_id: Uuid,
    ) -> broadcast::Receiver<AccountLiveEvent> {
        if let Some(tx) = self.account_watchers.read().await.get(&user_id) {
            return tx.subscribe();
        }
        let mut map = self.account_watchers.write().await;
        let tx = map
            .entry(user_id)
            .or_insert_with(|| broadcast::channel(64).0);
        tx.subscribe()
    }

    pub async fn publish_account_event(&self, user_id: Uuid, event: AccountLiveEvent) {
        let tx = {
            let map = self.account_watchers.read().await;
            map.get(&user_id).cloned()
        };
        if let Some(tx) = tx {
            let _ = tx.send(event);
        }
    }

    pub async fn notify_device_enrolled(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        device_name: &str,
        role: &str,
        online: Option<bool>,
    ) {
        self.publish_account_event(
            user_id,
            AccountLiveEvent {
                kind: "devices_changed".to_string(),
                device_id: Some(device_id),
                device_name: Some(device_name.to_string()),
                role: Some(role.to_string()),
                online,
                timestamp_ms: Some(Utc::now().timestamp_millis().max(0) as u64),
            },
        )
        .await;
    }

    /// Authenticates first WebSocket frame (`{"type":"auth","token":"..."}`) when no Authorization
    /// header or subprotocol token was provided, then delegates to `handle_account_events_ws`.
    pub async fn handle_account_events_ws_unauthenticated(
        &self,
        mut socket: WebSocket,
        db: Db,
    ) {
        let auth_msg = match timeout(Duration::from_secs(5), socket.recv()).await {
            Ok(Some(Ok(WsMessage::Text(txt)))) if txt.len() <= 8192 => txt,
            _ => return,
        };
        #[derive(serde::Deserialize)]
        struct WsAuthInit {
            #[serde(default)]
            r#type: String,
            #[serde(default)]
            token: String,
        }
        let Ok(parsed) = serde_json::from_str::<WsAuthInit>(&auth_msg) else {
            return;
        };
        if parsed.r#type != "auth" || parsed.token.trim().is_empty() {
            return;
        }
        let Ok(session) = db.authenticate_session(parsed.token.trim()).await else {
            let _ = socket.send(WsMessage::Close(None)).await;
            return;
        };
        self.handle_account_events_ws(socket, session.user.id, Some(session.token_sha256))
            .await;
    }

    /// Streams real-time account events (`/api/v1/events/ws`) to a logged-in desktop/mobile client.
    pub async fn handle_account_events_ws(
        &self,
        mut socket: WebSocket,
        user_id: Uuid,
        token_sha256: Option<String>,
    ) {
        let mut rx = self.subscribe_account_events(user_id).await;
        let mut rev_rx = self.session_revocations.subscribe();
        let hello = AccountLiveEvent {
            kind: "connected".to_string(),
            device_id: None,
            device_name: None,
            role: None,
            online: None,
            timestamp_ms: Some(Utc::now().timestamp_millis().max(0) as u64),
        };
        if let Ok(json_text) = serde_json::to_string(&hello) {
            if socket.send(WsMessage::Text(json_text.into())).await.is_err() {
                return;
            }
        }

        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(25));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = keepalive.tick() => {
                    if socket.send(WsMessage::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
                rev = rev_rx.recv() => {
                    match rev {
                        Ok(SessionRevocation::All(rev_uid)) if rev_uid == user_id => {
                            let _ = socket.send(WsMessage::Close(None)).await;
                            break;
                        }
                        Ok(SessionRevocation::Single(rev_uid, target))
                            if rev_uid == user_id && token_sha256.as_deref() == Some(target.as_str()) =>
                        {
                            let _ = socket.send(WsMessage::Close(None)).await;
                            break;
                        }
                        Ok(SessionRevocation::AllExcept(rev_uid, keep))
                            if rev_uid == user_id && token_sha256.as_deref() != Some(keep.as_str()) =>
                        {
                            let _ = socket.send(WsMessage::Close(None)).await;
                            break;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Fail closed on revocation channel lag so a missed revocation never leaves a dead session open.
                            let _ = socket.send(WsMessage::Close(None)).await;
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                recv = rx.recv() => {
                    match recv {
                        Ok(ev) => {
                            if let Ok(text) = serde_json::to_string(&ev) {
                                if socket.send(WsMessage::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let resync = AccountLiveEvent {
                                kind: "devices_changed".to_string(),
                                device_id: None,
                                device_name: None,
                                role: None,
                                online: None,
                                timestamp_ms: Some(Utc::now().timestamp_millis().max(0) as u64),
                            };
                            if let Ok(text) = serde_json::to_string(&resync) {
                                if socket.send(WsMessage::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                ws_msg = socket.recv() => {
                    match ws_msg {
                        Some(Ok(WsMessage::Ping(payload))) => {
                            if socket.send(WsMessage::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(WsMessage::Close(_))) | None => break,
                        Some(Ok(_)) => {}
                        Some(Err(_)) => break,
                    }
                }
            }
        }
    }

    /// Spawns the Domestic Cloud No-ICP Raw TCP Relay Listener (zero HTTP, zero domain SNI).
    pub async fn spawn_raw_tcp_listener(
        self: &Arc<Self>,
        bind_addr: SocketAddr,
    ) -> Result<SocketAddr> {
        let listener = TcpListener::bind(bind_addr)
            .await
            .with_context(|| format!("failed binding raw TCP relay on {bind_addr}"))?;
        let local_addr = listener.local_addr()?;
        info!("Coda No-ICP Raw TCP Relay listening on {local_addr} (TCP_NODELAY, zero SNI)");

        let hub = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        let _ = stream.set_nodelay(true);
                        let hub_conn = Arc::clone(&hub);
                        tokio::spawn(async move {
                            if let Err(e) = hub_conn.handle_raw_tcp_stream(stream, peer_addr).await {
                                debug!("raw tcp relay peer {peer_addr} closed: {e}");
                            }
                        });
                    }
                    Err(e) => warn!("raw tcp accept error: {e}"),
                }
            }
        });

        Ok(local_addr)
    }

    async fn handle_raw_tcp_stream(
        self: Arc<Self>,
        stream: TcpStream,
        _peer_addr: SocketAddr,
    ) -> Result<()> {
        let (mut reader, mut writer) = stream.into_split();

        // First frame MUST be `AuthBind` within timeout and bounded to 16 KiB to defeat pre-auth OOM/slowloris.
        let first_frame = match timeout(
            AUTH_BIND_TIMEOUT,
            read_length_delimited_frame_limited(&mut reader, MAX_AUTH_FRAME_BYTES),
        )
        .await
        {
            Ok(Ok(f)) => f,
            _ => return Ok(()),
        };
        let (claims, lane, generation) = match self.authenticate_bind_frame(first_frame).await {
            Ok(tuple) => tuple,
            Err(err_frame) => {
                let bytes = err_frame.encode_length_delimited()?;
                let _ = writer.write_all(&bytes).await;
                return Ok(());
            }
        };

        let (tx, mut rx) = mpsc::channel::<Arc<RelayWireFrame>>(512);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let shutdown_tx_w = shutdown_tx.clone();
        let write_task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if let Ok(encoded) = frame.encode_length_delimited() {
                    if writer.write_all(&encoded).await.is_err() {
                        break;
                    }
                }
            }
            let _ = shutdown_tx_w.send(true);
        });
        let write_abort = write_task.abort_handle();
        let conn_id = match self
            .register_lane(&claims, lane, generation, tx.clone(), write_abort, shutdown_tx)
            .await
        {
            Ok(id) => id,
            Err(err_frame) => {
                let _ = tx.try_send(Arc::new(err_frame));
                tokio::time::sleep(Duration::from_millis(50)).await;
                write_task.abort();
                return Ok(());
            }
        };

        let ack = Arc::new(RelayWireFrame::AuthAck {
            ok: true,
            device_id: claims.device_id,
            server_region: self.region_label.clone(),
            message: "authenticated raw_tcp_no_sni lane".into(),
        });
        if tx.send(ack).await.is_err() {
            write_task.abort();
            self.unregister_lane(claims.device_id, lane, conn_id).await;
            return Ok(());
        }

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                res = timeout(
                    TUNNEL_IDLE_TIMEOUT,
                    read_length_delimited_frame_limited(&mut reader, MAX_WIRE_FRAME_BYTES),
                ) => {
                    match res {
                        Ok(Ok(frame)) => {
                            self.process_inbound_frame(&claims, lane, frame).await;
                        }
                        _ => break,
                    }
                }
            }
        }

        write_task.abort();
        self.unregister_lane(claims.device_id, lane, conn_id).await;
        Ok(())
    }

    /// Handles an Overseas CDN-compatible WebSocket tunnel (`/v1/tunnel/ws`).
    pub async fn handle_ws_socket(self: Arc<Self>, socket: WebSocket) {
        let (mut ws_sink, mut ws_stream) = socket.split();

        let first_msg = match timeout(AUTH_BIND_TIMEOUT, ws_stream.next()).await {
            Ok(Some(Ok(m))) => m,
            _ => return,
        };
        let first_frame: RelayWireFrame =
            match parse_ws_message_limited(first_msg, MAX_AUTH_FRAME_BYTES) {
                Some(f) => f,
                None => return,
            };

        let (claims, lane, generation) = match self.authenticate_bind_frame(first_frame).await {
            Ok(t) => t,
            Err(err_frame) => {
                if let Ok(txt) = serde_json::to_string(&err_frame) {
                    let _ = ws_sink.send(WsMessage::Text(txt.into())).await;
                }
                return;
            }
        };

        let (tx, mut rx) = mpsc::channel::<Arc<RelayWireFrame>>(512);
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let shutdown_tx_w = shutdown_tx.clone();
        let write_task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if let Ok(txt) = serde_json::to_string(&*frame) {
                    if ws_sink.send(WsMessage::Text(txt.into())).await.is_err() {
                        break;
                    }
                }
            }
            let _ = shutdown_tx_w.send(true);
            let _ = ws_sink.send(WsMessage::Close(None)).await;
        });
        let write_abort = write_task.abort_handle();
        let conn_id = match self
            .register_lane(&claims, lane, generation, tx.clone(), write_abort, shutdown_tx)
            .await
        {
            Ok(id) => id,
            Err(err_frame) => {
                let _ = tx.try_send(Arc::new(err_frame));
                tokio::time::sleep(Duration::from_millis(50)).await;
                write_task.abort();
                return;
            }
        };

        let ack = Arc::new(RelayWireFrame::AuthAck {
            ok: true,
            device_id: claims.device_id,
            server_region: self.region_label.clone(),
            message: "authenticated cdn_wss lane".into(),
        });
        if tx.send(ack).await.is_err() {
            write_task.abort();
            self.unregister_lane(claims.device_id, lane, conn_id).await;
            return;
        }

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                res = timeout(TUNNEL_IDLE_TIMEOUT, ws_stream.next()) => {
                    match res {
                        Ok(Some(Ok(msg))) => {
                            if matches!(msg, WsMessage::Close(_)) {
                                break;
                            }
                            if let Some(frame) = parse_ws_message_limited(msg, MAX_WIRE_FRAME_BYTES) {
                                self.process_inbound_frame(&claims, lane, frame).await;
                            }
                        }
                        _ => break,
                    }
                }
            }
        }

        write_task.abort();
        self.unregister_lane(claims.device_id, lane, conn_id).await;
    }

    async fn authenticate_bind_frame(
        &self,
        frame: RelayWireFrame,
    ) -> std::result::Result<(RelayTicketClaims, TrafficLane, u64), RelayWireFrame> {
        match frame {
            RelayWireFrame::AuthBind {
                ticket,
                lane,
                generation,
            } => {
                let now = Utc::now().timestamp();
                let claims = match RelayTicketClaims::verify(&ticket, &self.relay_secret, now) {
                    Ok(c) => c,
                    Err(e) => {
                        return Err(RelayWireFrame::Error {
                            code: "invalid_ticket".into(),
                            message: e.to_string(),
                        });
                    }
                };
                if let Some(db) = &self.db {
                    match db.get_device_by_id(claims.device_id).await {
                        Ok(Some(dev)) if dev.user_id == claims.user_id => {}
                        Ok(_) => {
                            return Err(RelayWireFrame::Error {
                                code: "device_revoked".into(),
                                message: "device has been revoked or removed".into(),
                            });
                        }
                        Err(e) => {
                            warn!("relay auth_bind DB lookup failed: {e}");
                        }
                    }
                }
                Ok((claims, lane, generation))
            }
            _ => Err(RelayWireFrame::Error {
                code: "expected_auth_bind".into(),
                message: "first frame on tunnel must be auth_bind".into(),
            }),
        }
    }

    async fn register_lane(
        &self,
        claims: &RelayTicketClaims,
        lane: TrafficLane,
        generation: u64,
        tx: mpsc::Sender<Arc<RelayWireFrame>>,
        abort_handle: AbortHandle,
        shutdown_tx: watch::Sender<bool>,
    ) -> std::result::Result<u64, RelayWireFrame> {
        {
            let mut buckets = self.tenant_buckets.write().await;
            buckets
                .entry(claims.user_id)
                .or_insert_with(|| TenantBucket::new(claims.quota_mbps));
        }

        let conn_id = self.next_conn_id.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let superseded_conn: Option<ActiveLaneConn>;
        let became_online = {
            let mut map = self.devices.write().await;
            let entry = map.entry(claims.device_id).or_insert_with(|| DeviceSlots {
                user_id: claims.user_id,
                role: claims.role.clone(),
                control_p0: None,
                bulk_p1: None,
                endpoints: Vec::new(),
            });
            entry.role = claims.role.clone();
            let was_empty = entry.control_p0.is_none() && entry.bulk_p1.is_none();

            let conn = ActiveLaneConn {
                conn_id,
                generation,
                connected_at: now,
                tx,
                abort_handle,
                shutdown_tx,
            };
            let slot = match lane {
                TrafficLane::ControlP0 => &mut entry.control_p0,
                TrafficLane::BulkP1 => &mut entry.bulk_p1,
            };
            let accept = slot.as_ref().map_or(true, |old| {
                generation >= old.generation
                    || now.duration_since(old.connected_at) >= STALE_GEN_GRACE_WINDOW
            });
            if accept {
                superseded_conn = slot.replace(conn);
            } else {
                return Err(RelayWireFrame::Error {
                    code: "stale_generation".into(),
                    message: "superseded by a newer connection generation".into(),
                });
            }
            was_empty
        };

        if let Some(old) = superseded_conn {
            old.shutdown();
        }

        if became_online {
            let cached_eps = {
                let map = self.devices.read().await;
                map.get(&claims.device_id)
                    .map(|s| s.endpoints.clone())
                    .filter(|eps| !eps.is_empty())
            };
            self.sync_online_and_broadcast(
                claims.user_id,
                claims.device_id,
                cached_eps,
                Some(claims.role.clone()),
            )
            .await;
        }
        Ok(conn_id)
    }

    async fn unregister_lane(&self, device_id: Uuid, lane: TrafficLane, conn_id: u64) {
        let mut went_offline_info = None;
        {
            let mut map = self.devices.write().await;
            if let Some(slots) = map.get_mut(&device_id) {
                let slot = match lane {
                    TrafficLane::ControlP0 => &mut slots.control_p0,
                    TrafficLane::BulkP1 => &mut slots.bulk_p1,
                };
                if slot.as_ref().map_or(false, |c| c.conn_id == conn_id) {
                    *slot = None;
                }
                if slots.control_p0.is_none() && slots.bulk_p1.is_none() {
                    went_offline_info = Some((slots.user_id, slots.role.clone()));
                    map.remove(&device_id);
                }
            }
        }

        if let Some((user_id, role)) = went_offline_info {
            self.sync_online_and_broadcast(user_id, device_id, None, Some(role))
                .await;
            self.gc_idle_state(user_id).await;
        }
    }

    /// Immediately disconnects and kicks a deleted/revoked device across all active lanes.
    pub async fn disconnect_device(&self, user_id: Uuid, device_id: Uuid) {
        let removed = {
            let mut map = self.devices.write().await;
            if map.get(&device_id).is_some_and(|s| s.user_id == user_id) {
                map.remove(&device_id)
            } else {
                None
            }
        };
        if let Some(slots) = removed {
            if let Some(p0) = slots.control_p0 {
                p0.shutdown();
            }
            if let Some(p1) = slots.bulk_p1 {
                p1.shutdown();
            }
            self.sync_online_and_broadcast(user_id, device_id, None, Some(slots.role))
                .await;
            self.gc_idle_state(user_id).await;
        }
    }

    async fn sync_online_and_broadcast(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        endpoints_if_online: Option<Vec<NetworkEndpointCandidate>>,
        role: Option<String>,
    ) {
        // Serialize BOTH DB online writes and presence broadcasts under `online_sync_lock`
        // using authoritative re-read state so rapid reconnects cannot broadcast stale offline.
        let _guard = self.online_sync_lock.lock().await;
        let (is_online, eps) = {
            let map = self.devices.read().await;
            if let Some(s) = map.get(&device_id) {
                let e = endpoints_if_online.or_else(|| {
                    if s.endpoints.is_empty() {
                        None
                    } else {
                        Some(s.endpoints.clone())
                    }
                });
                (true, e)
            } else {
                (false, None)
            }
        };
        if let Some(db) = &self.db {
            if let Err(e) = db.set_device_online(device_id, is_online).await {
                warn!("failed syncing device {device_id} online={is_online} to DB: {e}");
            }
        }
        self.broadcast_presence_to_tenant(user_id, device_id, is_online, eps, role)
            .await;
    }

    async fn gc_idle_state(&self, user_id: Uuid) {
        let has_online_devices = self
            .devices
            .read()
            .await
            .values()
            .any(|s| s.user_id == user_id);
        if !has_online_devices {
            self.tenant_buckets.write().await.remove(&user_id);
        }
        let mut watchers = self.account_watchers.write().await;
        watchers.retain(|_, tx| tx.receiver_count() > 0);
    }

    async fn broadcast_presence_to_tenant(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        is_online: bool,
        endpoints: Option<Vec<NetworkEndpointCandidate>>,
        role: Option<String>,
    ) {
        let notice = Arc::new(RelayWireFrame::PeerPresence {
            device_id,
            is_online,
            endpoints,
        });
        let targets: Vec<mpsc::Sender<Arc<RelayWireFrame>>> = {
            let map = self.devices.read().await;
            map.iter()
                .filter(|(id, s)| **id != device_id && s.user_id == user_id)
                .filter_map(|(_, s)| s.control_p0.as_ref().or(s.bulk_p1.as_ref()))
                .map(|c| c.tx.clone())
                .collect()
        };
        for tx in targets {
            let _ = tx.try_send(Arc::clone(&notice));
        }

        self.publish_account_event(
            user_id,
            AccountLiveEvent {
                kind: "peer_presence".to_string(),
                device_id: Some(device_id),
                device_name: None,
                role,
                online: Some(is_online),
                timestamp_ms: Some(Utc::now().timestamp_millis().max(0) as u64),
            },
        )
        .await;
    }

    async fn process_inbound_frame(
        &self,
        sender: &RelayTicketClaims,
        bound_lane: TrafficLane,
        frame: RelayWireFrame,
    ) {
        match frame {
            RelayWireFrame::KeepAlive { ts_ms } => {
                let reply = Arc::new(RelayWireFrame::KeepAlive { ts_ms });
                if let Some((tx, _)) = self
                    .pick_target_sender(sender.user_id, sender.device_id, TrafficLane::ControlP0)
                    .await
                {
                    let _ = tx.try_send(reply);
                }
            }
            RelayWireFrame::SessionReset {
                dst_device_id,
                session_nonce_b64,
                reason,
                ..
            } => {
                if let Some((target_tx, _)) = self
                    .pick_target_sender(sender.user_id, dst_device_id, TrafficLane::ControlP0)
                    .await
                {
                    let forwarded = Arc::new(RelayWireFrame::SessionReset {
                        src_device_id: sender.device_id,
                        dst_device_id,
                        session_nonce_b64,
                        reason: reason.chars().take(128).collect(),
                    });
                    let _ = target_tx.try_send(forwarded);
                }
            }
            RelayWireFrame::RoutePacket {
                dst_device_id,
                lane,
                e2ee,
                sender_x25519_pub_b64,
                session_auth,
                ..
            } => {
                // If ticket pinned a specific peer_device_id, enforce it strictly
                if let Some(allowed_peer) = sender.peer_device_id {
                    if allowed_peer != dst_device_id {
                        return;
                    }
                }

                // Prevent lateral movement: a `daemon`-only controlled host may NEVER initiate
                // a brand-new client session (`session_auth.is_some()`).
                if sender.role == "daemon" && session_auth.is_some() {
                    warn!(
                        "blocked daemon-role device {} attempting to initiate relay session to {}",
                        sender.device_id, dst_device_id
                    );
                    return;
                }

                // Fair-queue token bucket shaping ONLY for BulkP1; never sleep inline on a ControlP0 socket
                // so bulk traffic cannot head-of-line block interactive keystrokes (`ControlP0`).
                if lane == TrafficLane::BulkP1 {
                    let delay_ms = {
                        let mut buckets = self.tenant_buckets.write().await;
                        buckets
                            .get_mut(&sender.user_id)
                            .map(|b| b.consume_and_delay_ms(e2ee.ciphertext_b64.len()))
                            .unwrap_or(0)
                    };
                    if delay_ms > 0 && bound_lane == TrafficLane::BulkP1 {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }

                // Zero-knowledge blind forward: enforce `dst` belongs to SAME `user_id`!
                if let Some((target_tx, target_role)) = self
                    .pick_target_sender(sender.user_id, dst_device_id, lane)
                    .await
                {
                    // Prevent daemon-to-daemon lateral movement inside a tenant.
                    if sender.role == "daemon" && target_role == "daemon" {
                        warn!(
                            "blocked daemon-to-daemon route from {} to {}",
                            sender.device_id, dst_device_id
                        );
                        return;
                    }
                    let forwarded = Arc::new(RelayWireFrame::RoutePacket {
                        src_device_id: sender.device_id,
                        dst_device_id,
                        lane,
                        e2ee,
                        sender_x25519_pub_b64,
                        session_auth,
                    });
                    if let Err(err) = target_tx.try_send(forwarded) {
                        let (code, msg) = match err {
                            mpsc::error::TrySendError::Full(_) => (
                                "peer_backpressure",
                                format!("target device {dst_device_id} outbound queue full"),
                            ),
                            mpsc::error::TrySendError::Closed(_) => (
                                "peer_offline",
                                format!("target device {dst_device_id} connection closed"),
                            ),
                        };
                        if let Some((back_tx, _)) = self
                            .pick_target_sender(
                                sender.user_id,
                                sender.device_id,
                                TrafficLane::ControlP0,
                            )
                            .await
                        {
                            let _ = back_tx.try_send(Arc::new(RelayWireFrame::Error {
                                code: code.into(),
                                message: msg,
                            }));
                        }
                    }
                } else if let Some((back_tx, _)) = self
                    .pick_target_sender(sender.user_id, sender.device_id, TrafficLane::ControlP0)
                    .await
                {
                    let _ = back_tx.try_send(Arc::new(RelayWireFrame::Error {
                        code: "peer_offline".into(),
                        message: format!(
                            "target device {dst_device_id} is offline or in another tenant"
                        ),
                    }));
                }
            }
            RelayWireFrame::AdvertiseEndpoints { endpoints } => {
                if sender.role == "client" {
                    return;
                }
                let endpoints = sanitize_network_endpoints(endpoints);
                {
                    let mut map = self.devices.write().await;
                    if let Some(slots) = map.get_mut(&sender.device_id) {
                        slots.endpoints = endpoints.clone();
                    }
                }
                if let Some(db) = &self.db {
                    let val = serde_json::to_value(&endpoints).unwrap_or_default();
                    let _ = db.update_device_endpoints(sender.device_id, &val).await;
                }
                self.sync_online_and_broadcast(
                    sender.user_id,
                    sender.device_id,
                    Some(endpoints),
                    Some(sender.role.clone()),
                )
                .await;
            }
            _ => {}
        }
    }

    async fn pick_target_sender(
        &self,
        expected_user_id: Uuid,
        target_device_id: Uuid,
        preferred_lane: TrafficLane,
    ) -> Option<(mpsc::Sender<Arc<RelayWireFrame>>, String)> {
        let map = self.devices.read().await;
        let slots = map.get(&target_device_id)?;
        // Strict tenant boundary check!
        if slots.user_id != expected_user_id {
            warn!(
                "blocked cross-tenant relay attempt from user {} to device {}",
                expected_user_id, target_device_id
            );
            return None;
        }
        let chosen: Option<&ActiveLaneConn> = match preferred_lane {
            TrafficLane::ControlP0 => slots.control_p0.as_ref().or(slots.bulk_p1.as_ref()),
            TrafficLane::BulkP1 => slots.bulk_p1.as_ref().or(slots.control_p0.as_ref()),
        };
        chosen.map(|c| (c.tx.clone(), slots.role.clone()))
    }
}

async fn read_length_delimited_frame_limited<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<RelayWireFrame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > max_bytes {
        anyhow::bail!("invalid wire frame length: {len} (limit {max_bytes})");
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}

fn parse_ws_message_limited(msg: WsMessage, max_bytes: usize) -> Option<RelayWireFrame> {
    match msg {
        WsMessage::Text(txt) if txt.len() <= max_bytes => serde_json::from_str(&txt).ok(),
        WsMessage::Binary(bin) if bin.len() <= max_bytes => serde_json::from_slice(&bin).ok(),
        _ => None,
    }
}
