//! Remote E2EE Mesh Hub, account sync, Smart-Path LAN Direct <-> Smart Relay hot-switch,
//! and embedded host control for `coda-desktop`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use coda_daemon::{
    active_daemon_lock_pid, change_account_password, delete_account_device, list_account_hosts,
    login_with_password_and_enroll_with_identity, mint_peer_relay_config,
    poll_device_login_until_approved, rename_account_device, start_device_login_flow,
    start_embedded_host_with_handle, subscribe_account_events_ws, DaemonConfig, EmbeddedHostHandle,
    LivePeerSession, PeerAsyncEvent, RemoteHostSummary,
};
use coda_protocol::{HerdrTerminalSummary, PeerMessage, RemoteFileItem, TrafficLane};
use uuid::Uuid;

use crate::config::{
    delete_secret_from_keychain, invalidate_secret_and_daemon_config_caches, is_safe_http_url,
    space_avatar_color, tr, DesktopClientIdentityRecord, DesktopPreferences,
};
use crate::git_inspector::{RepoInspectorSnapshot, WorkspaceFileEntry};
use crate::helpers::{
    format_pasted_terminal_path, herdr_status_to_osc, load_desktop_daemon_config,
    normalize_newlines_for_vt, parse_git_status_short, path_within_root,
    save_desktop_daemon_config,
};
use crate::port_preview::{
    PortPreviewTunnelManager, ProxyHttpRequest, ProxyHttpResponse, TunnelTargetKey,
};
use crate::pty::LocalPtyManager;
use crate::smart_path::DesktopSmartPathPolicy;
use crate::{
    ActiveInputTarget, CodaDesktopState, CodaGpuWorkspaceView, SpaceEntry, SplitPane,
    TerminalSessionEntry,
};

fn default_notify_true() -> bool {
    true
}

/// Zero-knowledge offline webhook push settings synced via `/api/v1/me/notifications`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NotificationWebhookSettings {
    #[serde(default)]
    pub bark_url: String,
    #[serde(default)]
    pub telegram_bot_token: String,
    #[serde(default)]
    pub telegram_chat_id: String,
    #[serde(default)]
    pub feishu_webhook_url: String,
    #[serde(default)]
    pub discord_webhook_url: String,
    #[serde(default)]
    pub generic_webhook_url: String,
    #[serde(default = "default_notify_true")]
    pub notify_on_needs_you: bool,
    #[serde(default = "default_notify_true")]
    pub notify_on_finished: bool,
}

impl Default for NotificationWebhookSettings {
    fn default() -> Self {
        Self {
            bark_url: String::new(),
            telegram_bot_token: String::new(),
            telegram_chat_id: String::new(),
            feishu_webhook_url: String::new(),
            discord_webhook_url: String::new(),
            generic_webhook_url: String::new(),
            notify_on_needs_you: true,
            notify_on_finished: true,
        }
    }
}

pub async fn api_get_notification_webhooks(
    server_url: &str,
    token: &str,
) -> Result<NotificationWebhookSettings, String> {
    let base = server_url.trim().trim_end_matches('/');
    if !is_safe_http_url(base) {
        return Err("Invalid server URL".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(token.trim())
        .send()
        .await
        .map_err(|e| format!("GET /api/v1/me/notifications failed: {e}"))?;
    let status = resp.status();
    let txt = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Load notification settings failed (HTTP {}): {}",
            status.as_u16(),
            txt.chars().take(160).collect::<String>()
        ));
    }
    serde_json::from_str::<NotificationWebhookSettings>(&txt)
        .map_err(|e| format!("Invalid notification settings JSON: {e}"))
}

pub async fn api_put_notification_webhooks(
    server_url: &str,
    token: &str,
    settings: &NotificationWebhookSettings,
) -> Result<NotificationWebhookSettings, String> {
    let base = server_url.trim().trim_end_matches('/');
    if !is_safe_http_url(base) {
        return Err("Invalid server URL".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .put(format!("{base}/api/v1/me/notifications"))
        .bearer_auth(token.trim())
        .json(settings)
        .send()
        .await
        .map_err(|e| format!("PUT /api/v1/me/notifications failed: {e}"))?;
    let status = resp.status();
    let txt = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Save notification settings failed (HTTP {}): {}",
            status.as_u16(),
            txt.chars().take(160).collect::<String>()
        ));
    }
    serde_json::from_str::<NotificationWebhookSettings>(&txt).or_else(|_| Ok(settings.clone()))
}

pub async fn api_test_notification_webhooks(
    server_url: &str,
    token: &str,
) -> Result<(usize, Vec<String>), String> {
    let base = server_url.trim().trim_end_matches('/');
    if !is_safe_http_url(base) {
        return Err("Invalid server URL".into());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{base}/api/v1/me/notifications/test"))
        .bearer_auth(token.trim())
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| format!("POST /api/v1/me/notifications/test failed: {e}"))?;
    let status = resp.status();
    let txt = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Test push failed (HTTP {}): {}",
            status.as_u16(),
            txt.chars().take(160).collect::<String>()
        ));
    }
    let val: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
    let delivered = val
        .get("delivered_channels")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let errors: Vec<String> = val
        .get("errors")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok((delivered, errors))
}

pub async fn api_prune_stale_web_devices(
    server_url: &str,
    token: &str,
    max_idle_days: u32,
    exclude_device_id: Option<Uuid>,
) -> Result<u64, String> {
    let base = server_url.trim().trim_end_matches('/');
    if !is_safe_http_url(base) {
        return Err("Invalid server URL".into());
    }
    let mut payload = serde_json::json!({ "max_idle_days": max_idle_days });
    if let Some(dev_id) = exclude_device_id {
        payload["exclude_device_id"] = serde_json::json!(dev_id);
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{base}/api/v1/devices/prune-stale"))
        .bearer_auth(token.trim())
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("POST /api/v1/devices/prune-stale failed: {e}"))?;
    let status = resp.status();
    let txt = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Prune web devices failed (HTTP {}): {}",
            status.as_u16(),
            txt.chars().take(160).collect::<String>()
        ));
    }
    let val: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
    let count = val
        .get("pruned_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok(count)
}

pub(crate) static EMBEDDED_HOST_STARTED: AtomicBool = AtomicBool::new(false);
static PRIMARY_CLIENT_IDENTITY_CLAIMED: AtomicBool = AtomicBool::new(false);

struct ProcessEmbeddedHostCoordinator {
    rt: tokio::runtime::Runtime,
    op_lock: tokio::sync::Mutex<()>,
    handle: Mutex<Option<EmbeddedHostHandle>>,
    op_gen: AtomicU64,
    active_hubs: AtomicUsize,
}

impl ProcessEmbeddedHostCoordinator {
    fn global() -> &'static Self {
        static INSTANCE: OnceLock<ProcessEmbeddedHostCoordinator> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(1)
                .build()
                .expect("failed to start Coda embedded host coordinator runtime");
            Self {
                rt,
                op_lock: tokio::sync::Mutex::new(()),
                handle: Mutex::new(None),
                op_gen: AtomicU64::new(0),
                active_hubs: AtomicUsize::new(0),
            }
        })
    }

    fn is_active(&self) -> bool {
        self.handle
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false)
    }

    fn stop_now(&self) {
        self.op_gen.fetch_add(1, Ordering::AcqRel);
        EMBEDDED_HOST_STARTED.store(false, Ordering::Release);
        if let Ok(mut guard) = self.handle.lock() {
            if let Some(h) = guard.take() {
                h.stop();
            }
        }
    }
}

pub struct RemoteMeshHub {
    rt: Option<tokio::runtime::Runtime>,
    state: Arc<Mutex<CodaDesktopState>>,
    pty_mgr: Arc<Mutex<LocalPtyManager>>,
    ui_dirty: Arc<AtomicBool>,
    peers: tokio::sync::Mutex<HashMap<Uuid, Arc<LivePeerSession>>>,
    peers_sync: std::sync::RwLock<HashMap<Uuid, Arc<LivePeerSession>>>,
    client_identities:
        tokio::sync::Mutex<HashMap<Uuid, (Uuid, coda_protocol::StoredDeviceIdentity)>>,
    owns_primary_client_identity: AtomicBool,
    attached_terms: tokio::sync::Mutex<HashSet<(Uuid, String)>>,
    peer_connect_locks: tokio::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    smart_path: tokio::sync::Mutex<DesktopSmartPathPolicy>,
    tunnel_mgr: Arc<PortPreviewTunnelManager>,
    watcher_gen: AtomicU64,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    did_shutdown: AtomicBool,
}

impl Drop for RemoteMeshHub {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl RemoteMeshHub {
    pub fn new(
        state: Arc<Mutex<CodaDesktopState>>,
        pty_mgr: Arc<Mutex<LocalPtyManager>>,
        ui_dirty: Arc<AtomicBool>,
    ) -> Arc<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("failed to start Coda mesh tokio runtime");
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        let claimed_primary = PRIMARY_CLIENT_IDENTITY_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        ProcessEmbeddedHostCoordinator::global()
            .active_hubs
            .fetch_add(1, Ordering::AcqRel);
        Arc::new(Self {
            rt: Some(rt),
            state,
            pty_mgr,
            ui_dirty,
            peers: tokio::sync::Mutex::new(HashMap::new()),
            peers_sync: std::sync::RwLock::new(HashMap::new()),
            client_identities: tokio::sync::Mutex::new(HashMap::new()),
            owns_primary_client_identity: AtomicBool::new(claimed_primary),
            attached_terms: tokio::sync::Mutex::new(HashSet::new()),
            peer_connect_locks: tokio::sync::Mutex::new(HashMap::new()),
            smart_path: tokio::sync::Mutex::new(DesktopSmartPathPolicy::default()),
            tunnel_mgr: PortPreviewTunnelManager::new(),
            watcher_gen: AtomicU64::new(0),
            shutdown_tx,
            did_shutdown: AtomicBool::new(false),
        })
    }

    /// Cancels all background watchers, closes all live peers and loopback port tunnels, and stops
    /// the process-global embedded host if no other desktop window remains open (`Bug #1`).
    pub fn shutdown(&self) {
        if self
            .did_shutdown
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.watcher_gen.fetch_add(1, Ordering::AcqRel);
        let _ = self.shutdown_tx.send(true);
        self.tunnel_mgr.stop_all();

        let peers_to_close: Vec<Arc<LivePeerSession>> = self
            .peers_sync
            .write()
            .map(|mut m| m.drain().map(|(_, p)| p).collect())
            .unwrap_or_default();
        for peer in peers_to_close {
            peer.close();
        }
        if let Ok(mut map) = self.peers.try_lock() {
            map.clear();
        }

        if self
            .owns_primary_client_identity
            .swap(false, Ordering::AcqRel)
        {
            PRIMARY_CLIENT_IDENTITY_CLAIMED.store(false, Ordering::Release);
        }

        let coord = ProcessEmbeddedHostCoordinator::global();
        let prev = coord.active_hubs.fetch_sub(1, Ordering::AcqRel);
        if prev <= 1 {
            coord.stop_now();
        }
    }

