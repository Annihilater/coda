-- Coda Multi-Tenant Control Plane Schema (PostgreSQL 18+)
-- Zero-Knowledge Architecture: Server stores ONLY public keys (Ed25519 / X25519)
-- and routing metadata. Session payloads remain End-to-End Encrypted.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- 1. Users (Multi-tenant accounts for owner & friends)
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT NOT NULL UNIQUE,
    handle TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    bandwidth_quota_mbps INTEGER NOT NULL DEFAULT 25,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_users_email_lower ON users (LOWER(email));

-- 2. Friend Invite Codes (Controlled registration so relay bandwidth stays protected)
CREATE TABLE IF NOT EXISTS invite_codes (
    code TEXT PRIMARY KEY,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    note TEXT NOT NULL DEFAULT '',
    max_uses INTEGER NOT NULL DEFAULT 1,
    used_count INTEGER NOT NULL DEFAULT 0,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT chk_invite_uses CHECK (used_count <= max_uses)
);

-- 3. Auth Sessions (Bearer tokens hashed with SHA-256 at rest)
CREATE TABLE IF NOT EXISTS auth_sessions (
    token_sha256 TEXT PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID REFERENCES devices(id) ON DELETE CASCADE,
    scope TEXT NOT NULL DEFAULT 'full' CHECK (scope IN ('full', 'daemon_host')),
    client_label TEXT NOT NULL DEFAULT 'web',
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_auth_sessions_user ON auth_sessions (user_id);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_expires ON auth_sessions (expires_at);

-- 4. Devices (Isolated per user_id; stores Ed25519 identity & X25519 E2EE public keys)
CREATE TABLE IF NOT EXISTS devices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('daemon', 'client', 'hybrid')),
    platform TEXT NOT NULL CHECK (platform IN ('macos', 'ios', 'ipados', 'iphone_duo', 'linux', 'windows', 'web')),
    ed25519_pubkey TEXT NOT NULL,
    x25519_pubkey TEXT NOT NULL,
    endpoints_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    herdr_core_id TEXT,
    herdr_version TEXT,
    is_online BOOLEAN NOT NULL DEFAULT FALSE,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_devices_user_ed25519 UNIQUE (user_id, ed25519_pubkey)
);

CREATE INDEX IF NOT EXISTS idx_devices_user_online ON devices (user_id, is_online);

-- 5. Short-lived 6-digit Pairing Codes (e.g. `839-204` for 30-second Linux/Mac onboarding)
CREATE TABLE IF NOT EXISTS pairing_tokens (
    code TEXT PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    suggested_name TEXT,
    claimed_device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_pairing_tokens_user ON pairing_tokens (user_id, expires_at);

-- 6. Relay Nodes Directory (Supports both Domestic No-ICP Raw TCP & Overseas CDN WSS)
CREATE TABLE IF NOT EXISTS relay_nodes (
    id TEXT PRIMARY KEY,
    region TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('raw_tcp_no_sni', 'cdn_wss')),
    endpoint TEXT NOT NULL,
    node_pubkey TEXT NOT NULL DEFAULT '',
    priority INTEGER NOT NULL DEFAULT 100,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 7. RFC 8628 Device Authorization Requests (Browser Approval for CLI / Desktop / Mobile)
CREATE TABLE IF NOT EXISTS device_auth_requests (
    device_code TEXT PRIMARY KEY,
    user_code TEXT NOT NULL UNIQUE,
    device_name TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'daemon' CHECK (role IN ('daemon', 'client', 'hybrid')),
    platform TEXT NOT NULL DEFAULT 'linux' CHECK (platform IN ('macos', 'ios', 'ipados', 'iphone_duo', 'linux', 'windows', 'web')),
    ed25519_pubkey TEXT NOT NULL DEFAULT '',
    x25519_pubkey TEXT NOT NULL DEFAULT '',
    endpoints_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    herdr_core_id TEXT NOT NULL DEFAULT '',
    herdr_version TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'consumed', 'expired')),
    approved_user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    issued_session_token TEXT NOT NULL DEFAULT '',
    issued_device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_device_auth_user_code ON device_auth_requests (user_code, expires_at);
