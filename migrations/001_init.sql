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
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    client_label TEXT NOT NULL DEFAULT 'web',
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_auth_sessions_user ON auth_sessions (user_id);

-- 4. Devices (Isolated per user_id; stores Ed25519 identity & X25519 E2EE public keys)
CREATE TABLE IF NOT EXISTS devices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('daemon', 'client', 'hybrid')),
    platform TEXT NOT NULL CHECK (platform IN ('macos', 'ios', 'ipados', 'iphone_duo', 'linux', 'web')),
    ed25519_pubkey TEXT NOT NULL UNIQUE,
    x25519_pubkey TEXT NOT NULL,
    endpoints_json JSONB NOT NULL DEFAULT '[]'::jsonb,
    herdr_core_id TEXT,
    herdr_version TEXT,
    is_online BOOLEAN NOT NULL DEFAULT FALSE,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
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
    platform TEXT NOT NULL DEFAULT 'linux',
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
