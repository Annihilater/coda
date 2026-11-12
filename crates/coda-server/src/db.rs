use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_postgres::{Client, NoTls};
use tracing::{error, info};
use uuid::Uuid;

const INIT_SQL: &str = include_str!("../../../migrations/001_init.sql");

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

#[derive(Clone)]
pub struct Db {
    client: Arc<Client>,
}

impl Db {
    pub async fn connect_and_migrate(database_url: &str) -> Result<Self> {
        let (client, connection) = tokio_postgres::connect(database_url, NoTls)
            .await
            .with_context(|| format!("failed connecting to PostgreSQL: {database_url}"))?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("PostgreSQL connection error: {e}");
            }
        });

        client
            .batch_execute(INIT_SQL)
            .await
            .context("failed applying migrations/001_init.sql")?;

        // Reset stale online flags from any unclean previous restart
        client
            .execute("UPDATE devices SET is_online = FALSE WHERE is_online = TRUE", &[])
            .await?;

        info!("PostgreSQL connected and migrations verified");
        Ok(Self {
            client: Arc::new(client),
        })
    }

    pub async fn seed_default_relays(
        &self,
        raw_tcp_endpoint: &str,
        cdn_wss_endpoint: &str,
        server_pubkey: &str,
    ) -> Result<()> {
        self.client
            .execute(
                r#"
                INSERT INTO relay_nodes (id, region, mode, endpoint, node_pubkey, priority, is_active)
                VALUES ('cn-direct-tcp', 'CN-BGP (No-ICP Raw TCP)', 'raw_tcp_no_sni', $1, $2, 10, TRUE)
                ON CONFLICT (id) DO UPDATE
                SET endpoint = EXCLUDED.endpoint, node_pubkey = EXCLUDED.node_pubkey, updated_at = NOW()
                "#,
                &[&raw_tcp_endpoint, &server_pubkey],
            )
            .await?;

        self.client
            .execute(
                r#"
                INSERT INTO relay_nodes (id, region, mode, endpoint, node_pubkey, priority, is_active)
                VALUES ('global-cdn-wss', 'Global Edge CDN (WSS 443)', 'cdn_wss', $1, $2, 20, TRUE)
                ON CONFLICT (id) DO UPDATE
                SET endpoint = EXCLUDED.endpoint, node_pubkey = EXCLUDED.node_pubkey, updated_at = NOW()
                "#,
                &[&cdn_wss_endpoint, &server_pubkey],
            )
            .await?;
        Ok(())
    }

    /// Registers a new user.
    /// - If `users` table is completely empty, this user becomes the Owner/Admin (`is_admin = true`) without needing an invite code.
    /// - All subsequent users (friends) MUST supply a valid, non-exhausted `invite_code`.
    pub async fn register_user(
        &self,
        email: &str,
        handle: &str,
        display_name: &str,
        password: &str,
        invite_code: Option<&str>,
    ) -> Result<(UserRecord, String)> {
        let email = email.trim().to_lowercase();
        let handle = handle.trim().to_lowercase();
        if email.is_empty() || handle.is_empty() || password.len() < 6 {
            bail!("email, handle, and password (>= 6 chars) are required");
        }

        let salt = SaltString::generate(&mut OsRng);
        let password_hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow!("password hash error: {e}"))?
            .to_string();

        let count_row = self
            .client
            .query_one("SELECT COUNT(*)::BIGINT FROM users", &[])
            .await?;
        let user_count: i64 = count_row.get(0);
        let is_first_admin = user_count == 0;

        if !is_first_admin {
            let code = invite_code
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow!("invite_code is required to register on this server"))?;

            let updated = self
                .client
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
        }

        let quota_mbps: i32 = if is_first_admin { 100 } else { 25 };
        let row = self
            .client
            .query_one(
                r#"
                INSERT INTO users (email, handle, display_name, password_hash, is_admin, bandwidth_quota_mbps, last_login_at)
                VALUES ($1, $2, $3, $4, $5, $6, NOW())
                RETURNING id, email, handle, display_name, is_admin, bandwidth_quota_mbps, created_at
                "#,
                &[&email, &handle, &display_name, &password_hash, &is_first_admin, &quota_mbps],
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

        let token = self.issue_session_token(user.id, "register").await?;
        Ok((user, token))
    }

    pub async fn login_user(
        &self,
        email_or_handle: &str,
        password: &str,
        client_label: &str,
    ) -> Result<(UserRecord, String)> {
        let key = email_or_handle.trim().to_lowercase();
        let row = self
            .client
            .query_opt(
                r#"
                SELECT id, email, handle, display_name, password_hash, is_admin, bandwidth_quota_mbps, created_at
                FROM users
                WHERE LOWER(email) = $1 OR LOWER(handle) = $1
                "#,
                &[&key],
            )
            .await?
            .ok_or_else(|| anyhow!("invalid credentials"))?;

        let pw_hash_str: String = row.get(4);
        let parsed_hash =
            PasswordHash::new(&pw_hash_str).map_err(|_| anyhow!("corrupted password hash"))?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| anyhow!("invalid credentials"))?;

        let user = UserRecord {
            id: row.get(0),
            email: row.get(1),
            handle: row.get(2),
            display_name: row.get(3),
            is_admin: row.get(5),
            bandwidth_quota_mbps: row.get(6),
            created_at: row.get(7),
        };

        self.client
            .execute("UPDATE users SET last_login_at = NOW() WHERE id = $1", &[&user.id])
            .await?;

        let token = self.issue_session_token(user.id, client_label).await?;
        Ok((user, token))
    }

    async fn issue_session_token(&self, user_id: Uuid, client_label: &str) -> Result<String> {
        let mut raw_bytes = [0u8; 32];
        rand::thread_rng().fill(&mut raw_bytes);
        let raw_token = format!("coda_pat_{}", hex::encode(raw_bytes));
        let token_sha256 = sha256_hex(&raw_token);
        let expires_at = Utc::now() + Duration::days(30);

        self.client
            .execute(
                r#"
                INSERT INTO auth_sessions (token_sha256, user_id, client_label, expires_at)
                VALUES ($1, $2, $3, $4)
                "#,
                &[&token_sha256, &user_id, &client_label, &expires_at],
            )
            .await?;

        Ok(raw_token)
    }

    pub async fn authenticate_token(&self, raw_token: &str) -> Result<UserRecord> {
        let token_sha256 = sha256_hex(raw_token.trim());
        let row = self
            .client
            .query_opt(
                r#"
                SELECT u.id, u.email, u.handle, u.display_name, u.is_admin, u.bandwidth_quota_mbps, u.created_at
                FROM auth_sessions s
                JOIN users u ON u.id = s.user_id
                WHERE s.token_sha256 = $1 AND s.expires_at > NOW()
                "#,
                &[&token_sha256],
            )
            .await?
            .ok_or_else(|| anyhow!("unauthorized or expired session token"))?;

        Ok(UserRecord {
            id: row.get(0),
            email: row.get(1),
            handle: row.get(2),
            display_name: row.get(3),
            is_admin: row.get(4),
            bandwidth_quota_mbps: row.get(5),
            created_at: row.get(6),
        })
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
        let suffix: u32 = rand::thread_rng().gen_range(100_000..999_999);
        let code = format!("CODA-{suffix}");
        let expires_at = Some(Utc::now() + Duration::days(valid_days.max(1)));
        let max_uses = max_uses.clamp(1, 500);

        let row = self
            .client
            .query_one(
                r#"
                INSERT INTO invite_codes (code, created_by, note, max_uses, expires_at)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING code, note, max_uses, used_count, expires_at, created_at
                "#,
                &[&code, &creator.id, &note, &max_uses, &expires_at],
            )
            .await?;

        Ok(InviteCodeRecord {
            code: row.get(0),
            note: row.get(1),
            max_uses: row.get(2),
            used_count: row.get(3),
            expires_at: row.get(4),
            created_at: row.get(5),
        })
    }

    pub async fn list_invite_codes(&self, user: &UserRecord) -> Result<Vec<InviteCodeRecord>> {
        if !user.is_admin {
            return Ok(vec![]);
        }
        let rows = self
            .client
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

    /// Generates a short 6-digit pairing code (`839-204`) valid for 10 minutes.
    pub async fn create_pairing_token(
        &self,
        user_id: Uuid,
        suggested_name: Option<&str>,
    ) -> Result<(String, DateTime<Utc>)> {
        let a: u16 = rand::thread_rng().gen_range(100..999);
        let b: u16 = rand::thread_rng().gen_range(100..999);
        let code = format!("{a}-{b}");
        let expires_at = Utc::now() + Duration::minutes(10);

        self.client
            .execute(
                r#"
                INSERT INTO pairing_tokens (code, user_id, suggested_name, expires_at)
                VALUES ($1, $2, $3, $4)
                "#,
                &[&code, &user_id, &suggested_name, &expires_at],
            )
            .await?;

        Ok((code, expires_at))
    }

    /// Daemon claims a 6-digit pairing code and registers its Ed25519 + X25519 public keys.
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
        let normalized = code.trim();
        let token_row = self
            .client
            .query_opt(
                r#"
                SELECT user_id, suggested_name
                FROM pairing_tokens
                WHERE code = $1 AND claimed_device_id IS NULL AND expires_at > NOW()
                "#,
                &[&normalized],
            )
            .await?
            .ok_or_else(|| anyhow!("invalid, expired, or already-claimed pairing code"))?;

        let user_id: Uuid = token_row.get(0);
        let suggested: Option<String> = token_row.get(1);
        let final_name = suggested
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| name.to_string());

        let dev = self
            .upsert_device(
                user_id,
                &final_name,
                "daemon",
                platform,
                ed25519_pubkey,
                x25519_pubkey,
                endpoints_json,
                herdr_core_id,
                herdr_version,
            )
            .await?;

        self.client
            .execute(
                "UPDATE pairing_tokens SET claimed_device_id = $1 WHERE code = $2",
                &[&dev.id, &normalized],
            )
            .await?;

        let urow = self
            .client
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
        let row = self
            .client
            .query_one(
                r#"
                INSERT INTO devices (
                    user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                    endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, FALSE, NOW())
                ON CONFLICT (ed25519_pubkey) DO UPDATE
                SET name = EXCLUDED.name,
                    x25519_pubkey = EXCLUDED.x25519_pubkey,
                    endpoints_json = EXCLUDED.endpoints_json,
                    herdr_core_id = EXCLUDED.herdr_core_id,
                    herdr_version = EXCLUDED.herdr_version,
                    last_seen_at = NOW()
                WHERE devices.user_id = EXCLUDED.user_id
                RETURNING id, user_id, name, role, platform, ed25519_pubkey, x25519_pubkey,
                          endpoints_json, herdr_core_id, herdr_version, is_online, last_seen_at
                "#,
                &[
                    &user_id,
                    &name,
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
            .context("device registration failed or public key belongs to another user")?;

        Ok(map_device_row(&row))
    }

    /// Strict tenant-isolated device listing (`WHERE user_id = $1`).
    pub async fn list_user_devices(&self, user_id: Uuid) -> Result<Vec<DeviceRecord>> {
        let rows = self
            .client
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

    pub async fn get_device_by_id(&self, device_id: Uuid) -> Result<Option<DeviceRecord>> {
        let row = self
            .client
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
        let row = self
            .client
            .query_one(
                "SELECT bandwidth_quota_mbps FROM users WHERE id = $1",
                &[&user_id],
            )
            .await?;
        let q: i32 = row.get(0);
        Ok(q.max(1) as u32)
    }

    pub async fn set_device_online(&self, device_id: Uuid, is_online: bool) -> Result<()> {
        self.client
            .execute(
                "UPDATE devices SET is_online = $2, last_seen_at = NOW() WHERE id = $1",
                &[&device_id, &is_online],
            )
            .await?;
        Ok(())
    }

    pub async fn update_device_endpoints(&self, device_id: Uuid, endpoints: &Value) -> Result<()> {
        self.client
            .execute(
                "UPDATE devices SET endpoints_json = $2, last_seen_at = NOW() WHERE id = $1",
                &[&device_id, endpoints],
            )
            .await?;
        Ok(())
    }

    pub async fn list_active_relays(&self) -> Result<Vec<RelayNodeRecord>> {
        let rows = self
            .client
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

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}
