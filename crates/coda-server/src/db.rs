use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
    task::{Context as TaskContext, Poll},
};

use anyhow::{anyhow, bail, Context, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use coda_protocol::{
    open_opaque_secret, seal_opaque_secret, validate_ed25519_pub_b64, validate_x25519_pub_b64,
    TrustedPeerEntry,
};
use rand::{Rng, RngCore};
use rustls_pki_types::ServerName;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio_postgres::{
    tls::{ChannelBinding, MakeTlsConnect, TlsConnect, TlsStream},
    Client, NoTls,
};
use tokio_rustls::{client::TlsStream as RustlsStream, TlsConnector};
use tracing::{error, info};
use uuid::Uuid;

const INIT_SQL: &str = include_str!("../../../migrations/001_init.sql");
const READ_POOL_SIZE: usize = 4;
const TX_POOL_SIZE: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub handle: String,
    pub display_name: String,
    pub is_admin: bool,
    pub bandwidth_quota_mbps: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteCodeRecord {
    pub code: String,
    pub note: String,
    pub max_uses: i32,
    pub used_count: i32,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub role: String,
    pub platform: String,
    pub ed25519_pubkey: String,
    pub x25519_pubkey: String,
    pub endpoints_json: Value,
    pub herdr_core_id: Option<String>,
    pub herdr_version: Option<String>,
    pub is_online: bool,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayNodeRecord {
    pub id: String,
    pub region: String,
    pub mode: String,
    pub endpoint: String,
    pub node_pubkey: String,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAuthPublicInfo {
    pub user_code: String,
    pub device_name: String,
    pub role: String,
    pub platform: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeviceAuthPollState {
    Pending {
        expires_in_secs: i64,
    },
    Approved {
        session_token: String,
        user: UserRecord,
        device: DeviceRecord,
    },
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSessionRecord {
    pub id: Uuid,
    pub device_id: Option<Uuid>,
    pub scope: String,
    pub client_label: String,
    pub last_used_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub is_current: bool,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub user: UserRecord,
    pub device_id: Option<Uuid>,
    pub scope: String,
    pub token_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSettingsRecord {
    pub bark_url: String,
    pub telegram_bot_token: String,
    pub telegram_chat_id: String,
    pub feishu_webhook_url: String,
    pub discord_webhook_url: String,
    pub generic_webhook_url: String,
    pub notify_on_needs_you: bool,
    pub notify_on_finished: bool,
}

impl Default for NotificationSettingsRecord {
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

struct DbInner {
    database_url: String,
    token_wrap_key: Vec<u8>,
    read_pool: Vec<RwLock<Option<Arc<Client>>>>,
    read_rr: AtomicUsize,
    reconnect_lock: Mutex<()>,
    tx_sem: Arc<Semaphore>,
    tx_idle: Mutex<Vec<Client>>,
    dummy_password_hash: String,
}

struct PooledTxClient {
    inner: Arc<DbInner>,
    client: Option<Client>,
    _permit: OwnedSemaphorePermit,
}

impl PooledTxClient {
    fn as_mut(&mut self) -> &mut Client {
        self.client.as_mut().expect("pooled tx client present")
    }
}

impl Drop for PooledTxClient {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            if !client.is_closed() {
                if let Ok(mut idle) = self.inner.tx_idle.try_lock() {
                    idle.push(client);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct Db {
    inner: Arc<DbInner>,
}

const ADMIN_BOOTSTRAP_ADVISORY_LOCK_KEY: i64 = 7_240_991;
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Clone)]
struct PgRustlsConnect {
    config: Arc<rustls::ClientConfig>,
}

impl PgRustlsConnect {
    fn new() -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .expect("rustls default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        Self {
            config: Arc::new(config),
        }
    }
}

struct PgRustlsConnector {
    connector: TlsConnector,
    server_name: ServerName<'static>,
}

struct PgRustlsStream<S>(RustlsStream<S>);

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for PgRustlsStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for PgRustlsStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> TlsStream for PgRustlsStream<S> {
    fn channel_binding(&self) -> ChannelBinding {
        ChannelBinding::none()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> MakeTlsConnect<S> for PgRustlsConnect {
    type Stream = PgRustlsStream<S>;
    type TlsConnect = PgRustlsConnector;
    type Error = io::Error;

    fn make_tls_connect(&mut self, hostname: &str) -> std::result::Result<Self::TlsConnect, Self::Error> {
        let server_name = ServerName::try_from(hostname.to_string())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid TLS server name"))?;
        Ok(PgRustlsConnector {
            connector: TlsConnector::from(Arc::clone(&self.config)),
            server_name,
        })
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send + 'static> TlsConnect<S> for PgRustlsConnector {
    type Stream = PgRustlsStream<S>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = std::result::Result<Self::Stream, Self::Error>> + Send>>;

    fn connect(self, stream: S) -> Self::Future {
        Box::pin(async move {
            let tls = self.connector.connect(self.server_name, stream).await?;
            Ok(PgRustlsStream(tls))
        })
    }
}

fn redact_database_url(url: &str) -> String {
    let Some((scheme_and_auth, rest)) = url.split_once('@') else {
        return url.to_string();
    };
    if let Some((scheme_user, _pw)) = scheme_and_auth.rsplit_once(':') {
        if scheme_user.contains("://") {
            return format!("{scheme_user}:***@{rest}");
        }
    }
    url.to_string()
}

fn wants_postgres_tls(database_url: &str) -> bool {
    let lower = database_url.to_ascii_lowercase();
    lower.contains("sslmode=require")
        || lower.contains("sslmode=prefer")
        || lower.contains("sslmode=verify-ca")
        || lower.contains("sslmode=verify-full")
}

fn normalize_database_url_for_tokio_pg(database_url: &str) -> String {
    database_url
        .replace("sslmode=verify-full", "sslmode=require")
        .replace("sslmode=verify-ca", "sslmode=require")
}

impl Db {
    async fn connect_raw(database_url: &str) -> Result<Client> {
        let redacted = redact_database_url(database_url);
        let use_tls = wants_postgres_tls(database_url);
        let normalized = if use_tls {
            normalize_database_url_for_tokio_pg(database_url)
        } else {
            database_url.to_string()
        };
        let mut cfg: tokio_postgres::Config = normalized
            .parse()
            .with_context(|| format!("invalid PostgreSQL connection string: {redacted}"))?;
        if cfg.get_password().is_none() {
            if let Ok(pg_pw) = std::env::var("PGPASSWORD") {
                if !pg_pw.is_empty() {
                    cfg.password(pg_pw);
                }
            }
        }

        if use_tls {
            let tls = PgRustlsConnect::new();
            let (client, connection) = cfg
                .connect(tls)
                .await
                .with_context(|| format!("failed connecting to PostgreSQL (TLS): {redacted}"))?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    error!("PostgreSQL TLS connection error: {e}");
                }
            });
            return Ok(client);
        }

        let (client, connection) = cfg
            .connect(NoTls)
            .await
            .with_context(|| format!("failed connecting to PostgreSQL: {redacted}"))?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("PostgreSQL connection error: {e}");
            }
        });
        Ok(client)
    }

    pub async fn connect_and_migrate(database_url: &str) -> Result<Self> {
        let default_secret = std::env::var("CODA_RELAY_SECRET")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.into_bytes())
            .unwrap_or_else(|| {
                let mut key = vec![0u8; 32];
                OsRng.fill_bytes(&mut key);
                key
            });
        Self::connect_and_migrate_with_secret(database_url, &default_secret).await
    }

    pub async fn connect_and_migrate_with_secret(
        database_url: &str,
        token_wrap_secret: &[u8],
    ) -> Result<Self> {
        let client = Self::connect_raw(database_url).await?;

        client
            .batch_execute(INIT_SQL)
            .await
            .context("failed applying migrations/001_init.sql")?;

        // Reset stale online flags from any unclean previous restart
        client
            .execute("UPDATE devices SET is_online = FALSE WHERE is_online = TRUE", &[])
            .await?;

        let dummy_password_hash = tokio::task::spawn_blocking(|| {
            let salt = SaltString::generate(&mut OsRng);
            Argon2::default()
                .hash_password(b"coda-timing-equalizer-dummy", &salt)
                .map(|h| h.to_string())
                .map_err(|e| anyhow!("dummy argon2 hash error: {e}"))
        })
        .await??;

        let mut read_pool = Vec::with_capacity(READ_POOL_SIZE);
        read_pool.push(RwLock::new(Some(Arc::new(client))));
        for _ in 1..READ_POOL_SIZE {
            read_pool.push(RwLock::new(None));
        }

        let inner = Arc::new(DbInner {
            database_url: database_url.to_string(),
            token_wrap_key: token_wrap_secret.to_vec(),
            read_pool,
            read_rr: AtomicUsize::new(0),
            reconnect_lock: Mutex::new(()),
            tx_sem: Arc::new(Semaphore::new(TX_POOL_SIZE)),
            tx_idle: Mutex::new(Vec::with_capacity(TX_POOL_SIZE)),
            dummy_password_hash,
        });

        spawn_expiry_cleanup_task(Arc::downgrade(&inner));

        info!("PostgreSQL connected (pool={READ_POOL_SIZE}r/{TX_POOL_SIZE}tx) and migrations verified");
        Ok(Self { inner })
    }

    async fn client(&self) -> Result<Arc<Client>> {
        let idx = self.inner.read_rr.fetch_add(1, Ordering::Relaxed) % self.inner.read_pool.len();
        {
            let guard = self.inner.read_pool[idx].read().await;
            if let Some(c) = guard.as_ref() {
                if !c.is_closed() {
                    return Ok(Arc::clone(c));
                }
            }
        }
        let _reconn = self.inner.reconnect_lock.lock().await;
        {
            let guard = self.inner.read_pool[idx].read().await;
            if let Some(c) = guard.as_ref() {
                if !c.is_closed() {
                    return Ok(Arc::clone(c));
                }
            }
        }
        let fresh = Arc::new(Self::connect_raw(&self.inner.database_url).await?);
        let mut guard = self.inner.read_pool[idx].write().await;
        *guard = Some(Arc::clone(&fresh));
        Ok(fresh)
    }

    async fn acquire_tx_client(&self) -> Result<PooledTxClient> {
        let permit = Arc::clone(&self.inner.tx_sem)
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("tx connection pool closed"))?;
        let existing = {
            let mut idle = self.inner.tx_idle.lock().await;
            let mut found = None;
            while let Some(c) = idle.pop() {
                if !c.is_closed() {
                    found = Some(c);
                    break;
                }
            }
            found
        };
        let client = match existing {
            Some(c) => c,
            None => Self::connect_raw(&self.inner.database_url).await?,
        };
        Ok(PooledTxClient {
            inner: Arc::clone(&self.inner),
            client: Some(client),
            _permit: permit,
        })
    }

    /// Executes a lightweight query verifying live database reachability.
    pub async fn ping(&self) -> Result<()> {
        let client = self.client().await?;
        client
            .simple_query("SELECT 1")
            .await
            .context("database health check failed")?;
        Ok(())
    }

    /// Deletes expired auth sessions, pairing tokens, and device auth codes.
    pub async fn purge_expired_records(&self) -> Result<()> {
        let client = self.client().await?;
        purge_expired_with_client(&client).await
    }

    pub async fn seed_default_relays(
        &self,
        raw_tcp_endpoint: &str,
        cdn_wss_endpoint: &str,
        server_pubkey: &str,
    ) -> Result<()> {
        self.seed_smart_relays(raw_tcp_endpoint, true, cdn_wss_endpoint, server_pubkey)
            .await
    }

    pub async fn seed_smart_relays(
        &self,
        raw_tcp_endpoint: &str,
        raw_tcp_active: bool,
        cdn_wss_endpoint: &str,
        server_pubkey: &str,
    ) -> Result<()> {
        let client = self.client().await?;
        let wss_priority: i32 = if raw_tcp_active { 20 } else { 5 };
        client
            .execute(
                r#"
                INSERT INTO relay_nodes (id, region, mode, endpoint, node_pubkey, priority, is_active)
                VALUES ('cn-direct-tcp', 'CN-BGP (No-ICP Raw TCP)', 'raw_tcp_no_sni', $1, $2, 10, $3)
                ON CONFLICT (id) DO UPDATE
                SET endpoint = EXCLUDED.endpoint,
                    node_pubkey = EXCLUDED.node_pubkey,
                    is_active = EXCLUDED.is_active,
                    updated_at = NOW()
                "#,
                &[&raw_tcp_endpoint, &server_pubkey, &raw_tcp_active],
            )
            .await?;

        client
            .execute(
                r#"
                INSERT INTO relay_nodes (id, region, mode, endpoint, node_pubkey, priority, is_active)
                VALUES ('global-cdn-wss', 'Global Edge CDN / CF Tunnel (WSS 443)', 'cdn_wss', $1, $2, $3, TRUE)
                ON CONFLICT (id) DO UPDATE
                SET region = EXCLUDED.region,
                    endpoint = EXCLUDED.endpoint,
                    node_pubkey = EXCLUDED.node_pubkey,
                    priority = EXCLUDED.priority,
                    is_active = TRUE,
                    updated_at = NOW()
                "#,
                &[&cdn_wss_endpoint, &server_pubkey, &wss_priority],
            )
            .await?;
        Ok(())
    }

    pub async fn has_any_users(&self) -> Result<bool> {
        let client = self.client().await?;
        let row = client
            .query_one("SELECT EXISTS(SELECT 1 FROM users)", &[])
            .await?;
        Ok(row.get(0))
    }

    /// Registers a new user.
    /// - If `users` table is completely empty, this user becomes the Owner/Admin (`is_admin = true`) without needing an invite code.
    /// - Subsequent users require a valid `invite_code` unless `open_registration` is enabled.
    pub async fn register_user(
        &self,
        email: &str,
        handle: &str,
        display_name: &str,
        password: &str,
        invite_code: Option<&str>,
    ) -> Result<(UserRecord, String)> {
        self.register_user_with_policy(email, handle, display_name, password, invite_code, false)
            .await
    }

    pub async fn register_user_with_policy(
        &self,
        email: &str,
        handle: &str,
        display_name: &str,
        password: &str,
        invite_code: Option<&str>,
        open_registration: bool,
    ) -> Result<(UserRecord, String)> {
        let email = email.trim().to_lowercase();
        let handle = handle.trim().to_lowercase();
        let display_name = display_name.trim().to_string();
        validate_email(&email)?;
        validate_handle(&handle)?;
        if display_name.is_empty() || display_name.len() > 128 {
            bail!("display_name must be 1..=128 characters");
        }
        if password.len() < 6 || password.len() > MAX_PASSWORD_BYTES {
            bail!("password must be 6..={MAX_PASSWORD_BYTES} bytes");
        }

        // Fast pre-check before spending Argon2 CPU when registration is invite-gated.
        let trimmed_invite = invite_code.map(str::trim).filter(|s| !s.is_empty());
        {
            let pre_client = self.client().await?;
            let has_users: bool = pre_client
                .query_one("SELECT EXISTS(SELECT 1 FROM users)", &[])
                .await?
                .get(0);
            if has_users {
                if let Some(code) = trimmed_invite {
                    let valid: bool = pre_client
                        .query_one(
                            r#"
                            SELECT EXISTS(
                                SELECT 1 FROM invite_codes
                                WHERE code = $1
                                  AND used_count < max_uses
                                  AND (expires_at IS NULL OR expires_at > NOW())
                            )
                            "#,
                            &[&code],
                        )
                        .await?
                        .get(0);
                    if !valid {
                        bail!("invalid, expired, or exhausted invite code");
                    }
                } else if !open_registration {
                    bail!("invite_code is required to register on this server");
                }
            }
        }

        let pw_owned = password.to_string();
        let password_hash = tokio::task::spawn_blocking(move || {
            let salt = SaltString::generate(&mut OsRng);
            Argon2::default()
                .hash_password(pw_owned.as_bytes(), &salt)
                .map(|h| h.to_string())
                .map_err(|e| anyhow!("password hash error: {e}"))
        })
        .await??;

        let mut tx_guard = self.acquire_tx_client().await?;
        let tx = tx_guard.as_mut().transaction().await?;

        // Serialize bootstrap count check so concurrent initial registrations cannot both become admin.
        tx.execute(
            "SELECT pg_advisory_xact_lock($1)",
            &[&ADMIN_BOOTSTRAP_ADVISORY_LOCK_KEY],
        )
        .await?;

        let count_row = tx
            .query_one("SELECT COUNT(*)::BIGINT FROM users", &[])
            .await?;
        let user_count: i64 = count_row.get(0);
        let is_first_admin = user_count == 0;

        if !is_first_admin {
            if let Some(code) = trimmed_invite {
                let updated = tx
                    .execute(
                        r#"
                        UPDATE invite_codes
                        SET used_count = used_count + 1
                        WHERE code = $1
                          AND used_count < max_uses
                          AND (expires_at IS NULL OR expires_at > NOW())
                        "#,
                        &[&code],
                    )
                    .await?;
                if updated == 0 {
                    bail!("invalid, expired, or exhausted invite code");
                }
            } else if !open_registration {
                bail!("invite_code is required to register on this server");
            }
        }

        let quota_mbps: i32 = if is_first_admin { 100 } else { 25 };
        let row = tx
            .query_one(
                r#"
                INSERT INTO users (email, handle, display_name, password_hash, is_admin, bandwidth_quota_mbps, last_login_at)
                VALUES ($1, $2, $3, $4, $5, $6, NOW())
                RETURNING id, email, handle, display_name, is_admin, bandwidth_quota_mbps, created_at
                "#,
                &[
                    &email,
                    &handle,
                    &display_name,
                    &password_hash,
                    &is_first_admin,
                    &quota_mbps,
                ],
            )
            .await
            .context("email or handle already registered")?;

        let user = UserRecord {
            id: row.get(0),
            email: row.get(1),
            handle: row.get(2),
            display_name: row.get(3),
            is_admin: row.get(4),
            bandwidth_quota_mbps: row.get(5),
            created_at: row.get(6),
        };

        let raw_token = generate_raw_session_token();
        let token_sha256 = sha256_hex(&raw_token);
        let expires_at = Utc::now() + Duration::days(30);
        tx.execute(
            r#"
            INSERT INTO auth_sessions (token_sha256, user_id, client_label, expires_at)
            VALUES ($1, $2, 'register', $3)
            "#,
            &[&token_sha256, &user.id, &expires_at],
        )
        .await?;

        tx.commit().await?;
        Ok((user, raw_token))
    }

    pub async fn login_user(
        &self,
        email_or_handle: &str,
        password: &str,
        client_label: &str,
    ) -> Result<(UserRecord, String)> {
        if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
            bail!("invalid credentials");
        }
        let client = self.client().await?;
        let key = email_or_handle.trim().to_lowercase();
        let row = client
            .query_opt(
                r#"
                SELECT id, email, handle, display_name, password_hash, is_admin, bandwidth_quota_mbps, created_at
                FROM users
                WHERE LOWER(email) = $1 OR LOWER(handle) = $1
                "#,
                &[&key],
            )
            .await?;

        let Some(row) = row else {
            let dummy = self.inner.dummy_password_hash.clone();
            let pw = password.to_string();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(parsed) = PasswordHash::new(&dummy) {
                    let _ = Argon2::default().verify_password(pw.as_bytes(), &parsed);
                }
            })
            .await;
            bail!("invalid credentials");
        };

        let pw_hash_str: String = row.get(4);
        let pw_owned = password.to_string();
        let verified = tokio::task::spawn_blocking(move || -> Result<()> {
            let parsed_hash =
                PasswordHash::new(&pw_hash_str).map_err(|_| anyhow!("corrupted password hash"))?;
            Argon2::default()
                .verify_password(pw_owned.as_bytes(), &parsed_hash)
                .map_err(|_| anyhow!("invalid credentials"))
        })
        .await??;
        let _ = verified;

        let user = UserRecord {
            id: row.get(0),
            email: row.get(1),
            handle: row.get(2),
            display_name: row.get(3),
            is_admin: row.get(5),
            bandwidth_quota_mbps: row.get(6),
            created_at: row.get(7),
        };

        client
            .execute("UPDATE users SET last_login_at = NOW() WHERE id = $1", &[&user.id])
            .await?;

        let _ = self.prune_stale_web_devices(user.id, None, 30).await;

        let token = self.issue_session_token(user.id, client_label).await?;
        Ok((user, token))
    }

    async fn issue_session_token(&self, user_id: Uuid, client_label: &str) -> Result<String> {
        let client = self.client().await?;
        let raw_token = generate_raw_session_token();
        let token_sha256 = sha256_hex(&raw_token);
        let expires_at = Utc::now() + Duration::days(30);
        let label: String = client_label.chars().take(64).collect();

        client
            .execute(
                r#"
                INSERT INTO auth_sessions (token_sha256, user_id, client_label, expires_at)
                VALUES ($1, $2, $3, $4)
                "#,
                &[&token_sha256, &user_id, &label, &expires_at],
            )
            .await?;

        Ok(raw_token)
    }

    pub async fn delete_session(&self, raw_token: &str) -> Result<Option<(Uuid, String)>> {
        let client = self.client().await?;
        let token_sha256 = sha256_hex(raw_token.trim());
        let row = client
            .query_opt(
                "DELETE FROM auth_sessions WHERE token_sha256 = $1 RETURNING user_id",
                &[&token_sha256],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), token_sha256)))
    }

    pub async fn authenticate_session(&self, raw_token: &str) -> Result<AuthenticatedSession> {
        let client = self.client().await?;
        let token_sha256 = sha256_hex(raw_token.trim());
        let row = client
            .query_opt(
                r#"
                SELECT u.id, u.email, u.handle, u.display_name, u.is_admin, u.bandwidth_quota_mbps, u.created_at,
                       s.scope, s.expires_at, s.last_used_at, s.device_id
                FROM auth_sessions s
                JOIN users u ON u.id = s.user_id
                WHERE s.token_sha256 = $1 AND s.expires_at > NOW()
                "#,
                &[&token_sha256],
            )
            .await?
            .ok_or_else(|| anyhow!("unauthorized or expired session token"))?;

        let scope: String = row.get(7);
        let expires_at: DateTime<Utc> = row.get(8);
        let last_used_at: DateTime<Utc> = row.get(9);
        let device_id: Option<Uuid> = row.get(10);
        let now = Utc::now();

        // Sliding renewal: refresh last_used_at at most hourly and extend active sessions with <15d left.
        if now - last_used_at > Duration::hours(1) || expires_at - now < Duration::days(15) {
            let renewed_exp = expires_at.max(now + Duration::days(30));
            let _ = client
                .execute(
                    "UPDATE auth_sessions SET last_used_at = NOW(), expires_at = $2 WHERE token_sha256 = $1",
                    &[&token_sha256, &renewed_exp],
                )
                .await;
        }

        Ok(AuthenticatedSession {
            user: UserRecord {
                id: row.get(0),
                email: row.get(1),
                handle: row.get(2),
                display_name: row.get(3),
                is_admin: row.get(4),
                bandwidth_quota_mbps: row.get(5),
                created_at: row.get(6),
            },
            device_id,
            scope,
            token_sha256,
        })
    }

    pub async fn authenticate_token(&self, raw_token: &str) -> Result<UserRecord> {
        Ok(self.authenticate_session(raw_token).await?.user)
    }

    pub async fn list_sessions(
        &self,
        user_id: Uuid,
        current_token_sha256: &str,
    ) -> Result<Vec<AuthSessionRecord>> {
        let client = self.client().await?;
        let rows = client
            .query(
                r#"
                SELECT id, device_id, scope, client_label, last_used_at, expires_at, created_at, token_sha256
                FROM auth_sessions
                WHERE user_id = $1 AND expires_at > NOW()
                ORDER BY last_used_at DESC
                LIMIT 100
                "#,
                &[&user_id],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let sha: String = r.get(7);
                AuthSessionRecord {
                    id: r.get(0),
                    device_id: r.get(1),
                    scope: r.get(2),
                    client_label: r.get(3),
                    last_used_at: r.get(4),
                    expires_at: r.get(5),
                    created_at: r.get(6),
                    is_current: sha == current_token_sha256,
                }
            })
            .collect())
    }

    pub async fn delete_session_by_id(
        &self,
        user_id: Uuid,
        session_id: Uuid,
    ) -> Result<Option<String>> {
        let client = self.client().await?;
        let row = client
            .query_opt(
                "DELETE FROM auth_sessions WHERE id = $1 AND user_id = $2 RETURNING token_sha256",
                &[&session_id, &user_id],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    pub async fn delete_all_sessions(
        &self,
        user_id: Uuid,
        except_token_sha256: Option<&str>,
    ) -> Result<u64> {
        let client = self.client().await?;
        let deleted = if let Some(keep) = except_token_sha256 {
            client
                .execute(
                    "DELETE FROM auth_sessions WHERE user_id = $1 AND token_sha256 <> $2",
                    &[&user_id, &keep],
                )
                .await?
        } else {
            client
                .execute("DELETE FROM auth_sessions WHERE user_id = $1", &[&user_id])
                .await?
        };
        Ok(deleted)
    }

    pub async fn change_password(
        &self,
        user_id: Uuid,
        current_password: &str,
        new_password: &str,
        keep_token_sha256: Option<&str>,
    ) -> Result<Vec<String>> {
        if new_password.len() < 6 || new_password.len() > MAX_PASSWORD_BYTES {
            bail!("new password must be 6..={MAX_PASSWORD_BYTES} bytes");
        }
        if current_password.is_empty() || current_password.len() > MAX_PASSWORD_BYTES {
            bail!("invalid current password");
        }
        let client = self.client().await?;
        let row = client
            .query_opt("SELECT password_hash FROM users WHERE id = $1", &[&user_id])
            .await?
            .ok_or_else(|| anyhow!("user not found"))?;
        let existing_hash: String = row.get(0);
        let cur_pw = current_password.to_string();
        let new_pw = new_password.to_string();
        let new_hash = tokio::task::spawn_blocking(move || -> Result<String> {
            let parsed =
                PasswordHash::new(&existing_hash).map_err(|_| anyhow!("corrupted password hash"))?;
            Argon2::default()
                .verify_password(cur_pw.as_bytes(), &parsed)
                .map_err(|_| anyhow!("current password is incorrect"))?;
            let salt = SaltString::generate(&mut OsRng);
            Argon2::default()
                .hash_password(new_pw.as_bytes(), &salt)
                .map(|h| h.to_string())
                .map_err(|e| anyhow!("password hash error: {e}"))
        })
        .await??;

        let mut tx_guard = self.acquire_tx_client().await?;
        let tx = tx_guard.as_mut().transaction().await?;
        tx.execute(
            "UPDATE users SET password_hash = $2 WHERE id = $1",
            &[&user_id, &new_hash],
        )
        .await?;
        let revoked_rows = if let Some(keep) = keep_token_sha256 {
            tx.query(
                "DELETE FROM auth_sessions WHERE user_id = $1 AND token_sha256 <> $2 RETURNING token_sha256",
                &[&user_id, &keep],
            )
            .await?
        } else {
            tx.query(
                "DELETE FROM auth_sessions WHERE user_id = $1 RETURNING token_sha256",
                &[&user_id],
            )
            .await?
        };
        let revoked_hashes: Vec<String> = revoked_rows.into_iter().map(|r| r.get(0)).collect();
        tx.commit().await?;
        Ok(revoked_hashes)
    }

    pub async fn delete_user_account(&self, user_id: Uuid, password: &str) -> Result<Vec<Uuid>> {
        if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
            bail!("invalid password");
        }
        let client = self.client().await?;
        let row = client
            .query_opt("SELECT password_hash FROM users WHERE id = $1", &[&user_id])
            .await?
            .ok_or_else(|| anyhow!("user not found"))?;
        let existing_hash: String = row.get(0);
        let pw = password.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let parsed =
                PasswordHash::new(&existing_hash).map_err(|_| anyhow!("corrupted password hash"))?;
            Argon2::default()
                .verify_password(pw.as_bytes(), &parsed)
                .map_err(|_| anyhow!("password is incorrect"))
        })
        .await??;

        let dev_rows = client
            .query("SELECT id FROM devices WHERE user_id = $1", &[&user_id])
            .await?;
        let dev_ids: Vec<Uuid> = dev_rows.into_iter().map(|r| r.get(0)).collect();
        client
            .execute("DELETE FROM users WHERE id = $1", &[&user_id])
            .await?;
        Ok(dev_ids)
    }

    pub async fn create_invite_code(
        &self,
        creator: &UserRecord,
        note: &str,
        max_uses: i32,
        valid_days: i64,
    ) -> Result<InviteCodeRecord> {
        if !creator.is_admin {
            bail!("only admin accounts can generate new friend invite codes");
        }
        let client = self.client().await?;
        let expires_at = Some(Utc::now() + Duration::days(valid_days.clamp(1, 365)));
        let max_uses = max_uses.clamp(1, 500);
        let clean_note: String = note.trim().chars().take(128).collect();

        for _ in 0..10 {
            let code = generate_invite_code();
            let row = client
                .query_opt(
                    r#"
                    INSERT INTO invite_codes (code, created_by, note, max_uses, expires_at)
                    VALUES ($1, $2, $3, $4, $5)
                    ON CONFLICT (code) DO NOTHING
                    RETURNING code, note, max_uses, used_count, expires_at, created_at
                    "#,
                    &[&code, &creator.id, &clean_note, &max_uses, &expires_at],
                )
                .await?;
            if let Some(row) = row {
                return Ok(InviteCodeRecord {
                    code: row.get(0),
                    note: row.get(1),
                    max_uses: row.get(2),
                    used_count: row.get(3),
                    expires_at: row.get(4),
                    created_at: row.get(5),
                });
            }
        }
        bail!("failed allocating unique invite code after retries")
    }

    pub async fn list_invite_codes(&self, user: &UserRecord) -> Result<Vec<InviteCodeRecord>> {
        if !user.is_admin {
            bail!("only admin accounts can list invite codes");
        }
        let client = self.client().await?;
        let rows = client
            .query(
                "SELECT code, note, max_uses, used_count, expires_at, created_at FROM invite_codes ORDER BY created_at DESC LIMIT 50",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| InviteCodeRecord {
                code: r.get(0),
                note: r.get(1),
                max_uses: r.get(2),
                used_count: r.get(3),
                expires_at: r.get(4),
                created_at: r.get(5),
            })
            .collect())
    }

    pub async fn delete_invite_code(&self, user: &UserRecord, code: &str) -> Result<bool> {
        if !user.is_admin {
            bail!("only admin accounts can revoke invite codes");
        }
        let client = self.client().await?;
        let deleted = client
            .execute(
                "DELETE FROM invite_codes WHERE code = $1",
                &[&code.trim().to_uppercase()],
            )
            .await?;
        Ok(deleted > 0)
    }

    /// Generates a short 6-digit pairing code (`839-204`) valid for 10 minutes.
    pub async fn create_pairing_token(
        &self,
        user_id: Uuid,
        suggested_name: Option<&str>,
    ) -> Result<(String, DateTime<Utc>)> {
        let client = self.client().await?;
        let expires_at = Utc::now() + Duration::minutes(10);
        for _ in 0..10 {
            let a: u16 = rand::thread_rng().gen_range(100..=999);
            let b: u16 = rand::thread_rng().gen_range(100..=999);
            let code = format!("{a}-{b}");
            let res = client
                .execute(
                    r#"
                    INSERT INTO pairing_tokens (code, user_id, suggested_name, expires_at)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (code) DO NOTHING
                    "#,
                    &[&code, &user_id, &suggested_name, &expires_at],
                )
                .await?;
            if res > 0 {
                return Ok((code, expires_at));
            }
        }
        bail!("failed allocating unique pairing token after retries")
    }

    /// Daemon claims a 6-digit pairing code and registers its Ed25519 + X25519 public keys atomically.
    #[allow(clippy::too_many_arguments)]
    pub async fn claim_pairing_token(
        &self,
        code: &str,
        name: &str,
        platform: &str,
        ed25519_pubkey: &str,
        x25519_pubkey: &str,
        endpoints_json: &Value,
        herdr_core_id: Option<&str>,
        herdr_version: Option<&str>,
    ) -> Result<(DeviceRecord, UserRecord)> {
        validate_device_inputs(name, "daemon", platform, ed25519_pubkey, x25519_pubkey)?;
        let normalized = code.trim();

        let mut tx_guard = self.acquire_tx_client().await?;
        let tx = tx_guard.as_mut().transaction().await?;

        let token_row = tx
            .query_opt(
                r#"
                SELECT user_id, suggested_name
                FROM pairing_tokens
                WHERE code = $1 AND claimed_device_id IS NULL AND expires_at > NOW()
                FOR UPDATE
                "#,
                &[&normalized],
            )
            .await?
            .ok_or_else(|| anyhow!("invalid, expired, or already-claimed pairing code"))?;

        let user_id: Uuid = token_row.get(0);
        let suggested: Option<String> = token_row.get(1);
        let final_name = suggested
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| name.trim().to_string());

        let dev_row = tx
            .query_one(
                r#"
                INSERT INTO devices (
                    user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                    endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                )
                VALUES ($1, $2, 'daemon', $3, $4, $5, $6, $7, $8, FALSE, NOW())
                ON CONFLICT (user_id, ed25519_pubkey) DO UPDATE
                SET name = EXCLUDED.name,
                    role = EXCLUDED.role,
                    platform = EXCLUDED.platform,
                    x25519_pubkey = EXCLUDED.x25519_pubkey,
                    endpoints_json = EXCLUDED.endpoints_json,
                    herdr_core_id = EXCLUDED.herdr_core_id,
                    herdr_version = EXCLUDED.herdr_version,
                    last_seen_at = NOW()
                RETURNING id, user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                          endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                "#,
                &[
                    &user_id,
                    &final_name,
                    &platform,
                    &ed25519_pubkey,
                    &x25519_pubkey,
                    endpoints_json,
                    &herdr_core_id,
                    &herdr_version,
                ],
            )
            .await?;
        let dev = map_device_row(&dev_row);

        let updated = tx
            .execute(
                "UPDATE pairing_tokens SET claimed_device_id = $1 WHERE code = $2 AND claimed_device_id IS NULL",
                &[&dev.id, &normalized],
            )
            .await?;
        if updated == 0 {
            bail!("pairing token already claimed");
        }

        let urow = tx
            .query_one(
                "SELECT id, email, handle, display_name, is_admin, bandwidth_quota_mbps, created_at FROM users WHERE id = $1",
                &[&user_id],
            )
            .await?;
        let user = UserRecord {
            id: urow.get(0),
            email: urow.get(1),
            handle: urow.get(2),
            display_name: urow.get(3),
            is_admin: urow.get(4),
            bandwidth_quota_mbps: urow.get(5),
            created_at: urow.get(6),
        };

        tx.commit().await?;
        Ok((dev, user))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_device(
        &self,
        user_id: Uuid,
        name: &str,
        role: &str,
        platform: &str,
        ed25519_pubkey: &str,
        x25519_pubkey: &str,
        endpoints_json: &Value,
        herdr_core_id: Option<&str>,
        herdr_version: Option<&str>,
    ) -> Result<DeviceRecord> {
        validate_device_inputs(name, role, platform, ed25519_pubkey, x25519_pubkey)?;
        let client = self.client().await?;
        let trimmed_name = name.trim();
        let row = client
            .query_one(
                r#"
                INSERT INTO devices (
                    user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                    endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, FALSE, NOW())
                ON CONFLICT (user_id, ed25519_pubkey) DO UPDATE
                SET name = EXCLUDED.name,
                    role = EXCLUDED.role,
                    platform = EXCLUDED.platform,
                    x25519_pubkey = EXCLUDED.x25519_pubkey,
                    endpoints_json = EXCLUDED.endpoints_json,
                    herdr_core_id = EXCLUDED.herdr_core_id,
                    herdr_version = EXCLUDED.herdr_version,
                    last_seen_at = NOW()
                RETURNING id, user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                          endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                "#,
                &[
                    &user_id,
                    &trimmed_name,
                    &role,
                    &platform,
                    &ed25519_pubkey,
                    &x25519_pubkey,
                    endpoints_json,
                    &herdr_core_id,
                    &herdr_version,
                ],
            )
            .await
            .context("device registration failed")?;

        Ok(map_device_row(&row))
    }

    /// Strict tenant-isolated device listing (`WHERE user_id = $1`).
    pub async fn list_user_devices(&self, user_id: Uuid) -> Result<Vec<DeviceRecord>> {
        let client = self.client().await?;
        let rows = client
            .query(
                r#"
                SELECT id, user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                       endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                FROM devices
                WHERE user_id = $1
                ORDER BY is_online DESC, created_at ASC
                "#,
                &[&user_id],
            )
            .await?;
        Ok(rows.iter().map(map_device_row).collect())
    }

    /// Strict tenant-isolated trusted peer directory for cryptographic pinning on daemons & clients.
    pub async fn list_trusted_peers(&self, user_id: Uuid) -> Result<Vec<TrustedPeerEntry>> {
        let client = self.client().await?;
        let rows = client
            .query(
                r#"
                SELECT id, name, role, ed25519_pubkey, x25519_pubkey
                FROM devices
                WHERE user_id = $1
                ORDER BY created_at ASC
                "#,
                &[&user_id],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| TrustedPeerEntry {
                device_id: r.get(0),
                name: r.get(1),
                role: r.get(2),
                ed25519_pub_b64: r.get(3),
                x25519_pub_b64: r.get(4),
            })
            .collect())
    }

    pub async fn rename_device(
        &self,
        user_id: Uuid,
        device_id: Uuid,
        new_name: &str,
    ) -> Result<Option<DeviceRecord>> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() || trimmed.len() > 128 {
            bail!("device name must be 1..=128 characters");
        }
        let client = self.client().await?;
        let row = client
            .query_opt(
                r#"
                UPDATE devices
                SET name = $3
                WHERE id = $1 AND user_id = $2
                RETURNING id, user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                          endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                "#,
                &[&device_id, &user_id, &trimmed],
            )
            .await?;
        Ok(row.as_ref().map(map_device_row))
    }

    pub async fn list_session_hashes_for_device(
        &self,
        user_id: Uuid,
        device_id: Uuid,
    ) -> Result<Vec<String>> {
        let client = self.client().await?;
        let rows = client
            .query(
                "SELECT token_sha256 FROM auth_sessions WHERE user_id = $1 AND device_id = $2",
                &[&user_id, &device_id],
            )
            .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    pub async fn delete_device(&self, user_id: Uuid, device_id: Uuid) -> Result<bool> {
        let client = self.client().await?;
        let deleted = client
            .execute(
                "DELETE FROM devices WHERE id = $1 AND user_id = $2",
                &[&device_id, &user_id],
            )
            .await?;
        Ok(deleted > 0)
    }

    /// Deletes offline `web` client devices belonging to `user_id` (excluding `exclude_device_id` if present).
    /// When `max_idle_days > 0`, restricts deletion to rows with `COALESCE(last_seen_at, created_at) < NOW() - make_interval(days => max_idle_days)`.
    pub async fn prune_stale_web_devices(
        &self,
        user_id: Uuid,
        exclude_device_id: Option<Uuid>,
        max_idle_days: i32,
    ) -> Result<Vec<Uuid>> {
        let client = self.client().await?;
        let rows = if max_idle_days > 0 {
            let days = max_idle_days.clamp(1, 3650);
            client
                .query(
                    r#"
                    DELETE FROM devices
                    WHERE user_id = $1
                      AND platform = 'web'
                      AND role = 'client'
                      AND is_online = FALSE
                      AND ($2::uuid IS NULL OR id <> $2)
                      AND COALESCE(last_seen_at, created_at) < NOW() - make_interval(days => $3)
                    RETURNING id
                    "#,
                    &[&user_id, &exclude_device_id, &days],
                )
                .await?
        } else {
            client
                .query(
                    r#"
                    DELETE FROM devices
                    WHERE user_id = $1
                      AND platform = 'web'
                      AND role = 'client'
                      AND is_online = FALSE
                      AND ($2::uuid IS NULL OR id <> $2)
                    RETURNING id
                    "#,
                    &[&user_id, &exclude_device_id],
                )
                .await?
        };
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    pub async fn get_notification_settings(
        &self,
        user_id: Uuid,
    ) -> Result<NotificationSettingsRecord> {
        let client = self.client().await?;
        let row = client
            .query_opt(
                r#"
                SELECT bark_url, telegram_bot_token, telegram_chat_id,
                       feishu_webhook_url, discord_webhook_url, generic_webhook_url,
                       notify_on_needs_you, notify_on_finished
                FROM user_notification_settings
                WHERE user_id = $1
                "#,
                &[&user_id],
            )
            .await?;
        Ok(match row {
            Some(r) => NotificationSettingsRecord {
                bark_url: r.get(0),
                telegram_bot_token: r.get(1),
                telegram_chat_id: r.get(2),
                feishu_webhook_url: r.get(3),
                discord_webhook_url: r.get(4),
                generic_webhook_url: r.get(5),
                notify_on_needs_you: r.get(6),
                notify_on_finished: r.get(7),
            },
            None => NotificationSettingsRecord::default(),
        })
    }

    pub async fn upsert_notification_settings(
        &self,
        user_id: Uuid,
        settings: &NotificationSettingsRecord,
    ) -> Result<NotificationSettingsRecord> {
        let client = self.client().await?;
        let row = client
            .query_one(
                r#"
                INSERT INTO user_notification_settings (
                    user_id, bark_url, telegram_bot_token, telegram_chat_id,
                    feishu_webhook_url, discord_webhook_url, generic_webhook_url,
                    notify_on_needs_you, notify_on_finished, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
                ON CONFLICT (user_id) DO UPDATE
                SET bark_url = EXCLUDED.bark_url,
                    telegram_bot_token = EXCLUDED.telegram_bot_token,
                    telegram_chat_id = EXCLUDED.telegram_chat_id,
                    feishu_webhook_url = EXCLUDED.feishu_webhook_url,
                    discord_webhook_url = EXCLUDED.discord_webhook_url,
                    generic_webhook_url = EXCLUDED.generic_webhook_url,
                    notify_on_needs_you = EXCLUDED.notify_on_needs_you,
                    notify_on_finished = EXCLUDED.notify_on_finished,
                    updated_at = NOW()
                RETURNING bark_url, telegram_bot_token, telegram_chat_id,
                          feishu_webhook_url, discord_webhook_url, generic_webhook_url,
                          notify_on_needs_you, notify_on_finished
                "#,
                &[
                    &user_id,
                    &settings.bark_url,
                    &settings.telegram_bot_token,
                    &settings.telegram_chat_id,
                    &settings.feishu_webhook_url,
                    &settings.discord_webhook_url,
                    &settings.generic_webhook_url,
                    &settings.notify_on_needs_you,
                    &settings.notify_on_finished,
                ],
            )
            .await?;
        Ok(NotificationSettingsRecord {
            bark_url: row.get(0),
            telegram_bot_token: row.get(1),
            telegram_chat_id: row.get(2),
            feishu_webhook_url: row.get(3),
            discord_webhook_url: row.get(4),
            generic_webhook_url: row.get(5),
            notify_on_needs_you: row.get(6),
            notify_on_finished: row.get(7),
        })
    }

    pub async fn get_device_by_id(&self, device_id: Uuid) -> Result<Option<DeviceRecord>> {
        let client = self.client().await?;
        let row = client
            .query_opt(
                r#"
                SELECT id, user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                       endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                FROM devices
                WHERE id = $1
                "#,
                &[&device_id],
            )
            .await?;
        Ok(row.as_ref().map(map_device_row))
    }

    pub async fn get_user_quota_mbps(&self, user_id: Uuid) -> Result<u32> {
        let client = self.client().await?;
        let row = client
            .query_one(
                "SELECT bandwidth_quota_mbps FROM users WHERE id = $1",
                &[&user_id],
            )
            .await?;
        let q: i32 = row.get(0);
        Ok(q.max(1) as u32)
    }

    pub async fn set_device_online(&self, device_id: Uuid, is_online: bool) -> Result<()> {
        let client = self.client().await?;
        client
            .execute(
                "UPDATE devices SET is_online = $2, last_seen_at = NOW() WHERE id = $1",
                &[&device_id, &is_online],
            )
            .await?;
        Ok(())
    }

    pub async fn update_device_endpoints(&self, device_id: Uuid, endpoints: &Value) -> Result<()> {
        let client = self.client().await?;
        client
            .execute(
                "UPDATE devices SET endpoints_json = $2, last_seen_at = NOW() WHERE id = $1",
                &[&device_id, endpoints],
            )
            .await?;
        Ok(())
    }

    pub async fn list_active_relays(&self) -> Result<Vec<RelayNodeRecord>> {
        let client = self.client().await?;
        let rows = client
            .query(
                r#"
                SELECT id, region, mode, endpoint, node_pubkey, priority
                FROM relay_nodes
                WHERE is_active = TRUE
                ORDER BY priority ASC
                "#,
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| RelayNodeRecord {
                id: r.get(0),
                region: r.get(1),
                mode: r.get(2),
                endpoint: r.get(3),
                node_pubkey: r.get(4),
                priority: r.get(5),
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_device_auth_request(
        &self,
        device_name: &str,
        role: &str,
        platform: &str,
        ed25519_pubkey: &str,
        x25519_pubkey: &str,
        endpoints: &Value,
        herdr_core_id: &str,
        herdr_version: &str,
        ttl_secs: i64,
    ) -> Result<(String, String, DateTime<Utc>)> {
        validate_device_inputs(device_name, role, platform, ed25519_pubkey, x25519_pubkey)?;
        let client = self.client().await?;
        let device_code = format!("dev_{}_{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let user_code = generate_user_code();
        let expires_at = Utc::now() + Duration::seconds(ttl_secs.max(60));
        let trimmed_name = device_name.trim();
        client
            .execute(
                r#"
                INSERT INTO device_auth_requests (
                    device_code, user_code, device_name, role, platform,
                    ed25519_pubkey, x25519_pubkey, endpoints_json,
                    herdr_core_id, herdr_version, status, expires_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending', $11)
                "#,
                &[
                    &device_code,
                    &user_code,
                    &trimmed_name,
                    &role,
                    &platform,
                    &ed25519_pubkey,
                    &x25519_pubkey,
                    endpoints,
                    &herdr_core_id,
                    &herdr_version,
                    &expires_at,
                ],
            )
            .await?;
        Ok((device_code, user_code, expires_at))
    }

    pub async fn get_device_auth_info(
        &self,
        user_code: &str,
    ) -> Result<Option<DeviceAuthPublicInfo>> {
        let client = self.client().await?;
        let normalized = normalize_user_code(user_code);
        if normalized.is_empty() {
            return Ok(None);
        }
        let row = client
            .query_opt(
                r#"
                SELECT user_code, device_name, role, platform, status, expires_at
                FROM device_auth_requests
                WHERE user_code = $1
                "#,
                &[&normalized],
            )
            .await?;
        Ok(row.map(|r| {
            let status: String = r.get(4);
            let expires_at: DateTime<Utc> = r.get(5);
            let effective_status = if status == "pending" && expires_at < Utc::now() {
                "expired".to_string()
            } else {
                status
            };
            DeviceAuthPublicInfo {
                user_code: r.get(0),
                device_name: r.get(1),
                role: r.get(2),
                platform: r.get(3),
                status: effective_status,
                expires_at,
            }
        }))
    }

    pub async fn approve_device_auth(
        &self,
        user_id: Uuid,
        user_code: &str,
        device_name_override: Option<&str>,
    ) -> Result<DeviceRecord> {
        let normalized = normalize_user_code(user_code);
        let mut tx_guard = self.acquire_tx_client().await?;
        let tx = tx_guard.as_mut().transaction().await?;

        let row = tx
            .query_opt(
                r#"
                SELECT device_code, device_name, role, platform,
                       ed25519_pubkey, x25519_pubkey, endpoints_json,
                       herdr_core_id, herdr_version, status, expires_at
                FROM device_auth_requests
                WHERE user_code = $1
                FOR UPDATE
                "#,
                &[&normalized],
            )
            .await?
            .ok_or_else(|| anyhow!("device authorization code not found"))?;

        let device_code: String = row.get(0);
        let mut device_name: String = row.get(1);
        let role: String = row.get(2);
        let platform: String = row.get(3);
        let ed25519_pubkey: String = row.get(4);
        let x25519_pubkey: String = row.get(5);
        let endpoints_json: Value = row.get(6);
        let herdr_core_id: String = row.get(7);
        let herdr_version: String = row.get(8);
        let status: String = row.get(9);
        let expires_at: DateTime<Utc> = row.get(10);

        if status != "pending" {
            bail!("device authorization code is already {status}");
        }
        if expires_at < Utc::now() {
            let _ = tx
                .execute(
                    "UPDATE device_auth_requests SET status = 'expired' WHERE device_code = $1",
                    &[&device_code],
                )
                .await;
            let _ = tx.commit().await;
            bail!("device authorization code expired");
        }

        if let Some(override_name) = device_name_override.map(str::trim).filter(|s| !s.is_empty()) {
            device_name = override_name.to_string();
        }
        validate_device_inputs(&device_name, &role, &platform, &ed25519_pubkey, &x25519_pubkey)?;

        let core_opt = Some(herdr_core_id.as_str()).filter(|s| !s.is_empty());
        let ver_opt = Some(herdr_version.as_str()).filter(|s| !s.is_empty());
        let dev_row = tx
            .query_one(
                r#"
                INSERT INTO devices (
                    user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                    endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, FALSE, NOW())
                ON CONFLICT (user_id, ed25519_pubkey) DO UPDATE
                SET name = EXCLUDED.name,
                    role = EXCLUDED.role,
                    platform = EXCLUDED.platform,
                    x25519_pubkey = EXCLUDED.x25519_pubkey,
                    endpoints_json = EXCLUDED.endpoints_json,
                    herdr_core_id = EXCLUDED.herdr_core_id,
                    herdr_version = EXCLUDED.herdr_version,
                    last_seen_at = NOW()
                RETURNING id, user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                          endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                "#,
                &[
                    &user_id,
                    &device_name,
                    &role,
                    &platform,
                    &ed25519_pubkey,
                    &x25519_pubkey,
                    &endpoints_json,
                    &core_opt,
                    &ver_opt,
                ],
            )
            .await?;
        let device = map_device_row(&dev_row);

        let token = generate_raw_session_token();
        let token_sha256 = sha256_hex(&token);
        let session_exp = Utc::now() + Duration::days(30);
        let label: String = format!("device:{}", device.name).chars().take(64).collect();
        let scope = if device.role == "daemon" {
            "daemon_host"
        } else {
            "full"
        };
        tx.execute(
            r#"
            INSERT INTO auth_sessions (token_sha256, user_id, device_id, scope, client_label, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            &[&token_sha256, &user_id, &device.id, &scope, &label, &session_exp],
        )
        .await?;

        let sealed_token = seal_opaque_secret(&self.inner.token_wrap_key, &token)?;
        tx.execute(
            r#"
            UPDATE device_auth_requests
            SET status = 'approved',
                approved_user_id = $2,
                issued_session_token = $3,
                issued_device_id = $4,
                device_name = $5
            WHERE device_code = $1
            "#,
            &[
                &device_code,
                &user_id,
                &sealed_token,
                &device.id,
                &device.name,
            ],
        )
        .await?;

        tx.commit().await?;
        Ok(device)
    }

    pub async fn poll_device_auth(&self, device_code: &str) -> Result<DeviceAuthPollState> {
        let client = self.client().await?;

        // Atomically consume an approved & unexpired token first so concurrent polls cannot double-read it.
        if let Some(consumed_row) = client
            .query_opt(
                r#"
                WITH prev AS (
                    SELECT device_code, approved_user_id, issued_session_token, issued_device_id
                    FROM device_auth_requests
                    WHERE device_code = $1
                      AND status = 'approved'
                      AND expires_at > NOW()
                      AND issued_session_token <> ''
                    FOR UPDATE
                )
                UPDATE device_auth_requests d
                SET status = 'consumed',
                    issued_session_token = ''
                FROM prev
                WHERE d.device_code = prev.device_code
                RETURNING prev.approved_user_id, prev.issued_session_token, prev.issued_device_id
                "#,
                &[&device_code],
            )
            .await?
        {
            let approved_user_id: Option<Uuid> = consumed_row.get(0);
            let sealed_token: String = consumed_row.get(1);
            let issued_device_id: Option<Uuid> = consumed_row.get(2);
            if let (Some(user_id), Some(device_id)) = (approved_user_id, issued_device_id) {
                let session_token = open_opaque_secret(&self.inner.token_wrap_key, &sealed_token)?;
                let user_row = client
                    .query_one(
                        "SELECT id, email, handle, display_name, is_admin, bandwidth_quota_mbps, created_at FROM users WHERE id = $1",
                        &[&user_id],
                    )
                    .await?;
                let user = UserRecord {
                    id: user_row.get(0),
                    email: user_row.get(1),
                    handle: user_row.get(2),
                    display_name: user_row.get(3),
                    is_admin: user_row.get(4),
                    bandwidth_quota_mbps: user_row.get(5),
                    created_at: user_row.get(6),
                };
                let device = self
                    .get_device_by_id(device_id)
                    .await?
                    .ok_or_else(|| anyhow!("issued device not found"))?;
                return Ok(DeviceAuthPollState::Approved {
                    session_token,
                    user,
                    device,
                });
            }
        }

        let row = client
            .query_opt(
                r#"
                SELECT status, expires_at
                FROM device_auth_requests
                WHERE device_code = $1
                "#,
                &[&device_code],
            )
            .await?;

        let Some(row) = row else {
            return Ok(DeviceAuthPollState::Expired);
        };

        let status: String = row.get(0);
        let expires_at: DateTime<Utc> = row.get(1);

        if status == "pending" {
            let remaining = (expires_at - Utc::now()).num_seconds();
            if remaining <= 0 {
                let _ = client
                    .execute(
                        "UPDATE device_auth_requests SET status = 'expired' WHERE device_code = $1",
                        &[&device_code],
                    )
                    .await;
                return Ok(DeviceAuthPollState::Expired);
            }
            return Ok(DeviceAuthPollState::Pending {
                expires_in_secs: remaining,
            });
        }

        Ok(DeviceAuthPollState::Expired)
    }
}

fn validate_email(email: &str) -> Result<()> {
    let Some((local, domain)) = email.split_once('@') else {
        bail!("invalid email address");
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || email.len() > 254
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        bail!("invalid email address");
    }
    Ok(())
}

fn validate_handle(handle: &str) -> Result<()> {
    if handle.len() < 2
        || handle.len() > 64
        || handle.contains('@')
        || !handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("handle must be 2..=64 ASCII characters ([a-z0-9._-]) without '@'");
    }
    Ok(())
}

fn validate_device_inputs(
    name: &str,
    role: &str,
    platform: &str,
    ed25519_pubkey: &str,
    x25519_pubkey: &str,
) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        bail!("device name must be 1..=128 characters");
    }
    if !matches!(role, "daemon" | "client" | "hybrid") {
        bail!("invalid device role: {role}");
    }
    if !matches!(
        platform,
        "macos" | "ios" | "ipados" | "iphone_duo" | "linux" | "windows" | "web"
    ) {
        bail!("invalid device platform: {platform}");
    }
    validate_ed25519_pub_b64(ed25519_pubkey)?;
    validate_x25519_pub_b64(x25519_pubkey)?;
    Ok(())
}

fn generate_raw_session_token() -> String {
    let mut raw_bytes = [0u8; 32];
    rand::thread_rng().fill(&mut raw_bytes);
    format!("coda_pat_{}", hex::encode(raw_bytes))
}

async fn purge_expired_with_client(client: &Client) -> Result<()> {
    let _ = client
        .execute("DELETE FROM auth_sessions WHERE expires_at < NOW()", &[])
        .await?;
    let _ = client
        .execute(
            "DELETE FROM pairing_tokens WHERE expires_at < NOW() - INTERVAL '1 hour'",
            &[],
        )
        .await?;
    let _ = client
        .execute(
            "DELETE FROM device_auth_requests WHERE expires_at < NOW() - INTERVAL '1 hour'",
            &[],
        )
        .await?;
    let _ = client
        .execute(
            "DELETE FROM devices WHERE role = 'client' AND is_online = FALSE AND COALESCE(last_seen_at, created_at) < NOW() - INTERVAL '45 days'",
            &[],
        )
        .await?;
    Ok(())
}

fn spawn_expiry_cleanup_task(weak_inner: Weak<DbInner>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(900));
        interval.tick().await;
        loop {
            interval.tick().await;
            let Some(inner) = weak_inner.upgrade() else {
                break;
            };
            let db = Db { inner };
            if let Ok(client) = db.client().await {
                let _ = purge_expired_with_client(&client).await;
            }
        }
    });
}

pub fn normalize_user_code(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .to_uppercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if cleaned.len() == 8 {
        format!("{}-{}", &cleaned[..4], &cleaned[4..])
    } else if raw.contains('-') {
        raw.trim().to_uppercase()
    } else {
        cleaned
    }
}

fn generate_invite_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let suffix: String = (0..8)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect();
    format!("CODA-{suffix}")
}

fn generate_user_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let mut part = |len: usize| -> String {
        (0..len)
            .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
            .collect()
    };
    format!("{}-{}", part(4), part(4))
}

fn map_device_row(row: &tokio_postgres::Row) -> DeviceRecord {
    DeviceRecord {
        id: row.get(0),
        user_id: row.get(1),
        name: row.get(2),
        role: row.get(3),
        platform: row.get(4),
        ed25519_pubkey: row.get(5),
        x25519_pubkey: row.get(6),
        endpoints_json: row.get(7),
        herdr_core_id: row.get(8),
        herdr_version: row.get(9),
        is_online: row.get(10),
        last_seen_at: row.get(11),
    }
}

pub(crate) fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}