CREATE INDEX IF NOT EXISTS idx_device_auth_expires ON device_auth_requests (expires_at);

-- 8. User Offline Alert Notification Webhooks (Zero-Knowledge Metadata-Only Push)
CREATE TABLE IF NOT EXISTS user_notification_settings (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    bark_url TEXT NOT NULL DEFAULT '',
    telegram_bot_token TEXT NOT NULL DEFAULT '',
    telegram_chat_id TEXT NOT NULL DEFAULT '',
    feishu_webhook_url TEXT NOT NULL DEFAULT '',
    discord_webhook_url TEXT NOT NULL DEFAULT '',
    generic_webhook_url TEXT NOT NULL DEFAULT '',
    notify_on_needs_you BOOLEAN NOT NULL DEFAULT TRUE,
    notify_on_finished BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 9. Admin-Issued Passwordless One-Time Reset Links (SHA-256 hashed at rest, 24h TTL)
CREATE TABLE IF NOT EXISTS password_reset_tokens (
    token_sha256 TEXT PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_password_reset_tokens_user ON password_reset_tokens (user_id, expires_at);

-- 10. Server-Level Persistent Key-Value Metadata (e.g. W3C VAPID ES256 keypair)
CREATE TABLE IF NOT EXISTS server_kv (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 11. Standard W3C Web Push Subscriptions (RFC 8292 VAPID + RFC 8291 aes128gcm)
CREATE TABLE IF NOT EXISTS web_push_subscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    endpoint TEXT NOT NULL,
    p256dh_b64url TEXT NOT NULL,
    auth_b64url TEXT NOT NULL,
    user_agent TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT uq_web_push_user_endpoint UNIQUE (user_id, endpoint)
);

CREATE INDEX IF NOT EXISTS idx_web_push_subscriptions_user ON web_push_subscriptions (user_id, created_at);

-- 12. Cross-Device Saved Prompt Snippets
CREATE TABLE IF NOT EXISTS user_prompt_snippets (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    snippets_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Idempotent schema hardening for databases created before these constraints existed.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'devices_ed25519_pubkey_key') THEN
        ALTER TABLE devices DROP CONSTRAINT devices_ed25519_pubkey_key;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'uq_devices_user_ed25519') THEN
        ALTER TABLE devices ADD CONSTRAINT uq_devices_user_ed25519 UNIQUE (user_id, ed25519_pubkey);
    END IF;
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'devices_platform_check') THEN
        ALTER TABLE devices DROP CONSTRAINT devices_platform_check;
    END IF;
    ALTER TABLE devices
        ADD CONSTRAINT devices_platform_check
        CHECK (platform IN ('macos', 'ios', 'ipados', 'iphone_duo', 'linux', 'windows', 'web'));

    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'device_auth_requests_platform_check') THEN
        ALTER TABLE device_auth_requests DROP CONSTRAINT device_auth_requests_platform_check;
    END IF;
    ALTER TABLE device_auth_requests
        ADD CONSTRAINT device_auth_requests_platform_check
        CHECK (platform IN ('macos', 'ios', 'ipados', 'iphone_duo', 'linux', 'windows', 'web'));

    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'auth_sessions' AND column_name = 'id'
    ) THEN
        ALTER TABLE auth_sessions ADD COLUMN id UUID NOT NULL DEFAULT gen_random_uuid();
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'auth_sessions' AND column_name = 'device_id'
    ) THEN
        ALTER TABLE auth_sessions ADD COLUMN device_id UUID REFERENCES devices(id) ON DELETE CASCADE;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'auth_sessions' AND column_name = 'scope'
    ) THEN
        ALTER TABLE auth_sessions ADD COLUMN scope TEXT NOT NULL DEFAULT 'full';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'auth_sessions' AND column_name = 'last_used_at'
    ) THEN
        ALTER TABLE auth_sessions ADD COLUMN last_used_at TIMESTAMPTZ NOT NULL DEFAULT NOW();
    END IF;
END $$;

CREATE UNIQUE INDEX IF NOT EXISTS idx_auth_sessions_id ON auth_sessions (id);
CREATE INDEX IF NOT EXISTS idx_auth_sessions_device ON auth_sessions (device_id);
