//! Stateless Zero-Knowledge Relay Engine (`RelayHub`).
//! - Supports BOTH Domestic Cloud No-ICP Raw TCP (`:24443`, zero HTTP/SNI) AND Overseas CDN WSS (`/v1/tunnel/ws`).
//! - Enforces strict tenant isolation (`user_id` bound inside signed HMAC `RelayTicketClaims`).
//! - Implements Dual-Lane Priority Routing (`ControlP0` unthrottled fast-path vs `BulkP1` token-bucket shaped).
//! - Supports 75-second CDN Make-Before-Break seamless connection handoff via monotonic `generation`.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use chrono::Utc;
use coda_protocol::{RelayTicketClaims, RelayWireFrame, TrafficLane, MAX_WIRE_FRAME_BYTES};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, RwLock},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::db::Db;

struct ActiveLaneConn {
    generation: u64,
    tx: mpsc::Sender<Arc<RelayWireFrame>>,
}

struct DeviceSlots {
    user_id: Uuid,
    control_p0: Option<ActiveLaneConn>,
    bulk_p1: Option<ActiveLaneConn>,
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
        if self.tokens_bytes >= cost {
            self.tokens_bytes -= cost;
            0
        } else {
            let deficit = cost - self.tokens_bytes;
            self.tokens_bytes = 0.0;
            ((deficit / self.refill_bytes_per_sec) * 1000.0).min(250.0) as u64
        }
    }
}

pub struct RelayHub {
    relay_secret: Vec<u8>,
    region_label: String,
    devices: RwLock<HashMap<Uuid, DeviceSlots>>,
    tenant_buckets: RwLock<HashMap<Uuid, TenantBucket>>,
    /// Optional DB handle when running co-located with `coda-coord` to sync `is_online`.
    /// When deployed as a standalone stateless Domestic Aliyun/Tencent relay (`--relay-only`), `db` is `None`!
    db: Option<Db>,
}