    pub fn stop_all_tunnels_quiet(&self) {
        self.tunnel_mgr.stop_all();
        if let Ok(mut g) = self.state.lock() {
            g.port_preview.clear_tunnels();
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    async fn insert_peer(&self, host_id: Uuid, peer: Arc<LivePeerSession>) -> Option<Arc<LivePeerSession>> {
        let mut map = self.peers.lock().await;
        let old = map.insert(host_id, Arc::clone(&peer));
        if let Ok(mut sync_map) = self.peers_sync.write() {
            sync_map.insert(host_id, peer);
        }
        old
    }

    async fn remove_peer(&self, host_id: Uuid) -> Option<Arc<LivePeerSession>> {
        let mut map = self.peers.lock().await;
        let old = map.remove(&host_id);
        if let Ok(mut sync_map) = self.peers_sync.write() {
            sync_map.remove(&host_id);
        }
        old
    }

    async fn remove_peer_if_ptr_eq(&self, host_id: Uuid, expected: &Arc<LivePeerSession>) -> bool {
        let mut map = self.peers.lock().await;
        if map.get(&host_id).map(|cur| Arc::ptr_eq(cur, expected)).unwrap_or(false) {
            map.remove(&host_id);
            if let Ok(mut sync_map) = self.peers_sync.write() {
                if sync_map.get(&host_id).map(|cur| Arc::ptr_eq(cur, expected)).unwrap_or(false) {
                    sync_map.remove(&host_id);
                }
            }
            true
        } else {
            false
        }
    }

    async fn drain_peers(&self) -> Vec<Arc<LivePeerSession>> {
        let mut map = self.peers.lock().await;
        let drained: Vec<Arc<LivePeerSession>> = map.drain().map(|(_, p)| p).collect();
        if let Ok(mut sync_map) = self.peers_sync.write() {
            sync_map.clear();
        }
        drained
    }

    fn spawn_bg<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        if *self.shutdown_tx.borrow() {
            return;
        }
        if let Some(ref rt) = self.rt {
            rt.spawn(fut);
        }
    }

    pub fn ensure_embedded_host(self: &Arc<Self>) {
        let cfg_path = DaemonConfig::default_path();
        let Ok(cfg) = load_desktop_daemon_config(&cfg_path) else {
            return;
        };
        let allow_remote = self
            .state
            .lock()
            .map(|mut g| {
                g.local_host_device_id = Some(cfg.device_id);
                g.allow_remote_control
            })
            .unwrap_or(true);
        let coord = ProcessEmbeddedHostCoordinator::global();
        if coord.is_active() && allow_remote {
            EMBEDDED_HOST_STARTED.store(true, Ordering::Release);
            if let Ok(mut g) = self.state.lock() {
                g.embedded_host_active = true;
                g.external_daemon_running = false;
            }
            self.ui_dirty.store(true, Ordering::Release);
            return;
        }
        let ext_pid = active_daemon_lock_pid(&cfg_path);
        let external_running = ext_pid.map(|p| p != std::process::id()).unwrap_or(false);
        if let Ok(mut g) = self.state.lock() {
            g.external_daemon_running = external_running;
            if !allow_remote {
                g.embedded_host_active = false;
            }
        }
        if !allow_remote || external_running {
            self.ui_dirty.store(true, Ordering::Release);
            return;
        }
        let my_gen = coord.op_gen.fetch_add(1, Ordering::AcqRel) + 1;
        let weak_hub = Arc::downgrade(self);
        coord.rt.spawn(async move {
            let _op_guard = coord.op_lock.lock().await;
            if coord.op_gen.load(Ordering::Acquire) != my_gen || coord.active_hubs.load(Ordering::Acquire) == 0 {
                return;
            }
            if coord.is_active() {
                EMBEDDED_HOST_STARTED.store(true, Ordering::Release);
                if let Some(hub) = weak_hub.upgrade() {
                    if let Ok(mut g) = hub.state.lock() {
                        g.embedded_host_active = true;
                        g.external_daemon_running = false;
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                return;
            }
            EMBEDDED_HOST_STARTED.store(true, Ordering::Release);
            let mut started_handle = None;
            for attempt in 0..3 {
                if coord.op_gen.load(Ordering::Acquire) != my_gen
                    || coord.active_hubs.load(Ordering::Acquire) == 0
                {
                    break;
                }
                if let Some(handle) = start_embedded_host_with_handle(&cfg_path).await {
                    started_handle = Some(handle);
                    break;
                }
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                }
            }
            if let Some(handle) = started_handle {
                if coord.op_gen.load(Ordering::Acquire) != my_gen
                    || coord.active_hubs.load(Ordering::Acquire) == 0
                    || !EMBEDDED_HOST_STARTED.load(Ordering::Acquire)
                {
                    handle.stop();
                    EMBEDDED_HOST_STARTED.store(false, Ordering::Release);
                    return;
                }
                if let Ok(mut slot) = coord.handle.lock() {
                    *slot = Some(handle);
                }
                if let Some(hub) = weak_hub.upgrade() {
                    if let Ok(mut g) = hub.state.lock() {
                        g.embedded_host_active = true;
                        g.external_daemon_running = false;
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
            } else {
                EMBEDDED_HOST_STARTED.store(false, Ordering::Release);
                if let Some(hub) = weak_hub.upgrade() {
                    if let Ok(mut g) = hub.state.lock() {
                        g.embedded_host_active = false;
                        g.external_daemon_running = active_daemon_lock_pid(&cfg_path)
                            .map(|p| p != std::process::id())
                            .unwrap_or(false);
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
            }
        });
    }

    pub fn set_allow_remote_control(self: &Arc<Self>, enabled: bool) {
        if let Ok(mut g) = self.state.lock() {
            g.allow_remote_control = enabled;
            let notice = if enabled {
                tr(
                    g.locale,
                    "Remote control enabled for this Mac",
                    "已允许其他设备远端控制本机",
                )
            } else if g.external_daemon_running {
                tr(
                    g.locale,
                    "Embedded host off (standalone coda-daemon is still running)",
                    "已关闭内嵌被控（独立 coda-daemon 仍在后台运行）",
                )
            } else {
                tr(
                    g.locale,
                    "Remote control disabled (controller-only mode)",
                    "已关闭本机远端被控（仅控制端模式）",
                )
            };
            g.status_notice = Some(notice.to_string());
        }
        CodaGpuWorkspaceView::update_prefs_async(move |p| {
            p.allow_remote_control = enabled;
        });
        if enabled {
            self.ensure_embedded_host();
        } else {
            ProcessEmbeddedHostCoordinator::global().stop_now();
            if let Ok(mut g) = self.state.lock() {
                g.embedded_host_active = false;
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    /// Cleanly tears down old account peers, remote sessions, and embedded host before switching accounts.
    pub async fn reset_for_account_switch(self: &Arc<Self>) {
        self.watcher_gen.fetch_add(1, Ordering::AcqRel);
        ProcessEmbeddedHostCoordinator::global().stop_now();
        for peer in self.drain_peers().await {
            peer.close();
        }
        self.attached_terms.lock().await.clear();
        self.client_identities.lock().await.clear();
        self.peer_connect_locks.lock().await.clear();
        self.smart_path.lock().await.reset_all();
        self.tunnel_mgr.stop_all();
        DesktopClientIdentityRecord::remove_saved();
        invalidate_secret_and_daemon_config_caches();

        let mut removed_remote_sids = Vec::new();
        if let Ok(mut g) = self.state.lock() {
            for s in &g.sessions {
                if s.remote_host_id.is_some() {
                    removed_remote_sids.push(s.id.clone());
                }
            }
            g.embedded_host_active = false;
            g.remote_hosts.clear();
            g.remote_probe_status = None;
            g.webhook_settings = NotificationWebhookSettings::default();
            g.webhook_loading = false;
            g.webhook_loaded_ok = false;
            g.webhook_dirty = false;
            g.remote_diff_seq = g.remote_diff_seq.wrapping_add(1);
            g.git_diff_loading = false;
            g.port_preview.clear_tunnels();
            g.bootstrap_modal.reset();
            g.clear_settings_ephemeral_inputs();
            g.notify_tracker.reset();
            g.spaces.retain(|sp| !sp.is_remote);
            g.sessions.retain(|s| s.remote_host_id.is_none());
            if g.active_space().is_none() {
                g.active_space_id = g.spaces.first().map(|sp| sp.id.clone());
            }
            if let Some(ref aid) = g.active_session_id {
                if !g.sessions.iter().any(|s| s.id == *aid) {
                    g.active_session_id = g.sessions.first().map(|s| s.id.clone());
                }
            }
            if let Some(ref sid) = g.split_session_id {
                if !g.sessions.iter().any(|s| s.id == *sid) {
                    g.split_session_id = None;
                }
            }
        }
        if let Ok(mut mgr) = self.pty_mgr.lock() {
            for sid in removed_remote_sids {
                mgr.remove_remote_buffer(&sid);
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    /// Manually resets Smart-Path backoff and immediately probes LAN Direct for all active Relay peers.
    pub fn probe_all_relay_peers_now(self: &Arc<Self>) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            hub.smart_path.lock().await.reset_all();
            let hosts: Vec<RemoteHostSummary> = hub
                .state
                .lock()
                .map(|g| g.remote_hosts.iter().filter(|h| h.is_online).cloned().collect())
                .unwrap_or_default();
            let mut upgraded_any = false;
            let mut probed_any = false;
            for host in hosts {
                let is_relay = hub
                    .peers
                    .lock()
                    .await
                    .get(&host.id)
                    .map(|p| !p.is_direct_p2p())
                    .unwrap_or(false);
                if is_relay {
                    probed_any = true;
                    if hub.try_hot_upgrade_peer_to_direct(&host).await {
                        upgraded_any = true;
                    }
                }
            }
            if let Ok(mut g) = hub.state.lock() {
                if !probed_any {
                    g.status_notice = Some(
                        tr(
                            g.locale,
                            "All online remote hosts are already on Direct P2P (or idle)",
                            "当前在线主机均已处于直连状态（或暂无中继连接）",
                        )
                        .to_string(),
                    );
                } else if !upgraded_any {
                    g.status_notice = Some(
                        tr(
                            g.locale,
                            "LAN Direct unreachable on current network — staying on Smart Relay",
                            "当前网络暂无法直连远端主机，继续通过智能中继传输",
                        )
                        .to_string(),
                    );
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn rename_remote_device(self: &Arc<Self>, device_id: Uuid, new_name: String) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                return;
            };
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            match rename_account_device(&cfg.server_url, tok, device_id, &new_name).await {
                Ok(()) => {
                    let host_str = device_id.to_string();
                    if let Ok(mut g) = hub.state.lock() {
                        for h in &mut g.remote_hosts {
                            if h.id == device_id {
                                h.name = new_name.clone();
                            }
                        }
                        for sp in &mut g.spaces {
                            if sp.machine_id == host_str {
                                sp.machine_label = new_name.clone();
                                if sp.id == format!("remote:{device_id}") {
                                    sp.name = new_name.clone();
                                }
                            }
                        }
                        g.status_notice = Some(format!(
                            "{}: {new_name}",
                            tr(g.locale, "Renamed machine", "主机已重命名")
                        ));
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Rename device failed: {err}"));
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn change_password(self: &Arc<Self>, current_password: String, new_password: String) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let cfg_path = DaemonConfig::default_path();
            let Ok(mut cfg) = load_desktop_daemon_config(&cfg_path) else {
                return;
            };
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            match change_account_password(&cfg.server_url, tok, &current_password, &new_password)
                .await
            {
                Ok(rotated_tok) => {
                    if let Some(new_tok) = rotated_tok {
                        cfg.session_token = Some(new_tok);
                        let _ = save_desktop_daemon_config(&cfg, &cfg_path);
                    }
                    if let Ok(mut g) = hub.state.lock() {
                        g.pw_change_expanded = false;
                        g.auth_error = None;
                        g.status_notice = Some(
                            tr(g.locale, "Password updated", "密码已更新").to_string(),
                        );
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        let msg = format!("{err}");
                        g.auth_error = Some(msg.clone());
                        g.status_notice = Some(msg);
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn delete_remote_device(self: &Arc<Self>, device_id: Uuid) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                return;
            };
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            match delete_account_device(&cfg.server_url, tok, device_id).await {
                Ok(()) => {
                    hub.tunnel_mgr.stop_all_for_device(device_id);
                    hub.drop_peer(device_id).await;
                    let host_str = device_id.to_string();
                    let mut removed_sids = Vec::new();
                    if let Ok(mut g) = hub.state.lock() {
                        g.remote_hosts.retain(|h| h.id != device_id);
                        let hosts_snap = g.remote_hosts.clone();
                        g.port_preview
                            .sync_from_manager(&hub.tunnel_mgr, &hosts_snap, None);
                        g.port_preview.tunnel_starting = false;
                        if g.bootstrap_modal.host_id == Some(device_id) {
                            g.bootstrap_modal.reset();
                        }
                        g.spaces.retain(|sp| sp.machine_id != host_str);
                        if g.renaming_host_id == Some(device_id) {
                            g.renaming_host_id = None;
                            g.rename_host_input.clear();
                            if g.input_target == ActiveInputTarget::RenameDeviceInput {
                                g.input_target = ActiveInputTarget::TerminalDirect;
                            }
                        }
                        for s in &g.sessions {
                            if s.remote_host_id.as_deref() == Some(host_str.as_str()) {
                                removed_sids.push(s.id.clone());
                            }
                        }
                        for sid in &removed_sids {
                            g.notify_tracker.prune_session(sid);
                        }
                        g.sessions
                            .retain(|s| s.remote_host_id.as_deref() != Some(host_str.as_str()));
                        if g.active_space().is_none() {
                            g.active_space_id = g.spaces.first().map(|sp| sp.id.clone());
                        }
                        if let Some(ref aid) = g.active_session_id {
                            if !g.sessions.iter().any(|s| s.id == *aid) {
                                g.active_session_id = g.sessions.first().map(|s| s.id.clone());
                            }
                        }
                        if let Some(ref sid) = g.split_session_id {
                            if !g.sessions.iter().any(|s| s.id == *sid) {
                                g.split_session_id = None;
                            }
                        }
                        g.status_notice = Some(
                            tr(g.locale, "Removed remote machine", "已移除远端主机").to_string(),
                        );
                    }
                    if let Ok(mut mgr) = hub.pty_mgr.lock() {
                        for sid in removed_sids {
                            mgr.remove_remote_buffer(&sid);
                        }
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remove device failed: {err}"));
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn sign_out(self: &Arc<Self>) {
        self.watcher_gen.fetch_add(1, Ordering::AcqRel);
        self.tunnel_mgr.stop_all();
        ProcessEmbeddedHostCoordinator::global().stop_now();

        let cfg_path = DaemonConfig::default_path();
        let prev_cfg = load_desktop_daemon_config(&cfg_path).ok();
        let external_daemon_owns_cfg = active_daemon_lock_pid(&cfg_path)
            .map(|pid| pid != std::process::id())
            .unwrap_or(false);
        if !external_daemon_owns_cfg {
            let _ = std::fs::remove_file(&cfg_path);
        }
        delete_secret_from_keychain("session_token");
        DesktopClientIdentityRecord::remove_saved();
        invalidate_secret_and_daemon_config_caches();

        let mut removed_remote_sids = Vec::new();
        if let Ok(mut g) = self.state.lock() {
            for s in &g.sessions {
                if s.remote_host_id.is_some() {
                    removed_remote_sids.push(s.id.clone());
                }
            }
            g.is_logged_in = false;
            g.auth_form_expanded = false;
            g.auth_loading = false;
            g.auth_error = None;
            g.embedded_host_active = false;
            g.external_daemon_running = external_daemon_owns_cfg;
            g.account_email.clear();
            g.auth_account_input.clear();
            g.auth_password_input.clear();
            g.webhook_settings = NotificationWebhookSettings::default();
            g.webhook_loading = false;
            g.webhook_loaded_ok = false;
            g.webhook_dirty = false;
            g.remote_diff_seq = g.remote_diff_seq.wrapping_add(1);
            g.git_diff_loading = false;
            g.port_preview.clear_tunnels();
            g.bootstrap_modal.reset();
            g.clear_settings_ephemeral_inputs();
            g.notify_tracker.reset();
            g.device_login_code = None;
            g.device_login_uri = None;
            g.remote_hosts.clear();
            g.remote_probe_status = None;
            g.spaces.retain(|sp| !sp.is_remote);
            g.sessions.retain(|s| s.remote_host_id.is_none());
            if g.active_space().is_none() {
                g.active_space_id = g.spaces.first().map(|sp| sp.id.clone());
            }
            if let Some(ref aid) = g.active_session_id {
                if !g.sessions.iter().any(|s| s.id == *aid) {
                    g.active_session_id = g.sessions.first().map(|s| s.id.clone());
                }
            }
            if let Some(ref sid) = g.split_session_id {
                if !g.sessions.iter().any(|s| s.id == *sid) {
                    g.split_session_id = None;
                }
            }
            g.input_target = ActiveInputTarget::TerminalDirect;
            g.status_notice = Some(
                if external_daemon_owns_cfg {
                    tr(
                        g.locale,
                        "Signed out (kept standalone coda-daemon config)",
                        "已退出登录（保留独立 coda-daemon 后台配置）",
                    )
                } else {
                    tr(g.locale, "Signed out", "已退出登录")
                }
                .to_string(),
            );
        }
        if let Ok(mut mgr) = self.pty_mgr.lock() {
            for sid in removed_remote_sids {
                mgr.remove_remote_buffer(&sid);
            }
        }
        self.ui_dirty.store(true, Ordering::Release);

        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            for peer in hub.drain_peers().await {
                peer.close();
            }
            hub.attached_terms.lock().await.clear();
            hub.client_identities.lock().await.clear();
            hub.peer_connect_locks.lock().await.clear();
            hub.smart_path.lock().await.reset_all();
            if !external_daemon_owns_cfg {
                if let Some(cfg) = prev_cfg {
                    if let Some(ref tok) = cfg.session_token {
                        let _ = coda_daemon::logout_account_session(&cfg.server_url, tok).await;
                    }
                }
            }
        });
    }

    pub fn start_account_sync(self: &Arc<Self>) {
        let my_gen = self.watcher_gen.fetch_add(1, Ordering::AcqRel) + 1;
        let weak_hub = Arc::downgrade(self);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        self.spawn_bg(async move {
            if let Some(hub) = weak_hub.upgrade() {
                hub.sync_all_hosts_and_sessions().await;
            } else {
                return;
            }

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                let Some(cfg) = ({
                    let Some(hub) = weak_hub.upgrade() else {
                        break;
                    };
                    if hub.watcher_gen.load(Ordering::Acquire) != my_gen {
                        break;
                    }
                    load_desktop_daemon_config(&DaemonConfig::default_path()).ok()
                }) else {
                    break;
                };
                let Some(ref tok) = cfg.session_token else {
                    break;
                };

                match subscribe_account_events_ws(&cfg.server_url, tok).await {
                    Ok(mut rx) => loop {
                        let ev_opt = tokio::select! {
                            changed = shutdown_rx.changed() => {
                                if changed.is_err() || *shutdown_rx.borrow() {
                                    return;
                                }
                                continue;
                            }
                            ev = rx.recv() => ev,
                        };
                        let Some(ev) = ev_opt else {
                            break;
                        };
                        let Some(hub) = weak_hub.upgrade() else {
                            return;
                        };
                        if hub.watcher_gen.load(Ordering::Acquire) != my_gen {
                            return;
                        }
                        if ev.role.as_deref() == Some("client") {
                            continue;
                        }
                        if matches!(
                            ev.kind.as_str(),
                            "connected" | "devices_changed" | "peer_presence"
                        ) {
                            hub.smart_path.lock().await.reset_all();
                            hub.sync_all_hosts_and_sessions().await;
                        }
                    },
                    Err(e) => {
                        tracing::debug!("account /api/v1/events/ws reconnect scheduled: {e}");
                    }
                }
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
        });
    }

    async fn sync_all_hosts_and_sessions(self: &Arc<Self>) {
        let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
            return;
        };
        let Some(ref tok) = cfg.session_token else {
            return;
        };
        let Ok(list) = list_account_hosts(&cfg.server_url, tok).await else {
            return;
        };
        let local_host_id = self
            .state
            .lock()
            .ok()
            .and_then(|g| g.local_host_device_id);
        let hosts: Vec<RemoteHostSummary> = list
            .into_iter()
            .filter(|h| h.id != cfg.device_id && Some(h.id) != local_host_id)
            .collect();

        let live_host_ids: HashSet<Uuid> = hosts.iter().map(|h| h.id).collect();
        let live_host_strs: HashSet<String> = hosts.iter().map(|h| h.id.to_string()).collect();
        let mut stale_host_ids: Vec<Uuid> = Vec::new();
        let mut pruned_sids: Vec<String> = Vec::new();
        {
            let Ok(mut g) = self.state.lock() else {
                return;
            };
            for prev_h in &g.remote_hosts {
                if !live_host_ids.contains(&prev_h.id) {
                    stale_host_ids.push(prev_h.id);
                }
            }
            // V3-31: Prune spaces & sessions belonging to hosts removed from the account.
            g.spaces
                .retain(|sp| !sp.is_remote || live_host_strs.contains(&sp.machine_id));
            for s in &g.sessions {
                if let Some(ref hid) = s.remote_host_id {
                    if !live_host_strs.contains(hid) {
                        pruned_sids.push(s.id.clone());
                    }
                }
            }
            if !pruned_sids.is_empty() {
                let pruned_set: HashSet<&str> = pruned_sids.iter().map(String::as_str).collect();
                for sid in &pruned_sids {
                    g.notify_tracker.prune_session(sid);
                }
                g.sessions.retain(|s| !pruned_set.contains(s.id.as_str()));
            }
            if let Some(ren_id) = g.renaming_host_id {
                if !live_host_ids.contains(&ren_id) {
                    g.renaming_host_id = None;
                    g.rename_host_input.clear();
                    if g.input_target == ActiveInputTarget::RenameDeviceInput {
                        g.input_target = ActiveInputTarget::TerminalDirect;
                    }
                }
            }
            if g.active_space().is_none() {
                g.active_space_id = g.spaces.first().map(|sp| sp.id.clone());
            }
            if let Some(ref aid) = g.active_session_id {
                if !g.sessions.iter().any(|s| s.id == *aid) {
                    g.active_session_id = g.sessions.first().map(|s| s.id.clone());
                }
            }
            if let Some(ref sid) = g.split_session_id {
                if !g.sessions.iter().any(|s| s.id == *sid) {
                    g.split_session_id = None;
                }
            }

            let online_map: HashMap<String, bool> =
                hosts.iter().map(|h| (h.id.to_string(), h.is_online)).collect();
            for sp in &mut g.spaces {
                if sp.is_remote {
                    match online_map.get(&sp.machine_id) {
                        Some(true) => {
                            if sp.branch == "offline" {
                                sp.branch = "online".to_string();
                            }
                        }
                        _ => sp.branch = "offline".to_string(),
                    }
                }
            }
            for host in &hosts {
                let sid = format!("remote:{}", host.id);
                if !g.spaces.iter().any(|s| s.machine_id == host.id.to_string()) {
                    g.spaces.push(SpaceEntry {
                        id: sid,
                        avatar_color: space_avatar_color(&host.name),
                        name: host.name.clone(),
                        path: ".".to_string(),
                        machine_id: host.id.to_string(),
                        machine_label: host.name.clone(),
                        is_remote: true,
                        branch: if host.is_online { "online" } else { "offline" }.to_string(),
                        dirty_files: 0,
                        ahead: 0,
                        behind: 0,
                    });
                }
            }
            let mut merged_hosts = hosts.clone();
            for h in &mut merged_hosts {
                if h.is_online && h.health.is_none() {
                    if let Some(prev) = g.remote_hosts.iter().find(|p| p.id == h.id) {
                        h.health = prev.health.clone();
                    }
                }
            }
            g.remote_hosts = merged_hosts;
        }
        if !pruned_sids.is_empty() {
            if let Ok(mut mgr) = self.pty_mgr.lock() {
                for sid in pruned_sids {
                    mgr.remove_remote_buffer(&sid);
                }
            }
        }
        for stale_id in stale_host_ids {
            self.drop_peer(stale_id).await;
        }
        self.ui_dirty.store(true, Ordering::Release);

        for host in &hosts {
            if !host.is_online {
                self.drop_peer(host.id).await;
                continue;
            }
            self.sync_single_remote_host(&cfg, host).await;
        }
    }

    async fn drop_peer(self: &Arc<Self>, host_id: Uuid) {
        if let Some(peer) = self.remove_peer(host_id).await {
            peer.close();
        }
        self.mark_host_sessions_disconnected_if_no_peer(host_id).await;
    }

    async fn drop_peer_if_ptr_eq(
        self: &Arc<Self>,
        host_id: Uuid,
        expected: &Arc<LivePeerSession>,
    ) {
        if self.remove_peer_if_ptr_eq(host_id, expected).await {
            expected.close();
        }
        self.mark_host_sessions_disconnected_if_no_peer(host_id).await;
    }

    async fn mark_host_sessions_disconnected_if_no_peer(&self, host_id: Uuid) {
        if self.peers.lock().await.contains_key(&host_id) {
            return;
        }
        {
            let mut att = self.attached_terms.lock().await;
            att.retain(|(id, _)| *id != host_id);
        }
        let host_str = host_id.to_string();
        let affected: Vec<String> = self
            .state
            .lock()
            .map(|mut g| {
                let mut ids = Vec::new();
                for s in &mut g.sessions {
                    if s.remote_host_id.as_deref() == Some(host_str.as_str()) {
                        s.alive = false;
                        ids.push(s.id.clone());
                    }
                }
                ids
            })
            .unwrap_or_default();
        if let Ok(mgr) = self.pty_mgr.lock() {
            for sid in affected {
                mgr.set_remote_alive(&sid, false);
            }
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    async fn resolve_client_identity(
        self: &Arc<Self>,
        cfg: &DaemonConfig,
    ) -> (Uuid, coda_protocol::StoredDeviceIdentity, bool) {
        if let Some(existing) = self.client_identities.lock().await.get(&Uuid::nil()).cloned() {
            return (existing.0, existing.1, true);
        }
        let owns_primary = self.owns_primary_client_identity.load(Ordering::Acquire);
        if let Some(tok) = cfg.session_token.as_deref() {
            if owns_primary {
                if let Some((saved_id, saved_ident)) =
                    DesktopClientIdentityRecord::load_matching(&cfg.server_url, tok)
                {
                    self.client_identities
                        .lock()
                        .await
                        .insert(Uuid::nil(), (saved_id, saved_ident.clone()));
                    return (saved_id, saved_ident, true);
                }
            }
            let client_name = if owns_primary {
                "desktop-client"
            } else {
                "desktop-window"
            };
            if let Ok(created) = coda_daemon::register_client_identity(
                &cfg.server_url,
                tok,
                client_name,
                "macos",
            )
            .await
            {
                if owns_primary {
                    let _ = DesktopClientIdentityRecord::save_for_token(
                        &cfg.server_url,
                        tok,
                        created.0,
                        &created.1,
                    );
                }
                self.client_identities
                    .lock()
                    .await
                    .insert(Uuid::nil(), created.clone());
                return (created.0, created.1, false);
            }
        }
        (cfg.device_id, cfg.identity.clone(), false)
    }

    async fn reattach_terms_preserving_screen(
        self: &Arc<Self>,
        peer: &Arc<LivePeerSession>,
        host_id: Uuid,
    ) {
        let terms: Vec<String> = self
            .attached_terms
            .lock()
            .await
            .iter()
            .filter(|(hid, _)| *hid == host_id)
            .map(|(_, t)| t.clone())
            .collect();
        for term_addr in terms {
            let sid = format!("remote:{host_id}:{term_addr}");
            let (cols, rows) = self
                .pty_mgr
                .lock()
                .ok()
                .and_then(|mgr| mgr.session_dims(&sid))
                .unwrap_or((120, 36));
            let _ = peer
                .request(
                    PeerMessage::PtyAttach {
                        req_id: format!("hot-{}", Uuid::new_v4().simple()),
                        terminal: term_addr,
                        cols,
                        rows,
                    },
                    TrafficLane::ControlP0,
                    Duration::from_secs(5),
                )
                .await;
        }
    }

    /// Background Smart-Path hot-upgrade from Smart Relay to Direct LAN/IPv6 P2P with zero black screen.
    async fn try_hot_upgrade_peer_to_direct(self: &Arc<Self>, host: &RemoteHostSummary) -> bool {
        let connect_lock = {
            let mut locks = self.peer_connect_locks.lock().await;
            Arc::clone(
                locks
                    .entry(host.id)
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _connect_guard = connect_lock.lock().await;

        let current_peer = match self.peers.lock().await.get(&host.id).cloned() {
            Some(p) if !p.is_direct_p2p() => p,
            _ => return false,
        };
        let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
            return false;
        };
        let (client_dev_id, client_identity, _) = self.resolve_client_identity(&cfg).await;

        let Ok(direct_sess) = LivePeerSession::connect_auto(
            client_identity,
            client_dev_id,
            host.id,
            host.identity_pin(),
            host.endpoints.clone(),
            Some("255.255.255.255:24444".parse().unwrap()),
            None,
        )
        .await
        else {
            return false;
        };
        if !direct_sess.is_direct_p2p() {
            direct_sess.close();
            return false;
        }

        let new_peer = Arc::new(direct_sess);
        let route_label = new_peer.route_label().to_string();

        // V3-01: Atomically swap active peer BEFORE re-attaching so the old Relay event loop
        // immediately stops forwarding duplicate PTY chunks, then detach & close old Relay.
        let swapped_old = {
            let mut map = self.peers.lock().await;
            let still_same = map
                .get(&host.id)
                .map(|p| Arc::ptr_eq(p, &current_peer))
                .unwrap_or(false);
            if still_same {
                let old = map.insert(host.id, Arc::clone(&new_peer));
                if let Ok(mut sync_map) = self.peers_sync.write() {
                    sync_map.insert(host.id, Arc::clone(&new_peer));
                }
                old
            } else {
                None
            }
        };

        if let Some(old_relay) = swapped_old {
            let terms_to_detach: Vec<String> = self
                .attached_terms
                .lock()
                .await
                .iter()
                .filter(|(hid, _)| *hid == host.id)
                .map(|(_, t)| t.clone())
                .collect();
            for term_addr in terms_to_detach {
                let _ = old_relay.try_send_oneway(
                    PeerMessage::PtyDetach {
                        req_id: format!("hdet-{}", Uuid::new_v4().simple()),
                        terminal: term_addr,
                    },
                    TrafficLane::ControlP0,
                );
            }
            old_relay.close();

            self.spawn_peer_event_loop(Arc::clone(&new_peer), host.clone());
            self.reattach_terms_preserving_screen(&new_peer, host.id)
                .await;

            self.smart_path
                .lock()
                .await
                .record_upgrade_success(host.id);
            if let Ok(mut g) = self.state.lock() {
                g.remote_probe_status = Some(format!("{} · {route_label}", host.name));
                g.status_notice = Some(format!(
                    "{} · {} ({route_label})",
                    tr(
                        g.locale,
                        "Hot-switched seamlessly to LAN Direct",
                        "已无感热切至局域网直连"
                    ),
                    host.name
                ));
            }
            self.ui_dirty.store(true, Ordering::Release);
            true
        } else {
            new_peer.close();
            false
        }
    }

    fn spawn_lan_upgrade_watcher(
        self: &Arc<Self>,
        relay_peer: Arc<LivePeerSession>,
        host: RemoteHostSummary,
    ) {
        let weak_hub = Arc::downgrade(self);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let my_gen = self.watcher_gen.load(Ordering::Acquire);
        self.spawn_bg(async move {
            loop {
                let Some(delay) = ({
                    let Some(hub) = weak_hub.upgrade() else {
                        break;
                    };
                    let d = hub.smart_path.lock().await.next_delay(host.id);
                    Some(d)
                }) else {
                    break;
                };

                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(delay) => {}
                }

                let Some(hub) = weak_hub.upgrade() else {
                    break;
                };
                if hub.watcher_gen.load(Ordering::Acquire) != my_gen {
                    break;
                }
                let still_active_relay = {
                    let map = hub.peers.lock().await;
                    map.get(&host.id)
                        .map(|p| Arc::ptr_eq(p, &relay_peer) && !p.is_direct_p2p())
                        .unwrap_or(false)
                };
                if !still_active_relay {
                    break;
                }
                let latest_host = hub
                    .state
                    .lock()
                    .ok()
                    .and_then(|g| g.remote_hosts.iter().find(|h| h.id == host.id).cloned())
                    .unwrap_or_else(|| host.clone());
                if !latest_host.is_online {
                    break;
                }
                let can_probe = hub
                    .smart_path
                    .lock()
                    .await
                    .can_probe_now(host.id, false, Instant::now());
                if !can_probe {
                    continue;
                }
                if hub.try_hot_upgrade_peer_to_direct(&latest_host).await {
                    break;
                }
                hub.smart_path.lock().await.record_probe_failure(host.id);
            }
        });
    }

    fn spawn_peer_event_loop(
        self: &Arc<Self>,
        arc_sess: Arc<LivePeerSession>,
        host: RemoteHostSummary,
    ) {
        let mut rx = arc_sess.subscribe_events();
        let weak_hub = Arc::downgrade(self);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let host_id = host.id;
        let host_clone = host.clone();
        let peer_for_task = Arc::clone(&arc_sess);
        let sync_notify = Arc::new(tokio::sync::Notify::new());
        let sync_stop = Arc::new(AtomicBool::new(false));
        {
            // Decouple `HerdrLiveEvent` full-list RPC sync into a coalesced background task so
            // high-frequency PTY output chunks are never blocked waiting on RPC round-trips.
            let weak_hub_sync = Arc::downgrade(self);
            let mut shutdown_rx_sync = self.shutdown_tx.subscribe();
            let peer_sync = Arc::clone(&arc_sess);
            let host_sync = host.clone();
            let notify_rx = Arc::clone(&sync_notify);
            let stop_flag = Arc::clone(&sync_stop);
            self.spawn_bg(async move {
                loop {
                    tokio::select! {
                        changed = shutdown_rx_sync.changed() => {
                            if changed.is_err() || *shutdown_rx_sync.borrow() {
                                break;
                            }
                            continue;
                        }
                        _ = notify_rx.notified() => {}
                    }
                    if stop_flag.load(Ordering::Acquire) {
                        break;
                    }
                    tokio::select! {
                        changed = shutdown_rx_sync.changed() => {
                            if changed.is_err() || *shutdown_rx_sync.borrow() {
                                break;
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                    }
                    if stop_flag.load(Ordering::Acquire) {
                        break;
                    }
                    let Some(hub_sync) = weak_hub_sync.upgrade() else {
                        break;
                    };
                    hub_sync
                        .sync_with_connected_peer(Arc::clone(&peer_sync), &host_sync)
                        .await;
                }
            });
        }
        self.spawn_bg(async move {
            loop {
                let recv_res = tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            sync_stop.store(true, Ordering::Release);
                            sync_notify.notify_one();
                            break;
                        }
                        continue;
                    }
                    ev = rx.recv() => ev,
                };
                let Some(hub) = weak_hub.upgrade() else {
                    sync_stop.store(true, Ordering::Release);
                    sync_notify.notify_one();
                    break;
                };
                match recv_res {
                    Ok(PeerAsyncEvent::PtyOutputChunk {
                        terminal,
                        data_b64,
                        ..
                    }) => {
                        // V3-01: Drop any late chunks from a superseded peer (e.g. old Relay after Direct hot-switch).
                        let is_current = hub
                            .peers_sync
                            .read()
                            .ok()
                            .and_then(|m| m.get(&host_id).map(|cur| Arc::ptr_eq(cur, &peer_for_task)))
                            .unwrap_or(false);
                        if !is_current {
                            continue;
                        }
                        if let Ok(bytes) = B64.decode(&data_b64) {
                            let sid = format!("remote:{host_id}:{terminal}");
                            let replies = if let Ok(mut mgr) = hub.pty_mgr.lock() {
                                mgr.ensure_remote_buffer(&sid, 120, 36);
                                mgr.feed_remote_output(&sid, &bytes);
                                mgr.drain_remote_replies(&sid)
                            } else {
                                Vec::new()
                            };
                            for reply in replies {
                                hub.send_remote_pty_input(host_id, &terminal, reply);
                            }
                        }
                    }
                    Ok(PeerAsyncEvent::HerdrLiveEvent { terminal, .. }) => {
                        let is_current = hub
                            .peers_sync
                            .read()
                            .ok()
                            .and_then(|m| m.get(&host_id).map(|cur| Arc::ptr_eq(cur, &peer_for_task)))
                            .unwrap_or(false);
                        if !is_current {
                            continue;
                        }
                        if let Some(term) = terminal {
                            let sid = format!("remote:{}:{}", host_id, term.address);
                            if let Ok(mut g) = hub.state.lock() {
                                if let Some(sess) = g.sessions.iter_mut().find(|s| s.id == sid) {
                                    sess.alive = term.alive;
                                }
                                if let Some(osc) = herdr_status_to_osc(&term.status) {
                                    g.record_session_osc_status(&sid, osc);
                                }
                            }
                            if let Ok(mgr) = hub.pty_mgr.lock() {
                                mgr.set_remote_alive(&sid, term.alive);
                            }
                            hub.ui_dirty.store(true, Ordering::Release);
                        }
                        sync_notify.notify_one();
                    }
                    Ok(PeerAsyncEvent::Disconnected(_))
                    | Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        sync_stop.store(true, Ordering::Release);
                        sync_notify.notify_one();

                        // If this session was already atomically replaced (e.g. Relay -> LAN Direct hot-switch),
                        // exit quietly without touching the live replacement peer.
                        let is_still_current = {
                            let map = hub.peers.lock().await;
                            map.get(&host_id)
                                .map(|cur| Arc::ptr_eq(cur, &peer_for_task))
                                .unwrap_or(false)
                        };
                        if !is_still_current {
                            break;
                        }

                        // Seamless failover: if a Direct P2P session drops while host is still online,
                        // keep VT buffers intact and immediately fail over to Smart Relay (respecting 8s flap guard).
                        let host_still_online = hub
                            .state
                            .lock()
                            .ok()
                            .and_then(|g| g.remote_hosts.iter().find(|h| h.id == host_id).cloned())
                            .filter(|h| h.is_online);

                        if peer_for_task.is_direct_p2p() {
                            hub.smart_path
                                .lock()
                                .await
                                .record_direct_drop(host_id, Instant::now());
                            if let Some(online_host) = host_still_online {
                                if !hub.remove_peer_if_ptr_eq(host_id, &peer_for_task).await {
                                    break;
                                }
                                peer_for_task.close();
                                if let Ok(cfg) =
                                    load_desktop_daemon_config(&DaemonConfig::default_path())
                                {
                                    // V3-12: Force Relay-only on Direct drop failover so we never re-probe dead LAN during 8s flap-guard!
                                    if let Ok(new_peer) = hub
                                        .get_or_connect_peer_with_mode(&cfg, &online_host, false)
                                        .await
                                    {
                                        hub.reattach_terms_preserving_screen(&new_peer, host_id)
                                            .await;
                                        let route_lbl = new_peer.route_label().to_string();
                                        if let Ok(mut g) = hub.state.lock() {
                                            g.remote_probe_status = Some(format!(
                                                "{} · {route_lbl}",
                                                online_host.name
                                            ));
                                            g.status_notice = Some(format!(
                                                "{} · {} ({route_lbl})",
                                                tr(
                                                    g.locale,
                                                    "LAN Direct dropped — failed over seamlessly to Smart Relay",
                                                    "局域网直连断开，已无缝切回智能中继"
                                                ),
                                                online_host.name
                                            ));
                                        }
                                        hub.ui_dirty.store(true, Ordering::Release);
                                        break;
                                    }
                                }
                                hub.mark_host_sessions_disconnected_if_no_peer(host_id).await;
                                break;
                            }
                        } else if let Some(online_host) = host_still_online {
                            // Relay leg dropped or received `SessionReset`: remove stale Relay peer,
                            // wait briefly to avoid tight loops, and seamlessly re-handshake & re-attach.
                            if !hub.remove_peer_if_ptr_eq(host_id, &peer_for_task).await {
                                break;
                            }
                            peer_for_task.close();
                            tokio::time::sleep(Duration::from_millis(350)).await;
                            if let Ok(cfg) =
                                load_desktop_daemon_config(&DaemonConfig::default_path())
                            {
                                if let Ok(new_peer) =
                                    hub.get_or_connect_peer(&cfg, &online_host).await
                                {
                                    hub.reattach_terms_preserving_screen(&new_peer, host_id)
                                        .await;
                                    hub.ui_dirty.store(true, Ordering::Release);
                                    break;
                                }
                            }
                            hub.mark_host_sessions_disconnected_if_no_peer(host_id).await;
                            break;
                        }

                        hub.drop_peer_if_ptr_eq(host_id, &peer_for_task).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Recover from dropped broadcast chunks by fetching a clean screen snapshot.
                        let hub_resync = Arc::clone(&hub);
                        let peer_resync = Arc::clone(&peer_for_task);
                        let host_resync = host_clone.clone();
                        hub.spawn_bg(async move {
                            let terms: Vec<String> = hub_resync
                                .attached_terms
                                .lock()
                                .await
                                .iter()
                                .filter(|(hid, _)| *hid == host_resync.id)
                                .map(|(_, t)| t.clone())
                                .collect();
                            for term_addr in terms {
                                let sid = format!("remote:{}:{term_addr}", host_resync.id);
                                if let Ok(PeerMessage::TerminalScreenSnapshot { text, .. }) =
                                    peer_resync
                                        .request(
                                            PeerMessage::ReadTerminalScreen {
                                                req_id: format!("lag-{}", Uuid::new_v4().simple()),
                                                terminal: term_addr.clone(),
                                                history_lines: 120,
                                            },
                                            TrafficLane::BulkP1,
                                            Duration::from_secs(6),
                                        )
                                        .await
                                {
                                    if !text.is_empty() {
                                        let norm = normalize_newlines_for_vt(&text);
                                        if let Ok(mgr) = hub_resync.pty_mgr.lock() {
                                            mgr.clear_buffer(&sid);
                                            mgr.feed_remote_output(&sid, &norm);
                                        }
                                    }
                                }
                            }
                        });
                    }
                }
            }
        });
    }

    async fn get_or_connect_peer(
        self: &Arc<Self>,
        cfg: &DaemonConfig,
        host: &RemoteHostSummary,
    ) -> anyhow::Result<Arc<LivePeerSession>> {
        self.get_or_connect_peer_with_mode(cfg, host, true).await
    }

    async fn get_or_connect_peer_with_mode(
        self: &Arc<Self>,
        cfg: &DaemonConfig,
        host: &RemoteHostSummary,
        allow_direct: bool,
    ) -> anyhow::Result<Arc<LivePeerSession>> {
        if let Some(existing) = self.peers.lock().await.get(&host.id).cloned() {
            return Ok(existing);
        }
        let connect_lock = {
            let mut locks = self.peer_connect_locks.lock().await;
            Arc::clone(
                locks
                    .entry(host.id)
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _connect_guard = connect_lock.lock().await;
        if let Some(existing) = self.peers.lock().await.get(&host.id).cloned() {
            return Ok(existing);
        }

        let (mut client_dev_id, mut client_identity, loaded_from_disk) =
            self.resolve_client_identity(cfg).await;

        let relay_cfg = match cfg.session_token.as_deref() {
            Some(tok) => {
                let first_try = mint_peer_relay_config(
                    &cfg.server_url,
                    tok,
                    client_dev_id,
                    host.id,
                    &host.ed25519_pubkey,
                    &host.x25519_pubkey,
                )
                .await;
                if let Ok(rc) = first_try {
                    Some(rc)
                } else if loaded_from_disk {
                    if let Ok(fresh) = coda_daemon::register_client_identity(
                        &cfg.server_url,
                        tok,
                        "desktop-client",
                        "macos",
                    )
                    .await
                    {
                        client_dev_id = fresh.0;
                        client_identity = fresh.1.clone();
                        let _ = DesktopClientIdentityRecord::save_for_token(
                            &cfg.server_url,
                            tok,
                            client_dev_id,
                            &client_identity,
                        );
                        self.client_identities
                            .lock()
                            .await
                            .insert(Uuid::nil(), (client_dev_id, client_identity.clone()));
                        mint_peer_relay_config(
                            &cfg.server_url,
                            tok,
                            client_dev_id,
                            host.id,
                            &host.ed25519_pubkey,
                            &host.x25519_pubkey,
                        )
                        .await
                        .ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            None => None,
        };
        let (endpoints, udp_probe) = if allow_direct {
            (
                host.endpoints.clone(),
                Some("255.255.255.255:24444".parse().unwrap()),
            )
        } else {
            (Vec::new(), None)
        };
        let session = LivePeerSession::connect_auto(
            client_identity,
            client_dev_id,
            host.id,
            host.identity_pin(),
            endpoints,
            udp_probe,
            relay_cfg,
        )
        .await?;
        let is_direct = session.is_direct_p2p();
        let arc_sess = Arc::new(session);
        self.insert_peer(host.id, Arc::clone(&arc_sess)).await;

        self.spawn_peer_event_loop(Arc::clone(&arc_sess), host.clone());
        if is_direct {
            self.smart_path
                .lock()
                .await
                .record_upgrade_success(host.id);
        } else {
            self.spawn_lan_upgrade_watcher(Arc::clone(&arc_sess), host.clone());
        }

        Ok(arc_sess)
    }

    async fn sync_single_remote_host(
        self: &Arc<Self>,
        cfg: &DaemonConfig,
        host: &RemoteHostSummary,
    ) {
        let Ok(peer) = self.get_or_connect_peer(cfg, host).await else {
            return;
        };
        self.sync_with_connected_peer(peer, host).await;
    }

    async fn sync_with_connected_peer(
        self: &Arc<Self>,
        peer: Arc<LivePeerSession>,
        host: &RemoteHostSummary,
    ) {
        let ws_reply = peer
            .request(
                PeerMessage::ListWorkspaces {
                    req_id: format!("ws-{}", Uuid::new_v4().simple()),
                    root_dirs: Vec::new(),
                },
                TrafficLane::BulkP1,
                Duration::from_secs(10),
            )
            .await;
        let (repos, voice_cfg_opt, host_health_opt) = match ws_reply {
            Ok(PeerMessage::WorkspacesSnapshot {
                repos,
                voice_config,
                host_health,
                ..
            }) => (repos, voice_config, host_health),
            _ => (Vec::new(), None, None),
        };
        if host_health_opt.is_some() || voice_cfg_opt.is_some() {
            if let Ok(mut g) = self.state.lock() {
                if let Some(ref hh) = host_health_opt {
                    if let Some(entry) = g.remote_hosts.iter_mut().find(|h| h.id == host.id) {
                        entry.health = Some(hh.clone());
                    }
                }
                if let Some(vc) = voice_cfg_opt {
                    if !vc.base_url.trim().is_empty() && g.voice_api_key.trim().is_empty() {
                        g.voice_base_url = vc.base_url;
                        if !vc.model.trim().is_empty() {
                            g.voice_model = vc.model;
                        }
                    }
                }
            }
            self.ui_dirty.store(true, Ordering::Release);
        }

        let terms_reply = peer
            .request(
                PeerMessage::ListTerminals {
                    req_id: format!("lt-{}", Uuid::new_v4().simple()),
                    include_finished: false,
                },
                TrafficLane::ControlP0,
                Duration::from_secs(10),
            )
            .await;
        let terms = match terms_reply {
            Ok(PeerMessage::TerminalsSnapshot { terminals, .. }) => terminals,
            _ => return,
        };

        let host_id_str = host.id.to_string();
        let remote_sid_prefix = format!("remote:{}:", host.id);
        let fallback_space_id = format!("remote:{}", host.id);
        let fallback_path = terms
            .iter()
            .find(|t| !t.cwd.trim().is_empty())
            .map(|t| t.cwd.clone())
            .unwrap_or_else(|| ".".to_string());

        let mut closed_snapshot = HashSet::new();
        let mut active_remote_refresh: Option<(String, String, String)> = None;
        {
            if let Ok(mut g) = self.state.lock() {
                let mut first_repo_id: Option<String> = None;
                for repo in &repos {
                    let sp_id = format!("remote:{}:{}", host.id, repo.root_path);
                    if g.dismissed_spaces.contains(&sp_id) {
                        continue;
                    }
                    if first_repo_id.is_none() {
                        first_repo_id = Some(sp_id.clone());
                    }
                    let primary_branch = repo
                        .branches
                        .first()
                        .map(|b| b.branch.clone())
                        .filter(|b| !b.is_empty())
                        .unwrap_or_else(|| "online".to_string());
                    let dirty_count = repo
                        .branches
                        .iter()
                        .map(|b| (b.diff_added + b.diff_deleted) as usize)
                        .sum();
                    if let Some(sp) = g.spaces.iter_mut().find(|s| s.id == sp_id) {
                        sp.branch = primary_branch;
                        sp.dirty_files = dirty_count;
                    } else {
                        g.spaces.push(SpaceEntry {
                            id: sp_id,
                            avatar_color: space_avatar_color(&repo.repo_name),
                            name: repo.repo_name.clone(),
                            path: repo.root_path.clone(),
                            machine_id: host_id_str.clone(),
                            machine_label: host.name.clone(),
                            is_remote: true,
                            branch: primary_branch,
                            dirty_files: dirty_count,
                            ahead: 0,
                            behind: 0,
                        });
                    }
                }

                if let Some(ref primary_repo_id) = first_repo_id {
                    g.spaces.retain(|s| s.id != fallback_space_id);
                    if g.active_space_id.as_deref() == Some(fallback_space_id.as_str()) {
                        g.active_space_id = Some(primary_repo_id.clone());
                    }
                    for sess in &mut g.sessions {
                        if sess.space_id == fallback_space_id {
                            sess.space_id = primary_repo_id.clone();
                        }
                    }
                } else if repos.is_empty()
                    && !g.dismissed_spaces.contains(&fallback_space_id)
                    && !g.spaces.iter().any(|s| s.id == fallback_space_id)
                {
                    g.spaces.push(SpaceEntry {
                        id: fallback_space_id.clone(),
                        avatar_color: space_avatar_color(&host.name),
                        name: host.name.clone(),
                        path: fallback_path.clone(),
                        machine_id: host_id_str.clone(),
                        machine_label: host.name.clone(),
                        is_remote: true,
                        branch: "online".to_string(),
                        dirty_files: 0,
                        ahead: 0,
                        behind: 0,
                    });
                }

                let live_sids: HashSet<String> = terms
                    .iter()
                    .map(|t| format!("remote:{}:{}", host.id, t.address))
                    .collect();
                g.closed_session_ids
                    .retain(|cid| !cid.starts_with(&remote_sid_prefix) || live_sids.contains(cid));
                closed_snapshot = g.closed_session_ids.clone();

                for sess in &mut g.sessions {
                    if sess.remote_host_id.as_deref() == Some(host_id_str.as_str())
                        && !live_sids.contains(&sess.id)
                    {
                        sess.alive = false;
                    }
                }

                for term in &terms {
                    let sid = format!("remote:{}:{}", host.id, term.address);
                    if closed_snapshot.contains(&sid) {
                        continue;
                    }
                    let space_id = g
                        .spaces
                        .iter()
                        .filter(|sp| sp.machine_id == host_id_str)
                        .filter(|sp| path_within_root(&term.cwd, &sp.path))
                        .max_by_key(|sp| sp.path.len())
                        .or_else(|| g.spaces.iter().find(|sp| sp.machine_id == host_id_str))
                        .map(|sp| sp.id.clone())
                        .unwrap_or_else(|| fallback_space_id.clone());

                    let osc = herdr_status_to_osc(&term.status);
                    let label = term
                        .name
                        .as_deref()
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or(&term.address);
                    if let Some(existing) = g.sessions.iter_mut().find(|s| s.id == sid) {
                        existing.alive = term.alive;
                        existing.space_id = space_id;
                        if let Some(o) = osc {
                            g.record_session_osc_status(&sid, o);
                        }
                    } else {
                        let initial_osc = osc.clone();
                        g.sessions.push(TerminalSessionEntry {
                            id: sid.clone(),
                            title: format!("{} · {}", host.name, label),
                            space_id,
                            cwd: term.cwd.clone(),
                            started_at: Instant::now(),
                            last_output_at: Instant::now(),
                            last_byte_sig: 0,
                            lines: Vec::new(),
                            styled_lines: Vec::new(),
                            scroll_offset: 0,
                            alive: term.alive,
                            attached_hc: true,
                            hc_addr: Some(term.address.clone()),
                            remote_host_id: Some(host_id_str.clone()),
                            osc_status: osc,
                        });
                        if let Some(o) = initial_osc {
                            g.record_session_osc_status(&sid, o);
                        } else {
                            g.notify_tracker
                                .seed_baseline(&sid, crate::pty::AgentLifecycleState::Idle);
                        }
                        if g.active_session_id.is_none() {
                            g.active_session_id = Some(sid);
                        }
                    }
                }

                if let Some(sp) = g.active_space() {
                    if sp.machine_id == host_id_str && sp.path != "." && g.dir_entries.is_empty() {
                        active_remote_refresh = Some((
                            sp.id.clone(),
                            sp.path.clone(),
                            g.current_browse_rel.clone(),
                        ));
                    }
                }
            }
        }

        if let Some((sp_id, sp_path, browse_rel)) = active_remote_refresh {
            self.refresh_remote_git_diff(host.id, &sp_id, &sp_path);
            self.refresh_remote_directory(host.id, &sp_id, &sp_path, &browse_rel);
        }

        for term in terms {
            let sid = format!("remote:{}:{}", host.id, term.address);
            if closed_snapshot.contains(&sid) {
                continue;
            }
            if let Ok(mut mgr) = self.pty_mgr.lock() {
                mgr.ensure_remote_buffer(&sid, term.cols.max(80), term.rows.max(24));
                mgr.set_remote_alive(&sid, term.alive);
            }
            let need_attach = {
                let mut att = self.attached_terms.lock().await;
                att.insert((host.id, term.address.clone()))
            };
            if need_attach {
                self.seed_and_attach_remote_term(Arc::clone(&peer), host.id, &term)
                    .await;
            }
        }

        self.ui_dirty.store(true, Ordering::Release);
    }

    async fn seed_and_attach_remote_term(
        self: &Arc<Self>,
        peer: Arc<LivePeerSession>,
        host_id: Uuid,
        term: &HerdrTerminalSummary,
    ) {
        let sid = format!("remote:{}:{}", host_id, term.address);
        let (cols, rows) = self
            .pty_mgr
            .lock()
            .ok()
            .and_then(|mgr| mgr.session_dims(&sid))
            .unwrap_or((term.cols.max(80), term.rows.max(24)));
        if let Ok(PeerMessage::TerminalScreenSnapshot { text, .. }) = peer
            .request(
                PeerMessage::ReadTerminalScreen {
                    req_id: format!("scr-{}", Uuid::new_v4().simple()),
                    terminal: term.address.clone(),
                    history_lines: 120,
                },
                TrafficLane::BulkP1,
                Duration::from_secs(6),
            )
            .await
        {
            if !text.is_empty() {
                let norm = normalize_newlines_for_vt(&text);
                if let Ok(mgr) = self.pty_mgr.lock() {
                    mgr.clear_buffer(&sid);
                    mgr.feed_remote_output(&sid, &norm);
                }
            }
        }

        let attached_ok = matches!(
            peer.request(
                PeerMessage::PtyAttach {
                    req_id: format!("att-{}", Uuid::new_v4().simple()),
                    terminal: term.address.clone(),
                    cols,
                    rows,
                },
                TrafficLane::ControlP0,
                Duration::from_secs(6),
            )
            .await,
            Ok(PeerMessage::PtyAttached { ok: true, .. })
        );

        if !attached_ok {
            self.attached_terms
                .lock()
                .await
                .remove(&(host_id, term.address.clone()));
        }
    }

    async fn ensure_peer_for_host(self: &Arc<Self>, host_id: Uuid) -> Option<Arc<LivePeerSession>> {
        if let Some(p) = self.peers.lock().await.get(&host_id).cloned() {
            return Some(p);
        }
        let cfg = load_desktop_daemon_config(&DaemonConfig::default_path()).ok()?;
        let host = self
            .state
            .lock()
            .ok()
            .and_then(|g| g.remote_hosts.iter().find(|h| h.id == host_id).cloned())?;
        self.get_or_connect_peer(&cfg, &host).await.ok()
    }

    pub fn spawn_remote_terminal_or_agent(
        self: &Arc<Self>,
        host_id: Uuid,
        agent: &str,
        cwd: &str,
        space_id: &str,
    ) {
        let hub = Arc::clone(self);
        let agent = agent.to_string();
        let cwd = cwd.to_string();
        let space_id = space_id.to_string();
        if let Ok(mut g) = hub.state.lock() {
            g.status_notice = Some(format!("Starting {agent} on remote host..."));
        }
        hub.ui_dirty.store(true, Ordering::Release);
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some("Remote host is not connected".to_string());
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let host_name = hub
                .state
                .lock()
                .ok()
                .and_then(|g| {
                    g.remote_hosts
                        .iter()
                        .find(|h| h.id == host_id)
                        .map(|h| h.name.clone())
                })
                .unwrap_or_else(|| "remote".to_string());

            let cwd_opt = if cwd.trim().is_empty() || cwd == "." {
                None
            } else {
                Some(cwd.clone())
            };
            let hc_name = format!("coda-{}", &Uuid::new_v4().simple().to_string()[..8]);
            let req = PeerMessage::LaunchAgent {
                req_id: format!("la-{}", Uuid::new_v4().simple()),
                agent: agent.clone(),
                name: Some(hc_name),
                cwd: cwd_opt,
                initial_prompt: None,
                cols: 120,
                rows: 36,
            };
            match peer
                .request(req, TrafficLane::ControlP0, Duration::from_secs(12))
                .await
            {
                Ok(PeerMessage::LaunchAgentResult {
                    ok: true,
                    terminal: Some(term),
                    ..
                }) => {
                    let sid = format!("remote:{}:{}", host_id, term.address);
                    if let Ok(mut mgr) = hub.pty_mgr.lock() {
                        mgr.ensure_remote_buffer(&sid, 120, 36);
                    }
                    {
                        if let Ok(mut g) = hub.state.lock() {
                            g.closed_session_ids.remove(&sid);
                            g.status_notice = None;
                            if !g.sessions.iter().any(|s| s.id == sid) {
                                g.sessions.push(TerminalSessionEntry {
                                    id: sid.clone(),
                                    title: format!("{host_name} · {agent}"),
                                    space_id,
                                    cwd: term.cwd.clone(),
                                    started_at: Instant::now(),
                                    last_output_at: Instant::now(),
                                    last_byte_sig: 0,
                                    lines: Vec::new(),
                                    styled_lines: Vec::new(),
                                    scroll_offset: 0,
                                    alive: true,
                                    attached_hc: true,
                                    hc_addr: Some(term.address.clone()),
                                    remote_host_id: Some(host_id.to_string()),
                                    osc_status: None,
                                });
                                g.notify_tracker
                                    .seed_baseline(&sid, crate::pty::AgentLifecycleState::Idle);
                            }
                            if g.layout.split_terminals && g.focused_pane == SplitPane::Secondary {
                                g.split_session_id = Some(sid);
                            } else {
                                g.active_session_id = Some(sid);
                            }
                            g.input_target = ActiveInputTarget::TerminalDirect;
                        }
                    }
                    let need_attach = hub
                        .attached_terms
                        .lock()
                        .await
                        .insert((host_id, term.address.clone()));
                    if need_attach {
                        hub.seed_and_attach_remote_term(Arc::clone(&peer), host_id, &term)
                            .await;
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Ok(PeerMessage::LaunchAgentResult { error, .. }) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice =
                            Some(error.unwrap_or_else(|| "remote launch failed".into()));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Err(e) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("remote launch error: {e}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                _ => {}
            }
        });
    }

    pub fn send_remote_pty_input(self: &Arc<Self>, host_id: Uuid, terminal: &str, bytes: Vec<u8>) {
        let data_b64 = B64.encode(bytes);
        // V3-13: Synchronous FIFO enqueue on caller thread prevents out-of-order keystrokes across
        // Tokio worker threads; only fall back to async send if channel backpressure is hit.
        if let Ok(sync_map) = self.peers_sync.read() {
            if let Some(peer) = sync_map.get(&host_id) {
                if peer
                    .try_send_oneway(
                        PeerMessage::PtyInput {
                            terminal: terminal.to_string(),
                            data_b64: data_b64.clone(),
                        },
                        TrafficLane::ControlP0,
                    )
                    .is_ok()
                {
                    return;
                }
            }
        }
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            if let Some(peer) = hub.peers.lock().await.get(&host_id).cloned() {
                let _ = peer
                    .send_oneway(
                        PeerMessage::PtyInput { terminal, data_b64 },
                        TrafficLane::ControlP0,
                    )
                    .await;
            }
        });
    }

    pub fn resize_remote_pty(
        self: &Arc<Self>,
        host_id: Uuid,
        terminal: &str,
        cols: u16,
        rows: u16,
    ) {
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            if let Some(peer) = hub.peers.lock().await.get(&host_id).cloned() {
                let _ = peer
                    .send_oneway(
                        PeerMessage::PtyResize {
                            terminal,
                            cols,
                            rows,
                        },
                        TrafficLane::ControlP0,
                    )
                    .await;
            }
        });
    }

    pub fn send_remote_interrupt(self: &Arc<Self>, host_id: Uuid, terminal: &str) {
        if let Ok(sync_map) = self.peers_sync.read() {
            if let Some(peer) = sync_map.get(&host_id) {
                if peer
                    .try_send_oneway(
                        PeerMessage::PtyInput {
                            terminal: terminal.to_string(),
                            data_b64: B64.encode([0x03u8]),
                        },
                        TrafficLane::ControlP0,
                    )
                    .is_ok()
                {
                    return;
                }
            }
        }
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            if let Some(peer) = hub.peers.lock().await.get(&host_id).cloned() {
                let _ = peer
                    .send_oneway(
                        PeerMessage::PtyInput {
                            terminal,
                            data_b64: B64.encode([0x03u8]),
                        },
                        TrafficLane::ControlP0,
                    )
                    .await;
            }
        });
    }

    pub fn detach_remote_pty(self: &Arc<Self>, host_id: Uuid, terminal: &str) {
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            hub.attached_terms
                .lock()
                .await
                .remove(&(host_id, terminal.clone()));
            if let Some(peer) = hub.peers.lock().await.get(&host_id).cloned() {
                let _ = peer
                    .request(
                        PeerMessage::PtyDetach {
                            req_id: format!("det-{}", Uuid::new_v4().simple()),
                            terminal,
                        },
                        TrafficLane::ControlP0,
                        Duration::from_secs(4),
                    )
                    .await;
            }
        });
    }

    pub fn kill_remote_terminal(self: &Arc<Self>, host_id: Uuid, terminal: &str) {
        let hub = Arc::clone(self);
        let terminal = terminal.to_string();
        self.spawn_bg(async move {
            hub.attached_terms
                .lock()
                .await
                .remove(&(host_id, terminal.clone()));
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                return;
            };
            match peer
                .request(
                    PeerMessage::KillTerminal {
                        req_id: format!("kt-{}", Uuid::new_v4().simple()),
                        terminal: terminal.clone(),
                    },
                    TrafficLane::ControlP0,
                    Duration::from_secs(6),
                )
                .await
            {
                Ok(PeerMessage::ActionAck {
                    ok: false, detail, ..
                }) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote kill failed: {detail}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Err(e) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote kill error ({terminal}): {e}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                _ => {}
            }
        });
    }

    pub fn refresh_remote_directory(
        self: &Arc<Self>,
        host_id: Uuid,
        space_id: &str,
        root_path: &str,
        sub_path: &str,
    ) {
        let hub = Arc::clone(self);
        let space_id = space_id.to_string();
        let root_path = root_path.to_string();
        let sub_path = sub_path.to_string();
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                return;
            };
            let req = PeerMessage::ListDirectory {
                req_id: format!("ls-{}", Uuid::new_v4().simple()),
                root_path: root_path.clone(),
                sub_path: sub_path.clone(),
            };
            if let Ok(PeerMessage::DirectoryListing { entries, error, .. }) = peer
                .request(req, TrafficLane::BulkP1, Duration::from_secs(8))
                .await
            {
                if let Some(err) = error {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote ls error: {err}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                    return;
                }
                let root_trim = root_path.trim_end_matches('/');
                let mapped: Vec<WorkspaceFileEntry> = entries
                    .into_iter()
                    .map(|item: RemoteFileItem| WorkspaceFileEntry {
                        name: item.name,
                        full_path: format!("{root_trim}/{}", item.rel_path),
                        rel_path: item.rel_path,
                        is_dir: item.is_dir,
                        size_bytes: item.size_bytes,
                    })
                    .collect();
                if let Ok(mut g) = hub.state.lock() {
                    if g.active_space_id.as_deref() == Some(space_id.as_str()) {
                        g.dir_entries = mapped;
                    }
                }
                hub.ui_dirty.store(true, Ordering::Release);
            }
        });
    }

    pub fn open_remote_file_preview(
        self: &Arc<Self>,
        host_id: Uuid,
        full_path: &str,
        rel_path: &str,
    ) {
        let hub = Arc::clone(self);
        let full_path = full_path.to_string();
        let rel_path = rel_path.to_string();
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some("Remote host is not connected".to_string());
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let req = PeerMessage::ReadFilePreview {
                req_id: format!("rf-{}", Uuid::new_v4().simple()),
                path: full_path,
                max_bytes: 120_000,
            };
            match peer
                .request(req, TrafficLane::BulkP1, Duration::from_secs(8))
                .await
            {
                Ok(PeerMessage::FilePreviewResponse {
                    content,
                    error,
                    truncated,
                    ..
                }) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.selected_file_rel = Some(rel_path);
                        g.selected_file_content = match error {
                            Some(err) => format!("[Error reading remote file: {err}]"),
                            None if truncated => {
                                format!("{content}\n... (truncated on remote host)")
                            }
                            None => content,
                        };
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                Err(e) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("Remote read error: {e}"));
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
                _ => {}
            }
        });
    }

    /// Sends an E2EE `PeerMessage::GitDiffRequest` to `device_id` and returns the full `RemoteGitDiffPayload`.
    pub async fn fetch_remote_git_diff(
        self: &Arc<Self>,
        device_id: Uuid,
        cwd: &str,
    ) -> Result<RemoteGitDiffPayload, String> {
        let peer = self
            .ensure_peer_for_host(device_id)
            .await
            .ok_or_else(|| "Remote host is not connected".to_string())?;
        let req = PeerMessage::GitDiffRequest {
            req_id: format!("gd-{}", Uuid::new_v4().simple()),
            cwd: cwd.to_string(),
        };
        match peer
            .request(req, TrafficLane::BulkP1, Duration::from_secs(10))
            .await
        {
            Ok(PeerMessage::GitDiffResponse {
                branch,
                status_short,
                diff_unified,
                is_git_repo,
                repo_root,
                error,
                ..
            }) => Ok(RemoteGitDiffPayload {
                branch,
                status_short,
                diff_unified,
                is_git_repo,
                repo_root,
                error,
            }),
            Ok(_) => Err("Unexpected E2EE GitDiffResponse from remote host".to_string()),
            Err(e) => Err(format!("{e}")),
        }
    }

    fn apply_remote_git_diff_response(
        &self,
        seq: u64,
        device_id: Uuid,
        space_id_hint: Option<&str>,
        cwd: &str,
        payload: RemoteGitDiffPayload,
    ) {
        if let Ok(mut g) = self.state.lock() {
            apply_remote_git_diff_payload_to_state(
                &mut g,
                seq,
                device_id,
                space_id_hint,
                cwd,
                payload,
            );
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    /// Requests an E2EE `GitDiffRequest` for `(device_id, cwd)` guarded by monotonic `remote_diff_seq`
    /// and `(device_id, cwd)` verification (`Bug #7` & `Gap #3`).
    pub fn request_remote_git_diff(self: &Arc<Self>, device_id: Uuid, cwd: String) {
        let seq = if let Ok(mut g) = self.state.lock() {
            g.remote_diff_seq = g.remote_diff_seq.wrapping_add(1);
            g.git_diff_loading = true;
            g.remote_diff_seq
        } else {
            return;
        };
        self.ui_dirty.store(true, Ordering::Release);
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            match hub.fetch_remote_git_diff(device_id, &cwd).await {
                Ok(payload) => {
                    hub.apply_remote_git_diff_response(seq, device_id, None, &cwd, payload);
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        if g.remote_diff_seq == seq {
                            g.git_diff_loading = false;
                            g.status_notice = Some(format!("Remote Git diff failed: {err}"));
                        }
                    }
                    hub.ui_dirty.store(true, Ordering::Release);
                }
            }
        });
    }

    pub fn refresh_remote_git_diff(
        self: &Arc<Self>,
        host_id: Uuid,
        space_id: &str,
        cwd: &str,
    ) {
        let hub = Arc::clone(self);
        let space_id = space_id.to_string();
        let cwd = cwd.to_string();
        let seq = if let Ok(mut g) = hub.state.lock() {
            if g.effective_remote_diff_target()
                .map(|(did, ref cur_cwd)| did == host_id && cur_cwd == &cwd)
                .unwrap_or(false)
            {
                g.remote_diff_seq = g.remote_diff_seq.wrapping_add(1);
                g.git_diff_loading = true;
                g.remote_diff_seq
            } else {
                0
            }
        } else {
            return;
        };
        self.spawn_bg(async move {
            match hub.fetch_remote_git_diff(host_id, &cwd).await {
                Ok(payload) => {
                    hub.apply_remote_git_diff_response(
                        seq,
                        host_id,
                        Some(&space_id),
                        &cwd,
                        payload,
                    );
                }
                Err(_) => {
                    if seq != 0 {
                        if let Ok(mut g) = hub.state.lock() {
                            if g.remote_diff_seq == seq {
                                g.git_diff_loading = false;
                            }
                        }
                        hub.ui_dirty.store(true, Ordering::Release);
                    }
                }
            }
        });
    }

    pub fn probe_host(self: &Arc<Self>, host: RemoteHostSummary) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                return;
            };
            let exact_prefix = format!("remote:{}", host.id);
            let repo_prefix = format!("remote:{}:", host.id);
            if let Ok(mut g) = hub.state.lock() {
                g.dismissed_spaces
                    .retain(|id| id != &exact_prefix && !id.starts_with(&repo_prefix));
            }
            let mut prefs = DesktopPreferences::load_or_default();
            let before_len = prefs.dismissed_spaces.len();
            prefs
                .dismissed_spaces
                .retain(|id| id != &exact_prefix && !id.starts_with(&repo_prefix));
            if prefs.dismissed_spaces.len() != before_len {
                let _ = prefs.save();
            }

            // If currently on Relay, also attempt an immediate Direct LAN hot-upgrade.
            hub.smart_path.lock().await.reset_host(host.id);
            let _ = hub.try_hot_upgrade_peer_to_direct(&host).await;

            match hub.get_or_connect_peer(&cfg, &host).await {
                Ok(peer) => {
                    let route = peer.route_label().to_string();
                    hub.sync_single_remote_host(&cfg, &host).await;
                    let host_id_str = host.id.to_string();
                    let (term_count, target_space) = hub
                        .state
                        .lock()
                        .map(|mut g| {
                            let found_sp = g
                                .spaces
                                .iter()
                                .find(|s| {
                                    s.machine_id == host_id_str
                                        && s.id.starts_with(&repo_prefix)
                                        && s.path != "."
                                })
                                .or_else(|| g.spaces.iter().find(|s| s.machine_id == host_id_str))
                                .map(|s| (s.id.clone(), s.path.clone()));
                            if let Some((ref sp_id, _)) = found_sp {
                                g.active_space_id = Some(sp_id.clone());
                                g.current_browse_rel.clear();
                                g.selected_file_rel = None;
                                g.selected_file_content.clear();
                                g.selected_change_path = None;
                                g.active_session_id = g
                                    .sessions
                                    .iter()
                                    .find(|s| &s.space_id == sp_id)
                                    .map(|s| s.id.clone());
                                if g.layout.split_terminals {
                                    g.split_session_id = g
                                        .sessions
                                        .iter()
                                        .find(|s| {
                                            &s.space_id == sp_id
                                                && Some(&s.id) != g.active_session_id.as_ref()
                                        })
                                        .map(|s| s.id.clone());
                                }
                                g.input_target = ActiveInputTarget::TerminalDirect;
                            }
                            let cnt = g
                                .sessions
                                .iter()
                                .filter(|s| s.remote_host_id.as_deref() == Some(host_id_str.as_str()))
                                .count();
                            (cnt, found_sp)
                        })
                        .unwrap_or((0, None));
                    if let Some((sp_id, sp_path)) = target_space {
                        let sp_id_pref = sp_id.clone();
                        CodaGpuWorkspaceView::update_prefs_async(move |p| {
                            p.selected_repo_path = Some(sp_id_pref);
                        });
                        hub.refresh_remote_git_diff(host.id, &sp_id, &sp_path);
                        if sp_path != "." {
                            hub.refresh_remote_directory(host.id, &sp_id, &sp_path, "");
                        }
                    }
                    if let Ok(mut g) = hub.state.lock() {
                        g.remote_probe_status =
                            Some(format!("{} · {route} · {term_count} sessions", host.name));
                        g.status_notice = g.remote_probe_status.clone();
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("{} unreachable: {err}", host.name));
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn upgrade_remote_daemon(self: &Arc<Self>, host: RemoteHostSummary) {
        let hub = Arc::clone(self);
        if let Ok(mut g) = hub.state.lock() {
            g.status_notice = Some(format!(
                "Upgrading coda-daemon on {} (zero-downtime OTA)…",
                host.name
            ));
        }
        hub.ui_dirty.store(true, Ordering::Release);

        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some("Sign in required before remote OTA upgrade.".into());
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            match hub.get_or_connect_peer(&cfg, &host).await {
                Ok(peer) => {
                    let reply = peer
                        .request(
                            PeerMessage::UpgradeDaemon {
                                req_id: format!("ota-{}", Uuid::new_v4().simple()),
                                target_version: None,
                            },
                            TrafficLane::ControlP0,
                            Duration::from_secs(75),
                        )
                        .await;
                    match reply {
                        Ok(PeerMessage::UpgradeDaemonResult {
                            ok,
                            previous_version,
                            target_version,
                            message,
                            ..
                        }) => {
                            let target_label = target_version.as_deref().unwrap_or("latest");
                            if let Ok(mut g) = hub.state.lock() {
                                g.status_notice = Some(if ok {
                                    format!(
                                        "{}: v{} -> v{} ({message})",
                                        host.name, previous_version, target_label
                                    )
                                } else {
                                    format!("{} OTA failed: {message}", host.name)
                                });
                            }
                            if ok {
                                // Drop cached peer so the re-exec'd daemon negotiates a fresh E2EE session cleanly.
                                hub.drop_peer(host.id).await;
                                tokio::time::sleep(Duration::from_millis(1200)).await;
                                hub.sync_single_remote_host(&cfg, &host).await;
                            }
                        }
                        Ok(_) => {
                            if let Ok(mut g) = hub.state.lock() {
                                g.status_notice =
                                    Some(format!("{} returned unexpected OTA response", host.name));
                            }
                        }
                        Err(err) => {
                            if let Ok(mut g) = hub.state.lock() {
                                g.status_notice =
                                    Some(format!("{} OTA error: {err}", host.name));
                            }
                        }
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!("{} unreachable: {err}", host.name));
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    /// Sends a full-method E2EE `HttpLocalPreviewRequest` (`method`, `headers`, `req_body_b64`) to
    /// `device_id` and returns `ProxyHttpResponse` including upstream response headers (`Bug #6` / `Gap #1`).
    pub async fn request_remote_port_preview_full(
        self: &Arc<Self>,
        device_id: Uuid,
        remote_port: u16,
        req: ProxyHttpRequest,
    ) -> Result<ProxyHttpResponse, String> {
        let peer = if let Some(existing) = self.peers.lock().await.get(&device_id).cloned() {
            existing
        } else {
            let cfg = load_desktop_daemon_config(&DaemonConfig::default_path())
                .map_err(|_| "Sign in required before previewing remote ports.".to_string())?;
            let host = self
                .state
                .lock()
                .ok()
                .and_then(|g| g.remote_hosts.iter().find(|h| h.id == device_id).cloned())
                .ok_or_else(|| "Remote host not found.".to_string())?;
            self.get_or_connect_peer(&cfg, &host)
                .await
                .map_err(|err| format!("Host unreachable: {err}"))?
        };

        let reply = peer
            .request(
                PeerMessage::HttpLocalPreviewRequest {
                    req_id: format!("pp-{}", Uuid::new_v4().simple()),
                    port: remote_port,
                    path: req.path_and_query,
                    method: Some(req.method),
                    headers: req.headers,
                    req_body_b64: if req.body.is_empty() {
                        None
                    } else {
                        Some(B64.encode(&req.body))
                    },
                },
                TrafficLane::BulkP1,
                Duration::from_secs(12),
            )
            .await
            .map_err(|err| format!("Preview error: {err}"))?;
        match reply {
            PeerMessage::HttpLocalPreviewResponse {
                status_code,
                content_type,
                body_b64,
                headers,
                truncated,
                ..
            } => {
                if truncated && status_code < 400 {
                    return Ok(ProxyHttpResponse {
                        status_code: 413,
                        content_type: "text/plain; charset=utf-8".to_string(),
                        headers: Vec::new(),
                        body: b"Payload Too Large (exceeds remote 4 MiB E2EE preview limit)"
                            .to_vec(),
                    });
                }
                let raw_bytes = crate::port_preview::decode_remote_preview_raw_bytes(&body_b64)?;
                Ok(ProxyHttpResponse {
                    status_code,
                    content_type,
                    headers,
                    body: raw_bytes,
                })
            }
            _ => Err("Unexpected E2EE response from host.".to_string()),
        }
    }

    /// Sends an E2EE `GET` `HttpLocalPreviewRequest` to `device_id` and decodes `body_b64` into raw `Vec<u8>`.
    pub async fn request_remote_port_preview(
        self: &Arc<Self>,
        device_id: Uuid,
        remote_port: u16,
        path_and_query: String,
    ) -> Result<(u16, String, Vec<u8>), String> {
        let resp = self
            .request_remote_port_preview_full(
                device_id,
                remote_port,
                ProxyHttpRequest {
                    method: "GET".to_string(),
                    path_and_query,
                    headers: Vec::new(),
                    body: Vec::new(),
                },
            )
            .await?;
        Ok((resp.status_code, resp.content_type, resp.body))
    }

    pub fn spawn_remote_port_preview_snapshot(
        self: &Arc<Self>,
        device_id: Uuid,
        port: u16,
        path: String,
    ) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            match hub
                .request_remote_port_preview(device_id, port, path)
                .await
            {
                Ok((sc, ct, raw_bytes)) => {
                    let body = crate::port_preview::format_preview_body_bytes(&raw_bytes, &ct);
                    if let Ok(mut g) = hub.state.lock() {
                        g.port_preview.loading = false;
                        g.port_preview.status_code = Some(sc);
                        g.port_preview.content_type = ct;
                        g.port_preview.body_preview = body;
                    }
                }
                Err(err) => {
                    let code = if err.contains("Sign in required") {
                        401
                    } else if err.contains("not found") {
                        404
                    } else {
                        502
                    };
                    if let Ok(mut g) = hub.state.lock() {
                        g.port_preview.loading = false;
                        g.port_preview.status_code = Some(code);
                        g.port_preview.content_type = "text/plain".to_string();
                        g.port_preview.body_preview = err;
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    /// Spawns or reuses a strictly loopback (`127.0.0.1:0`) full-method HTTP/1.1 proxy tunnel for `(device_id, remote_port)`.
    pub fn open_live_loopback_tunnel(
        self: &Arc<Self>,
        device_id: Option<Uuid>,
        remote_port: u16,
        initial_path: String,
        open_in_browser: bool,
    ) {
        let key = TunnelTargetKey {
            device_id,
            remote_port,
        };
        if let Some(existing_port) = self.tunnel_mgr.reusable_local_port(key) {
            let base_url = format!("http://127.0.0.1:{existing_port}/");
            let full_url = format!("http://127.0.0.1:{existing_port}{initial_path}");
            if open_in_browser {
                let _ = crate::port_preview::open_loopback_tunnel_in_browser(&full_url);
            }
            if let Ok(mut g) = self.state.lock() {
                g.port_preview.tunnel_starting = false;
                let hosts = g.remote_hosts.clone();
                g.port_preview
                    .sync_from_manager(&self.tunnel_mgr, &hosts, Some(key));
                g.status_notice = Some(format!("Live tunnel active: {base_url}"));
            }
            self.ui_dirty.store(true, Ordering::Release);
            return;
        }

        if let Ok(mut g) = self.state.lock() {
            g.port_preview.tunnel_starting = true;
        }
        self.ui_dirty.store(true, Ordering::Release);

        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let hub_for_fetch = Arc::clone(&hub);
            let spawn_res = crate::port_preview::spawn_loopback_http_proxy_with_full_fetcher(
                remote_port,
                move |port, proxy_req| {
                    let hub_req = Arc::clone(&hub_for_fetch);
                    async move {
                        if let Some(did) = device_id {
                            hub_req
                                .request_remote_port_preview_full(did, port, proxy_req)
                                .await
                        } else {
                            crate::port_preview::fetch_local_port_preview_full(
                                port,
                                proxy_req,
                                crate::port_preview::max_local_tunnel_body_bytes(),
                            )
                            .await
                        }
                    }
                },
            )
            .await;

            match spawn_res {
                Ok((local_port, stop_tx)) => {
                    hub.tunnel_mgr.register_tunnel(key, local_port, stop_tx);
                    let base_url = format!("http://127.0.0.1:{local_port}/");
                    let full_url = format!("http://127.0.0.1:{local_port}{initial_path}");
                    if open_in_browser {
                        let _ = crate::port_preview::open_loopback_tunnel_in_browser(&full_url);
                    }
                    if let Ok(mut g) = hub.state.lock() {
                        g.port_preview.tunnel_starting = false;
                        let hosts = g.remote_hosts.clone();
                        g.port_preview
                            .sync_from_manager(&hub.tunnel_mgr, &hosts, Some(key));
                        g.status_notice = Some(format!(
                            "{}: {base_url}",
                            tr(g.locale, "Live tunnel started", "已启动本地回环实时隧道")
                        ));
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.port_preview.tunnel_starting = false;
                        g.status_notice = Some(err);
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn stop_live_loopback_tunnel(&self, key_opt: Option<TunnelTargetKey>) {
        if let Some(key) = key_opt {
            self.tunnel_mgr.stop_tunnel(key);
        } else {
            self.tunnel_mgr.stop_all();
        }
        if let Ok(mut g) = self.state.lock() {
            g.port_preview.tunnel_starting = false;
            let hosts = g.remote_hosts.clone();
            g.port_preview
                .sync_from_manager(&self.tunnel_mgr, &hosts, None);
            g.status_notice = Some(
                tr(
                    g.locale,
                    "Stopped loopback port tunnel",
                    "已关闭本地回环端口隧道",
                )
                .to_string(),
            );
        }
        self.ui_dirty.store(true, Ordering::Release);
    }

    /// Uploads a file (`UploadTerminalFile`) over E2EE to a remote terminal workspace and returns `(remote_path, bytes)`.
    pub async fn request_upload_terminal_file(
        self: &Arc<Self>,
        device_id: Uuid,
        terminal: String,
        filename: String,
        raw_bytes: Vec<u8>,
    ) -> Result<(String, usize), String> {
        let peer = self
            .ensure_peer_for_host(device_id)
            .await
            .ok_or_else(|| "Remote host is not connected".to_string())?;
        let req = PeerMessage::UploadTerminalFile {
            req_id: format!("up-{}", Uuid::new_v4().simple()),
            terminal,
            filename,
            data_b64: B64.encode(&raw_bytes),
        };
        match peer
            .request(req, TrafficLane::BulkP1, Duration::from_secs(20))
            .await
        {
            Ok(PeerMessage::UploadTerminalFileResult {
                ok: true,
                remote_path: Some(rpath),
                bytes,
                ..
            }) if !rpath.trim().is_empty() => Ok((rpath, bytes)),
            Ok(PeerMessage::UploadTerminalFileResult { error, .. }) => {
                Err(error.unwrap_or_else(|| "Remote file upload rejected".to_string()))
            }
            Ok(_) => Err("Unexpected E2EE UploadTerminalFile response".to_string()),
            Err(e) => Err(format!("E2EE upload failed: {e}")),
        }
    }

    /// Uploads a clipboard screenshot PNG over E2EE to `terminal` and pastes the shell-escaped path + trailing space.
    pub fn upload_clipboard_png_and_paste(
        self: &Arc<Self>,
        device_id: Uuid,
        session_id: String,
        terminal: String,
        filename: String,
        png_bytes: Vec<u8>,
    ) {
        if let Ok(mut g) = self.state.lock() {
            g.status_notice = Some(
                tr(
                    g.locale,
                    "Uploading clipboard screenshot over E2EE…",
                    "正在通过 E2EE 加密上传剪贴板截图…",
                )
                .to_string(),
            );
        }
        self.ui_dirty.store(true, Ordering::Release);

        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            match hub
                .request_upload_terminal_file(device_id, terminal.clone(), filename, png_bytes)
                .await
            {
                Ok((remote_path, bytes)) => {
                    let pasted = format_pasted_terminal_path(&remote_path);
                    hub.send_remote_pty_input(device_id, &terminal, pasted.into_bytes());
                    if let Ok(mut g) = hub.state.lock() {
                        if let Some(s) = g.sessions.iter_mut().find(|s| s.id == session_id) {
                            s.scroll_offset = 0;
                        }
                        g.status_notice = Some(format!(
                            "{}: {remote_path} ({bytes} B)",
                            tr(g.locale, "Uploaded screenshot", "截图已加密上传并填入")
                        ));
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(err);
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn fetch_notification_webhooks(self: &Arc<Self>) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                return;
            };
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            if let Ok(mut g) = hub.state.lock() {
                g.webhook_loading = true;
            }
            hub.ui_dirty.store(true, Ordering::Release);

            match api_get_notification_webhooks(&cfg.server_url, tok).await {
                Ok(settings) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.webhook_loading = false;
                        g.webhook_loaded_ok = true;
                        // Never overwrite in-progress user edits made while GET was in flight (`Bug #11b`).
                        if !g.webhook_dirty {
                            g.webhook_settings = settings;
                        }
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.webhook_loading = false;
                        if g.settings_open {
                            g.status_notice = Some(err);
                        }
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn save_notification_webhooks(self: &Arc<Self>, settings: NotificationWebhookSettings) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some(
                        tr(
                            g.locale,
                            "Sign in required to save notification webhooks",
                            "请先登录账号再保存离线推送 Webhook",
                        )
                        .to_string(),
                    );
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            if let Ok(mut g) = hub.state.lock() {
                g.webhook_loading = true;
            }
            hub.ui_dirty.store(true, Ordering::Release);

            match api_put_notification_webhooks(&cfg.server_url, tok, &settings).await {
                Ok(updated) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.webhook_loading = false;
                        g.webhook_loaded_ok = true;
                        g.webhook_dirty = false;
                        g.webhook_settings = updated;
                        g.status_notice = Some(
                            tr(
                                g.locale,
                                "Saved offline notification webhooks",
                                "已保存离线通知 Webhook 配置",
                            )
                            .to_string(),
                        );
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.webhook_loading = false;
                        g.status_notice = Some(err);
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn test_notification_webhooks(self: &Arc<Self>) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some(
                        tr(
                            g.locale,
                            "Sign in required to test notification webhooks",
                            "请先登录账号再发送测试推送",
                        )
                        .to_string(),
                    );
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            if let Ok(mut g) = hub.state.lock() {
                g.webhook_loading = true;
            }
            hub.ui_dirty.store(true, Ordering::Release);

            match api_test_notification_webhooks(&cfg.server_url, tok).await {
                Ok((delivered, errors)) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.webhook_loading = false;
                        g.status_notice = Some(if errors.is_empty() {
                            format!(
                                "{}: {delivered}",
                                tr(
                                    g.locale,
                                    "Test alert delivered to channels",
                                    "测试推送已送达渠道数"
                                )
                            )
                        } else {
                            format!("Delivered {delivered}, errors: {}", errors.join("; "))
                        });
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.webhook_loading = false;
                        g.status_notice = Some(err);
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    pub fn prune_offline_web_devices(self: &Arc<Self>) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Ok(cfg) = load_desktop_daemon_config(&DaemonConfig::default_path()) else {
                if let Ok(mut g) = hub.state.lock() {
                    g.status_notice = Some(
                        tr(
                            g.locale,
                            "Sign in required to prune offline web devices",
                            "请先登录账号再清理离线 Web 设备",
                        )
                        .to_string(),
                    );
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };
            let Some(ref tok) = cfg.session_token else {
                return;
            };
            let exclude_id = hub
                .client_identities
                .lock()
                .await
                .get(&Uuid::nil())
                .map(|(id, _)| *id)
                .or_else(|| {
                    DesktopClientIdentityRecord::load_matching(&cfg.server_url, tok)
                        .map(|(id, _)| id)
                })
                .or(Some(cfg.device_id));
            match api_prune_stale_web_devices(&cfg.server_url, tok, 0, exclude_id).await {
                Ok(pruned) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(format!(
                            "{}: {pruned}",
                            tr(
                                g.locale,
                                "Pruned offline web devices",
                                "已清理离线 Web 设备数量"
                            )
                        ));
                    }
                    hub.sync_all_hosts_and_sessions().await;
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.status_notice = Some(err);
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }

    /// Creates a new folder or clones a Git repository on `host_id` over E2EE `BootstrapWorkspace` (`Gap #2`),
    /// then refreshes remote workspaces, selects the new space, and spawns a shell session in it.
    pub fn bootstrap_remote_workspace(
        self: &Arc<Self>,
        host_id: Uuid,
        parent_dir: String,
        folder_name: String,
        git_clone_url: Option<String>,
    ) {
        let hub = Arc::clone(self);
        self.spawn_bg(async move {
            let Some(peer) = hub.ensure_peer_for_host(host_id).await else {
                if let Ok(mut g) = hub.state.lock() {
                    g.bootstrap_modal.loading = false;
                    g.bootstrap_modal.error = Some(
                        tr(
                            g.locale,
                            "Remote host is offline or unreachable",
                            "远端主机当前离线或无法连接",
                        )
                        .to_string(),
                    );
                }
                hub.ui_dirty.store(true, Ordering::Release);
                return;
            };

            let req = PeerMessage::BootstrapWorkspace {
                req_id: format!("bw-{}", Uuid::new_v4().simple()),
                parent_dir: parent_dir.clone(),
                folder_name: folder_name.clone(),
                git_clone_url: git_clone_url.clone(),
            };
            match peer
                .request(req, TrafficLane::ControlP0, Duration::from_secs(65))
                .await
            {
                Ok(PeerMessage::BootstrapWorkspaceResult {
                    ok: true,
                    workspace_path,
                    detail,
                    ..
                }) => {
                    let clean_ws_path = workspace_path.trim().to_string();
                    let new_space_id = format!("remote:{host_id}:{clean_ws_path}");
                    let fallback_space_id = format!("remote:{host_id}");
                    let repo_name = Path::new(&clean_ws_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&folder_name)
                        .to_string();

                    // Refresh host workspaces including the newly created parent/workspace directory.
                    let _ = peer
                        .request(
                            PeerMessage::ListWorkspaces {
                                req_id: format!("ws-{}", Uuid::new_v4().simple()),
                                root_dirs: vec![parent_dir, clean_ws_path.clone()],
                            },
                            TrafficLane::BulkP1,
                            Duration::from_secs(10),
                        )
                        .await;

                    if let Some(host_summary) = hub
                        .state
                        .lock()
                        .ok()
                        .and_then(|g| g.remote_hosts.iter().find(|h| h.id == host_id).cloned())
                    {
                        hub.sync_with_connected_peer(Arc::clone(&peer), &host_summary)
                            .await;
                    }

                    let mut need_spawn_shell = false;
                    if let Ok(mut g) = hub.state.lock() {
                        let host_name = g
                            .remote_hosts
                            .iter()
                            .find(|h| h.id == host_id)
                            .map(|h| h.name.clone())
                            .unwrap_or_else(|| "remote".to_string());
                        g.dismissed_spaces.remove(&new_space_id);
                        g.spaces.retain(|s| s.id != fallback_space_id);
                        if !g.spaces.iter().any(|s| s.id == new_space_id) {
                            g.spaces.push(SpaceEntry {
                                id: new_space_id.clone(),
                                avatar_color: space_avatar_color(&repo_name),
                                name: repo_name.clone(),
                                path: clean_ws_path.clone(),
                                machine_id: host_id.to_string(),
                                machine_label: host_name,
                                is_remote: true,
                                branch: if git_clone_url.is_some() {
                                    "main".to_string()
                                } else {
                                    "folder".to_string()
                                },
                                dirty_files: 0,
                                ahead: 0,
                                behind: 0,
                            });
                        }
                        g.active_space_id = Some(new_space_id.clone());
                        g.current_browse_rel.clear();
                        g.selected_file_rel = None;
                        g.selected_file_content.clear();
                        g.selected_change_path = None;
                        let existing_sess = g
                            .sessions
                            .iter()
                            .find(|s| s.space_id == new_space_id)
                            .map(|s| s.id.clone());
                        g.active_session_id = existing_sess.clone();
                        need_spawn_shell = existing_sess.is_none();
                        g.bootstrap_modal.reset();
                        g.input_target = ActiveInputTarget::TerminalDirect;
                        g.status_notice = Some(format!(
                            "{}: {repo_name} ({detail})",
                            tr(
                                g.locale,
                                "Remote workspace ready",
                                "远端工作区已就绪"
                            )
                        ));
                    }

                    let pref_space = new_space_id.clone();
                    let mut prefs = DesktopPreferences::load_or_default();
                    prefs.dismissed_spaces.retain(|x| x != &pref_space);
                    prefs.selected_repo_path = Some(pref_space);
                    let _ = prefs.save();

                    hub.refresh_remote_git_diff(host_id, &new_space_id, &clean_ws_path);
                    hub.refresh_remote_directory(host_id, &new_space_id, &clean_ws_path, "");
                    if need_spawn_shell {
                        hub.spawn_remote_terminal_or_agent(
                            host_id,
                            "shell",
                            &clean_ws_path,
                            &new_space_id,
                        );
                    }
                }
                Ok(PeerMessage::BootstrapWorkspaceResult { detail, .. }) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.bootstrap_modal.loading = false;
                        g.bootstrap_modal.error = Some(detail);
                    }
                }
                Ok(_) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.bootstrap_modal.loading = false;
                        g.bootstrap_modal.error =
                            Some("Unexpected BootstrapWorkspace response".to_string());
                    }
                }
                Err(err) => {
                    if let Ok(mut g) = hub.state.lock() {
                        g.bootstrap_modal.loading = false;
                        g.bootstrap_modal.error = Some(format!("Bootstrap error: {err}"));
                    }
                }
            }
            hub.ui_dirty.store(true, Ordering::Release);
        });
    }
}

#[derive(Debug, Clone)]
pub struct RemoteGitDiffPayload {
    pub branch: String,
    pub status_short: String,
    pub diff_unified: String,
    pub is_git_repo: bool,
    pub repo_root: Option<String>,
    pub error: Option<String>,
}

/// Applies a remote `GitDiffResponse` payload into `CodaDesktopState`, enforcing the monotonic
/// `(seq, device_id, cwd)` guard so stale responses or split-pane local focus never clobber `repo_snapshot` (`Bug #7`).
pub(crate) fn apply_remote_git_diff_payload_to_state(
    g: &mut CodaDesktopState,
    seq: u64,
    device_id: Uuid,
    space_id_hint: Option<&str>,
    cwd: &str,
    payload: RemoteGitDiffPayload,
) -> bool {
    let changed_files = parse_git_status_short(&payload.status_short);
    let legacy_empty_non_git = payload.repo_root.is_none()
        && payload.branch.trim().is_empty()
        && payload.status_short.trim().is_empty()
        && payload.diff_unified.trim().is_empty();
    let eff_is_git_repo = payload.is_git_repo && !legacy_empty_non_git;
    let clean_branch = if payload.branch.trim().is_empty() {
        if eff_is_git_repo {
            "HEAD".to_string()
        } else {
            "online".to_string()
        }
    } else {
        payload.branch.trim().to_string()
    };
    let host_id_str = device_id.to_string();

    // Update sidebar space badge ONLY for the exact `(device_id, cwd)` space.
    for sp in &mut g.spaces {
        if sp.machine_id == host_id_str
            && (space_id_hint == Some(sp.id.as_str()) || sp.path == cwd)
        {
            if eff_is_git_repo {
                sp.branch = clean_branch.clone();
            }
            sp.dirty_files = changed_files.len();
        }
    }

    // Strict `(seq, device_id, cwd)` guard before touching `git_diff_loading` or `repo_snapshot`!
    if seq == 0 || g.remote_diff_seq != seq {
        return false;
    }
    let Some((cur_dev, cur_cwd)) = g.effective_remote_diff_target() else {
        return false;
    };
    if cur_dev != device_id || cur_cwd != cwd {
        return false;
    }

    g.git_diff_loading = false;
    if let Some(ref sel) = g.selected_change_path {
        if !changed_files.iter().any(|f| &f.path == sel) {
            g.selected_change_path = None;
        }
    }

    let root_for_name = payload
        .repo_root
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(cwd);
    let repo_name = Path::new(root_for_name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != ".")
        .unwrap_or("remote")
        .to_string();
    let clean_err = payload
        .error
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty());

    let (diff_title, diff_text) = if let Some(ref err_msg) = clean_err {
        ("Remote Git diff error".to_string(), err_msg.clone())
    } else if !eff_is_git_repo {
        (
            "Not a Git repository".to_string(),
            format!("Remote directory `{cwd}` is not a Git repository."),
        )
    } else if payload.diff_unified.trim().is_empty() {
        (
            format!("{root_for_name} · remote diff"),
            "Clean working tree on remote host.".to_string(),
        )
    } else {
        (
            format!("{root_for_name} · remote diff"),
            payload.diff_unified.clone(),
        )
    };

    g.repo_snapshot = RepoInspectorSnapshot {
        repo_path: root_for_name.to_string(),
        repo_root: payload.repo_root,
        repo_name,
        is_git_repo: eff_is_git_repo,
        branch: if eff_is_git_repo {
            clean_branch
        } else {
            String::new()
        },
        head_summary: format!("{} changed (E2EE remote)", changed_files.len()),
        diff_added: payload
            .diff_unified
            .lines()
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .count() as u32,
        diff_deleted: payload
            .diff_unified
            .lines()
            .filter(|l| l.starts_with('-') && !l.starts_with("---"))
            .count() as u32,
        changed_files,
        diff_title,
        diff_text,
        error: clean_err,
        ..Default::default()
    };
    true
}

pub(crate) fn spawn_device_login(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    ui_dirty: Arc<AtomicBool>,
    mesh_hub: Arc<RemoteMeshHub>,
    server_url: String,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let cfg_path = DaemonConfig::default_path();
            let existing_identity = load_desktop_daemon_config(&cfg_path)
                .ok()
                .map(|c| c.identity);
            match start_device_login_flow(&server_url, "hybrid", None, existing_identity).await {
                Ok(flow) => {
                    let uri = flow.verification_uri_complete.clone();
                    #[cfg(target_os = "macos")]
                    if is_safe_http_url(&uri) {
                        let _ = std::process::Command::new("open").arg(&uri).spawn();
                    }
                    if let Ok(mut g) = state_bg.lock() {
                        g.device_login_code = Some(flow.user_code.clone());
                        g.device_login_uri = Some(uri);
                        g.auth_error = None;
                        g.status_notice = Some("Approve sign-in in your browser...".to_string());
                    }
                    ui_dirty.store(true, Ordering::Release);

                    match poll_device_login_until_approved(&flow).await {
                        Ok(cfg) => {
                            mesh_hub.reset_for_account_switch().await;
                            let _ = save_desktop_daemon_config(&cfg, &cfg_path);
                            if let Ok(mut g) = state_bg.lock() {
                                g.is_logged_in = true;
                                g.auth_form_expanded = false;
                                g.auth_loading = false;
                                g.auth_error = None;
                                g.local_host_device_id = Some(cfg.device_id);
                                g.input_target = ActiveInputTarget::TerminalDirect;
                                g.account_email = cfg
                                    .owner_email
                                    .clone()
                                    .unwrap_or_else(|| cfg.owner_handle.clone());
                                g.device_login_code = None;
                                g.status_notice = Some("Signed in".to_string());
                            }
                            ui_dirty.store(true, Ordering::Release);
                            mesh_hub.ensure_embedded_host();
                            mesh_hub.start_account_sync();
                            mesh_hub.fetch_notification_webhooks();
                        }
                        Err(err) => {
                            let msg = format!("Login failed: {err}");
                            if let Ok(mut g) = state_bg.lock() {
                                g.auth_loading = false;
                                g.auth_error = Some(msg.clone());
                                g.status_notice = Some(msg);
                            }
                            ui_dirty.store(true, Ordering::Release);
                        }
                    }
                }
                Err(err) => {
                    let msg = format!("Auth init failed: {err}");
                    if let Ok(mut g) = state_bg.lock() {
                        g.auth_loading = false;
                        g.auth_error = Some(msg.clone());
                        g.status_notice = Some(msg);
                    }
                    ui_dirty.store(true, Ordering::Release);
                }
            }
        });
    });
}

pub(crate) fn spawn_password_login(
    state_bg: Arc<Mutex<CodaDesktopState>>,
    ui_dirty: Arc<AtomicBool>,
    mesh_hub: Arc<RemoteMeshHub>,
    server_url: String,
    email: String,
    password: String,
) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let cfg_path = DaemonConfig::default_path();
            let existing_identity = load_desktop_daemon_config(&cfg_path)
                .ok()
                .map(|c| c.identity);
            match login_with_password_and_enroll_with_identity(
                &server_url,
                &email,
                &password,
                "hybrid",
                None,
                existing_identity,
            )
            .await
            {
                Ok(cfg) => {
                    mesh_hub.reset_for_account_switch().await;
                    let _ = save_desktop_daemon_config(&cfg, &cfg_path);
                    if let Ok(mut g) = state_bg.lock() {
                        g.is_logged_in = true;
                        g.auth_form_expanded = false;
                        g.auth_loading = false;
                        g.auth_error = None;
                        g.local_host_device_id = Some(cfg.device_id);
                        g.input_target = ActiveInputTarget::TerminalDirect;
                        g.account_email = cfg.owner_email.clone().unwrap_or(email);
                        g.auth_password_input.clear();
                        g.status_notice = Some("Signed in".to_string());
                    }
                    ui_dirty.store(true, Ordering::Release);
                    mesh_hub.ensure_embedded_host();
                    mesh_hub.start_account_sync();
                    mesh_hub.fetch_notification_webhooks();
                }
                Err(err) => {
                    let msg = format!("Login error: {err}");
                    if let Ok(mut g) = state_bg.lock() {
                        g.auth_loading = false;
                        g.auth_error = Some(msg.clone());
                        g.status_notice = Some(msg);
                    }
                    ui_dirty.store(true, Ordering::Release);
                }
            }
        });
    });
}