impl RelayHub {
    pub fn new(relay_secret: Vec<u8>, region_label: impl Into<String>, db: Option<Db>) -> Arc<Self> {
        Arc::new(Self {
            relay_secret,
            region_label: region_label.into(),
            devices: RwLock::new(HashMap::new()),
            tenant_buckets: RwLock::new(HashMap::new()),
            db,
        })
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

        // First frame MUST be `AuthBind`
        let first_frame = read_length_delimited_frame(&mut reader).await?;
        let (claims, lane, generation) = match self.authenticate_bind_frame(first_frame) {
            Ok(tuple) => tuple,
            Err(err_frame) => {
                let bytes = err_frame.encode_length_delimited()?;
                let _ = writer.write_all(&bytes).await;
                return Ok(());
            }
        };

        let ack = RelayWireFrame::AuthAck {
            ok: true,
            device_id: claims.device_id,
            server_region: self.region_label.clone(),
            message: "authenticated raw_tcp_no_sni lane".into(),
        };
        writer.write_all(&ack.encode_length_delimited()?).await?;

        let (tx, mut rx) = mpsc::channel::<Arc<RelayWireFrame>>(512);
        self.register_lane(&claims, lane, generation, tx).await;

        let write_task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if let Ok(encoded) = frame.encode_length_delimited() {
                    if writer.write_all(&encoded).await.is_err() {
                        break;
                    }
                }
            }
        });

        while let Ok(frame) = read_length_delimited_frame(&mut reader).await {
            self.process_inbound_frame(&claims, frame).await;
        }

        write_task.abort();
        self.unregister_lane(claims.device_id, lane, generation).await;
        Ok(())
    }

    /// Handles an Overseas CDN-compatible WebSocket tunnel (`/v1/tunnel/ws`).
    pub async fn handle_ws_socket(self: Arc<Self>, socket: WebSocket) {
        let (mut ws_sink, mut ws_stream) = socket.split();

        let Some(Ok(first_msg)) = ws_stream.next().await else {
            return;
        };
        let first_frame: RelayWireFrame = match parse_ws_message(first_msg) {
            Some(f) => f,
            None => return,
        };

        let (claims, lane, generation) = match self.authenticate_bind_frame(first_frame) {
            Ok(t) => t,
            Err(err_frame) => {
                if let Ok(txt) = serde_json::to_string(&err_frame) {
                    let _ = ws_sink.send(WsMessage::Text(txt.into())).await;
                }
                return;
            }
        };

        let ack = RelayWireFrame::AuthAck {
            ok: true,
            device_id: claims.device_id,
            server_region: self.region_label.clone(),
            message: "authenticated cdn_wss lane".into(),
        };
        if let Ok(txt) = serde_json::to_string(&ack) {
            if ws_sink.send(WsMessage::Text(txt.into())).await.is_err() {
                return;
            }
        }

        let (tx, mut rx) = mpsc::channel::<Arc<RelayWireFrame>>(512);
        self.register_lane(&claims, lane, generation, tx).await;

        let write_task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if let Ok(txt) = serde_json::to_string(&*frame) {
                    if ws_sink.send(WsMessage::Text(txt.into())).await.is_err() {
                        break;
                    }
                }
            }
        });

        while let Some(Ok(msg)) = ws_stream.next().await {
            if let Some(frame) = parse_ws_message(msg) {
                self.process_inbound_frame(&claims, frame).await;
            }
        }

        write_task.abort();
        self.unregister_lane(claims.device_id, lane, generation).await;
    }

    fn authenticate_bind_frame(
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
                match RelayTicketClaims::verify(&ticket, &self.relay_secret, now) {
                    Ok(claims) => Ok((claims, lane, generation)),
                    Err(e) => Err(RelayWireFrame::Error {
                        code: "invalid_ticket".into(),
                        message: e.to_string(),
                    }),
                }
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
    ) {
        {
            let mut buckets = self.tenant_buckets.write().await;
            buckets
                .entry(claims.user_id)
                .or_insert_with(|| TenantBucket::new(claims.quota_mbps));
        }

        let became_online = {
            let mut map = self.devices.write().await;
            let entry = map.entry(claims.device_id).or_insert_with(|| DeviceSlots {
                user_id: claims.user_id,
                control_p0: None,
                bulk_p1: None,
            });
            let was_empty = entry.control_p0.is_none() && entry.bulk_p1.is_none();

            let conn = ActiveLaneConn {
                generation,
                tx,
            };
            match lane {
                TrafficLane::ControlP0 => {
                    if entry
                        .control_p0
                        .as_ref()
                        .map_or(true, |old| generation >= old.generation)
                    {
                        entry.control_p0 = Some(conn);
                    }
                }
                TrafficLane::BulkP1 => {
                    if entry
                        .bulk_p1
                        .as_ref()
                        .map_or(true, |old| generation >= old.generation)
                    {
                        entry.bulk_p1 = Some(conn);
                    }
                }
            }
            was_empty
        };

        if became_online {
            if let Some(db) = &self.db {
                let _ = db.set_device_online(claims.device_id, true).await;
            }
            self.broadcast_presence_to_tenant(claims.user_id, claims.device_id, true)
                .await;
        }
    }

    async fn unregister_lane(&self, device_id: Uuid, lane: TrafficLane, generation: u64) {
        let mut went_offline_user = None;
        {
            let mut map = self.devices.write().await;
            if let Some(slots) = map.get_mut(&device_id) {
                match lane {
                    TrafficLane::ControlP0 => {
                        if slots
                            .control_p0
                            .as_ref()
                            .map_or(false, |c| c.generation == generation)
                        {
                            slots.control_p0 = None;
                        }
                    }
                    TrafficLane::BulkP1 => {
                        if slots
                            .bulk_p1
                            .as_ref()
                            .map_or(false, |c| c.generation == generation)
                        {
                            slots.bulk_p1 = None;
                        }
                    }
                }
                if slots.control_p0.is_none() && slots.bulk_p1.is_none() {
                    went_offline_user = Some(slots.user_id);
                    map.remove(&device_id);
                }
            }
        }

        if let Some(user_id) = went_offline_user {
            if let Some(db) = &self.db {
                let _ = db.set_device_online(device_id, false).await;
            }
            self.broadcast_presence_to_tenant(user_id, device_id, false)
                .await;
        }
    }

    async fn broadcast_presence_to_tenant(&self, user_id: Uuid, device_id: Uuid, is_online: bool) {
        let notice = Arc::new(RelayWireFrame::PeerPresence {
            device_id,
            is_online,
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
    }

    async fn process_inbound_frame(&self, sender: &RelayTicketClaims, frame: RelayWireFrame) {
        match frame {
            RelayWireFrame::KeepAlive { ts_ms } => {
                let reply = Arc::new(RelayWireFrame::KeepAlive { ts_ms });
                if let Some(tx) = self.pick_target_sender(sender.user_id, sender.device_id, TrafficLane::ControlP0).await {
                    let _ = tx.try_send(reply);
                }
            }
            RelayWireFrame::RoutePacket {
                dst_device_id,
                lane,
                e2ee,
                sender_x25519_pub_b64,
                ..
            } => {
                // If ticket pinned a specific peer_device_id, enforce it strictly
                if let Some(allowed_peer) = sender.peer_device_id {
                    if allowed_peer != dst_device_id {
                        return;
                    }
                }

                // Fair-queue token bucket shaping ONLY for BulkP1; ControlP0 is 0ms fast-path!
                if lane == TrafficLane::BulkP1 {
                    let delay_ms = {
                        let mut buckets = self.tenant_buckets.write().await;
                        buckets
                            .get_mut(&sender.user_id)
                            .map(|b| b.consume_and_delay_ms(e2ee.ciphertext_b64.len()))
                            .unwrap_or(0)
                    };
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }

                // Zero-knowledge blind forward: enforce `dst` belongs to SAME `user_id`!
                if let Some(target_tx) = self
                    .pick_target_sender(sender.user_id, dst_device_id, lane)
                    .await
                {
                    let forwarded = Arc::new(RelayWireFrame::RoutePacket {
                        src_device_id: sender.device_id,
                        dst_device_id,
                        lane,
                        e2ee,
                        sender_x25519_pub_b64,
                    });
                    let _ = target_tx.send(forwarded).await;
                } else if let Some(back_tx) = self
                    .pick_target_sender(sender.user_id, sender.device_id, TrafficLane::ControlP0)
                    .await
                {
                    let _ = back_tx.try_send(Arc::new(RelayWireFrame::Error {
                        code: "peer_offline".into(),
                        message: format!("target device {dst_device_id} is offline or in another tenant"),
                    }));
                }
            }
            _ => {}
        }
    }

    async fn pick_target_sender(
        &self,
        expected_user_id: Uuid,
        target_device_id: Uuid,
        preferred_lane: TrafficLane,
    ) -> Option<mpsc::Sender<Arc<RelayWireFrame>>> {
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
        chosen.map(|c| c.tx.clone())
    }
}

async fn read_length_delimited_frame<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<RelayWireFrame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_WIRE_FRAME_BYTES {
        anyhow::bail!("invalid wire frame length: {len}");
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}

fn parse_ws_message(msg: WsMessage) -> Option<RelayWireFrame> {
    match msg {
        WsMessage::Text(t) => serde_json::from_str::<RelayWireFrame>(&t).ok(),
        WsMessage::Binary(b) => serde_json::from_slice::<RelayWireFrame>(&b).ok(),
        _ => None,
    }
}
