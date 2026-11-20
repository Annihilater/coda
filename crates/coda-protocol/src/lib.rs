//! Coda Wire Protocol, Zero-Knowledge E2EE (`X25519 + HKDF-SHA256 + ChaCha20Poly1305`),
//! `zstd` Adaptive Compression, Stateless HMAC Relay Tickets, and Dual-Lane QoS Framing.

use std::io::{Cursor, Read};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret as X25519Secret};

type HmacSha256 = Hmac<Sha256>;

/// Minimum plaintext byte size before `zstd` compression kicks in.
const ZSTD_COMPRESS_THRESHOLD: usize = 256;
/// Maximum raw TCP frame size (8 MiB) to prevent memory exhaustion on stateless relays.
pub const MAX_WIRE_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// Maximum permitted pre-authentication handshake frame size (16 KiB) to prevent pre-auth OOM.
pub const MAX_AUTH_FRAME_BYTES: usize = 16 * 1024;
/// Maximum permitted decompressed E2EE payload size (16 MiB) to prevent zstd zip-bombs.
pub const MAX_DECOMPRESSED_BYTES: usize = MAX_WIRE_FRAME_BYTES * 2;
/// Maximum permitted base64 length for an encoded wire/compressed blob.
const MAX_B64_PAYLOAD_LEN: usize = (MAX_WIRE_FRAME_BYTES * 4) / 3 + 256;
/// Length in bytes of per-session relay AEAD/HKDF salt nonces.
pub const RELAY_SESSION_NONCE_LEN: usize = 16;
/// Default clock-skew / freshness tolerance for P2P and Relay session handshakes (2 minutes).
pub const HANDSHAKE_MAX_SKEW_MS: u64 = 120_000;

/// Overwrites sensitive byte slice in place with zeroes using volatile writes.
#[inline]
pub fn zeroize_bytes(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[inline]
fn zeroize_string(s: &mut String) {
    // SAFETY: writing ASCII NUL bytes keeps valid UTF-8.
    unsafe {
        zeroize_bytes(s.as_bytes_mut());
    }
    s.clear();
}

fn default_true() -> bool {
    true
}

/// Returns current Unix epoch milliseconds.
pub fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Checks whether a peer-supplied handshake timestamp falls within `max_skew_ms` of `now_ms`.
pub fn is_timestamp_fresh(ts_ms: u64, now_ms: u64, max_skew_ms: u64) -> bool {
    now_ms.abs_diff(ts_ms) <= max_skew_ms
}

/// Derives a stable Ed25519 server public key deterministically from `relay_secret` so
/// coordinator restarts never mutate `server_pubkey`.
pub fn deterministic_server_pubkey_b64(relay_secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"coda-server-node-identity-v1:");
    hasher.update(relay_secret);
    let seed: [u8; 32] = hasher.finalize().into();
    let signing = SigningKey::from_bytes(&seed);
    B64.encode(signing.verifying_key().to_bytes())
}

/// Validates and decodes a canonical 32-byte Base64 public/secret key.
pub fn decode_b64_key_32(b64_str: &str, label: &str) -> Result<[u8; 32]> {
    let raw = B64
        .decode(b64_str)
        .with_context(|| format!("invalid base64 for {label}"))?;
    raw.try_into()
        .map_err(|_| anyhow!("invalid {label} length (expected 32 bytes)"))
}

/// Validates a Base64 Ed25519 verifying key.
pub fn validate_ed25519_pub_b64(b64_str: &str) -> Result<VerifyingKey> {
    let bytes = decode_b64_key_32(b64_str, "ed25519 public key")?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| anyhow!("invalid ed25519 public key point"))
}

/// Validates a Base64 X25519 public key (32 bytes, non-zero).
pub fn validate_x25519_pub_b64(b64_str: &str) -> Result<X25519Public> {
    let bytes = decode_b64_key_32(b64_str, "x25519 public key")?;
    if bytes == [0u8; 32] {
        bail!("all-zero x25519 public key rejected");
    }
    Ok(X25519Public::from(bytes))
}

/// Trusted device registry entry returned by `coda-server` for peer pinning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPeerEntry {
    pub device_id: Uuid,
    pub name: String,
    pub role: String,
    pub ed25519_pub_b64: String,
    pub x25519_pub_b64: String,
}

// ============================================================================
// 1. Device Cryptographic Identity & Zero-Knowledge E2EE Session
// ============================================================================

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredDeviceIdentity {
    pub device_id: Option<Uuid>,
    pub name: String,
    pub ed25519_secret_b64: String,
    pub ed25519_public_b64: String,
    pub x25519_secret_b64: String,
    pub x25519_public_b64: String,
}

impl std::fmt::Debug for StoredDeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredDeviceIdentity")
            .field("device_id", &self.device_id)
            .field("name", &self.name)
            .field("ed25519_public_b64", &self.ed25519_public_b64)
            .field("x25519_public_b64", &self.x25519_public_b64)
            .field("secrets", &"[REDACTED]")
            .finish()
    }
}

impl Drop for StoredDeviceIdentity {
    fn drop(&mut self) {
        zeroize_string(&mut self.ed25519_secret_b64);
        zeroize_string(&mut self.x25519_secret_b64);
    }
}

impl StoredDeviceIdentity {
    pub fn generate(name: impl Into<String>) -> Self {
        let signing = SigningKey::generate(&mut OsRng);
        let verifying = signing.verifying_key();
        let dh_secret = X25519Secret::random_from_rng(OsRng);
        let dh_public = X25519Public::from(&dh_secret);

        Self {
            device_id: None,
            name: name.into(),
            ed25519_secret_b64: B64.encode(signing.to_bytes()),
            ed25519_public_b64: B64.encode(verifying.to_bytes()),
            x25519_secret_b64: B64.encode(dh_secret.to_bytes()),
            x25519_public_b64: B64.encode(dh_public.as_bytes()),
        }
    }

    pub fn sign(&self, message: &[u8]) -> Result<String> {
        let bytes = B64.decode(&self.ed25519_secret_b64)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow!("invalid ed25519 secret key length"))?;
        let signing = SigningKey::from_bytes(&arr);
        let sig = signing.sign(message);
        Ok(B64.encode(sig.to_bytes()))
    }

    pub fn verify_signature(pubkey_b64: &str, message: &[u8], sig_b64: &str) -> Result<()> {
        let pk_bytes: [u8; 32] = B64
            .decode(pubkey_b64)?
            .try_into()
            .map_err(|_| anyhow!("invalid ed25519 public key length"))?;
        let sig_bytes: [u8; 64] = B64
            .decode(sig_b64)?
            .try_into()
            .map_err(|_| anyhow!("invalid ed25519 signature length"))?;
        let vk = VerifyingKey::from_bytes(&pk_bytes)?;
        let sig = Signature::from_bytes(&sig_bytes);
        vk.verify(message, &sig)
            .context("ed25519 signature verification failed")
    }

    pub fn x25519_secret(&self) -> Result<X25519Secret> {
        let arr = decode_b64_key_32(&self.x25519_secret_b64, "x25519 secret")?;
        Ok(X25519Secret::from(arr))
    }

    /// Generates an ephemeral X25519 secret/public pair encoded as Base64.
    pub fn generate_ephemeral_x25519() -> (X25519Secret, String) {
        let secret = X25519Secret::random_from_rng(OsRng);
        let public = X25519Public::from(&secret);
        (secret, B64.encode(public.as_bytes()))
    }
}

/// 64-bit sliding anti-replay window.
/// Call `check(seq)` before AEAD decryption and `commit(seq)` ONLY after AEAD succeeds.
#[derive(Debug, Clone, Default)]
pub struct ReplayWindow {
    highest_seq: u64,
    bitmap: u64,
}

impl ReplayWindow {
    /// Checks whether `seq` is fresh without mutating window state.
    pub fn check(&self, seq: u64) -> Result<()> {
        if seq == 0 {
            bail!("invalid E2EE sequence number seq=0");
        }
        if self.highest_seq > 0 && seq <= self.highest_seq {
            let diff = self.highest_seq - seq;
            if diff >= 64 {
                bail!(
                    "stale E2EE frame seq={} below replay window highest={}",
                    seq,
                    self.highest_seq
                );
            }
            if (self.bitmap & (1u64 << diff)) != 0 {
                bail!("duplicate E2EE frame seq={}", seq);
            }
        }
        Ok(())
    }

    /// Records `seq` as seen after AEAD authentication succeeds.
    pub fn commit(&mut self, seq: u64) {
        if self.highest_seq == 0 {
            self.highest_seq = seq;
            self.bitmap = 1;
        } else if seq > self.highest_seq {
            let shift = seq - self.highest_seq;
            self.bitmap = if shift >= 64 {
                1
            } else {
                (self.bitmap << shift) | 1
            };
            self.highest_seq = seq;
        } else {
            let diff = self.highest_seq - seq;
            if diff < 64 {
                self.bitmap |= 1u64 << diff;
            }
        }
    }
}

/// Signed relay session authentication block carried when establishing or resetting a Relay v2 session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySessionAuth {
    pub session_nonce_b64: String,
    pub init_ts_ms: u64,
    pub sender_ed25519_pub_b64: String,
    pub sender_x25519_pub_b64: String,
    #[serde(default = "default_true")]
    pub supports_zstd: bool,
    pub signature_b64: String,
}

/// Canonical signing payload for `RelaySessionAuth` v2 (legacy format without `supports_zstd`).
pub fn relay_session_auth_signing_bytes(
    client_device_id: Uuid,
    daemon_device_id: Uuid,
    sender_x25519_pub_b64: &str,
    session_nonce_b64: &str,
    init_ts_ms: u64,
) -> Vec<u8> {
    format!(
        "relay-sess-v2:{}:{}:{}:{}:{}",
        client_device_id.hyphenated(),
        daemon_device_id.hyphenated(),
        sender_x25519_pub_b64,
        session_nonce_b64,
        init_ts_ms
    )
    .into_bytes()
}

/// Canonical signing payload for `RelaySessionAuth` v2 binding `supports_zstd` against relay tampering.
pub fn relay_session_auth_signing_bytes_with_zstd(
    client_device_id: Uuid,
    daemon_device_id: Uuid,
    sender_x25519_pub_b64: &str,
    session_nonce_b64: &str,
    init_ts_ms: u64,
    supports_zstd: bool,
) -> Vec<u8> {
    format!(
        "relay-sess-v2:{}:{}:{}:{}:{}:zstd={}",
        client_device_id.hyphenated(),
        daemon_device_id.hyphenated(),
        sender_x25519_pub_b64,
        session_nonce_b64,
        init_ts_ms,
        if supports_zstd { 1 } else { 0 }
    )
    .into_bytes()
}

/// Canonical signing payload for `DirectP2pHello` v2 (legacy format without `supports_zstd`).
pub fn p2p_hello_v2_signing_bytes(
    client_device_id: Uuid,
    target_device_id: Uuid,
    client_static_x25519_pub_b64: &str,
    client_eph_x25519_pub_b64: &str,
    timestamp_ms: u64,
) -> Vec<u8> {
    format!(
        "p2p-hello-v2:{}:{}:{}:{}:{}",
        client_device_id.hyphenated(),
        target_device_id.hyphenated(),
        client_static_x25519_pub_b64,
        client_eph_x25519_pub_b64,
        timestamp_ms
    )
    .into_bytes()
}

/// Canonical signing payload for `DirectP2pHello` v2 binding `supports_zstd` against MITM tampering.
pub fn p2p_hello_v2_signing_bytes_with_zstd(
    client_device_id: Uuid,
    target_device_id: Uuid,
    client_static_x25519_pub_b64: &str,
    client_eph_x25519_pub_b64: &str,
    timestamp_ms: u64,
    supports_zstd: bool,
) -> Vec<u8> {
    format!(
        "p2p-hello-v2:{}:{}:{}:{}:{}:zstd={}",
        client_device_id.hyphenated(),
        target_device_id.hyphenated(),
        client_static_x25519_pub_b64,
        client_eph_x25519_pub_b64,
        timestamp_ms,
        if supports_zstd { 1 } else { 0 }
    )
    .into_bytes()
}

/// Canonical signing payload for Ed25519-authenticated Relay Ticket self-renewal (`POST /api/v1/relay/renew-ticket`).
pub fn relay_ticket_renew_signing_bytes(device_id: Uuid, timestamp_ms: u64) -> Vec<u8> {
    format!("coda-renew-ticket-v1:{}:{}", device_id.hyphenated(), timestamp_ms).into_bytes()
}

/// Canonical signing payload for `DirectP2pAck` v2.
pub fn p2p_ack_v2_signing_bytes(
    client_device_id: Uuid,
    daemon_device_id: Uuid,
    client_eph_x25519_pub_b64: &str,
    daemon_eph_x25519_pub_b64: &str,
    timestamp_ms: u64,
) -> Vec<u8> {
    format!(
        "p2p-ack-v2:{}:{}:{}:{}:{}",
        client_device_id.hyphenated(),
        daemon_device_id.hyphenated(),
        client_eph_x25519_pub_b64,
        daemon_eph_x25519_pub_b64,
        timestamp_ms
    )
    .into_bytes()
}

impl RelaySessionAuth {
    /// Creates a newly signed `RelaySessionAuth` with a random 16-byte nonce and signed `supports_zstd`.
    pub fn create(
        identity: &StoredDeviceIdentity,
        dst_daemon_id: Uuid,
        supports_zstd: bool,
        now_ms: u64,
    ) -> Result<Self> {
        let client_id = identity
            .device_id
            .ok_or_else(|| anyhow!("StoredDeviceIdentity missing device_id"))?;
        let mut nonce = [0u8; RELAY_SESSION_NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let session_nonce_b64 = B64.encode(nonce);
        let payload = relay_session_auth_signing_bytes_with_zstd(
            client_id,
            dst_daemon_id,
            &identity.x25519_public_b64,
            &session_nonce_b64,
            now_ms,
            supports_zstd,
        );
        let signature_b64 = identity.sign(&payload)?;
        Ok(Self {
            session_nonce_b64,
            init_ts_ms: now_ms,
            sender_ed25519_pub_b64: identity.ed25519_public_b64.clone(),
            sender_x25519_pub_b64: identity.x25519_public_b64.clone(),
            supports_zstd,
            signature_b64,
        })
    }

    /// Verifies signature, nonce format, timestamp freshness, `supports_zstd` integrity, and pinned peer identity keys.
    pub fn verify(
        &self,
        src_client_id: Uuid,
        dst_daemon_id: Uuid,
        expected_ed25519_pub_b64: &str,
        expected_x25519_pub_b64: &str,
        now_ms: u64,
        max_skew_ms: u64,
    ) -> Result<[u8; RELAY_SESSION_NONCE_LEN]> {
        let actual_ed = decode_b64_key_32(&self.sender_ed25519_pub_b64, "sender ed25519")?;
        let exp_ed = decode_b64_key_32(expected_ed25519_pub_b64, "expected ed25519")?;
        if actual_ed != exp_ed {
            bail!("relay session auth Ed25519 key does not match trusted device key");
        }
        let actual_x = decode_b64_key_32(&self.sender_x25519_pub_b64, "sender x25519")?;
        let exp_x = decode_b64_key_32(expected_x25519_pub_b64, "expected x25519")?;
        if actual_x != exp_x {
            bail!("relay session auth X25519 key does not match trusted device key");
        }
        if !is_timestamp_fresh(self.init_ts_ms, now_ms, max_skew_ms) {
            bail!(
                "relay session auth timestamp out of window (init_ts_ms={}, now_ms={})",
                self.init_ts_ms,
                now_ms
            );
        }
        let nonce_raw = B64
            .decode(&self.session_nonce_b64)
            .context("invalid relay session nonce base64")?;
        let nonce_arr: [u8; RELAY_SESSION_NONCE_LEN] = nonce_raw
            .try_into()
            .map_err(|_| anyhow!("invalid relay session nonce length (expected 16 bytes)"))?;
        let signing_bytes_zstd = relay_session_auth_signing_bytes_with_zstd(
            src_client_id,
            dst_daemon_id,
            &self.sender_x25519_pub_b64,
            &self.session_nonce_b64,
            self.init_ts_ms,
            self.supports_zstd,
        );
        if StoredDeviceIdentity::verify_signature(
            expected_ed25519_pub_b64,
            &signing_bytes_zstd,
            &self.signature_b64,
        )
        .is_err()
        {
            // Backward-compatible verification for v0.4.0 peers that did not append `:zstd={0|1}`.
            // Note: A signature created with `:zstd=0` will NOT verify under the legacy format,
            // so tampering `supports_zstd` on new clients is cryptographically rejected.
            let legacy_bytes = relay_session_auth_signing_bytes(
                src_client_id,
                dst_daemon_id,
                &self.sender_x25519_pub_b64,
                &self.session_nonce_b64,
                self.init_ts_ms,
            );
            StoredDeviceIdentity::verify_signature(
                expected_ed25519_pub_b64,
                &legacy_bytes,
                &self.signature_b64,
            )?;
        }
        Ok(nonce_arr)
    }
}

/// End-to-End Encrypted Directional Cipher (RFC 7748 X25519 + HKDF-SHA256 + ChaCha20Poly1305).
/// 100% interoperable with Apple Swift `CryptoKit` (`Curve25519` + `HKDF<SHA256>` + `ChaChaPoly`).
pub struct E2eeSession {
    tx_cipher: ChaCha20Poly1305,
    rx_cipher: ChaCha20Poly1305,
    tx_dir_tag: u8,
    rx_dir_tag: u8,
    next_tx_seq: u64,
    rx_window: ReplayWindow,
    peer_lacks_zstd: bool,
    aad_binding: Option<[u8; 16]>,
    session_nonce_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub seq: u64,
    pub compressed: bool,
    pub ciphertext_b64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_nonce_b64: Option<String>,
}

fn x25519_dh_checked(
    my_secret: &X25519Secret,
    peer_public_b64: &str,
    label: &str,
) -> Result<([u8; 32], [u8; 32])> {
    let peer_bytes = decode_b64_key_32(peer_public_b64, label)?;
    let peer_pub = X25519Public::from(peer_bytes);
    let shared = my_secret.diffie_hellman(&peer_pub);
    if !shared.was_contributory() {
        bail!("non-contributory (low-order) {label}");
    }
    Ok((*shared.as_bytes(), peer_bytes))
}

impl E2eeSession {
    /// Legacy static X25519 derivation helper (prefer `derive_ephemeral_p2p` or `derive_relay_session`).
    /// `is_client_initiator = true` on the controlling client (iPhone/Mac/Web), `false` on `coda-daemon`.
    pub fn derive(
        my_secret: &X25519Secret,
        peer_public_b64: &str,
        is_client_initiator: bool,
    ) -> Result<Self> {
        let (shared, _) = x25519_dh_checked(my_secret, peer_public_b64, "peer x25519 public key")?;

        let hk = Hkdf::<Sha256>::new(Some(b"coda-zk-mesh-salt-v1"), &shared);
        let mut c2d_key = [0u8; 32];
        let mut d2c_key = [0u8; 32];
        hk.expand(b"coda-client-to-daemon-aead", &mut c2d_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;
        hk.expand(b"coda-daemon-to-client-aead", &mut d2c_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;

        let (mut tx_raw, mut rx_raw, tx_dir_tag, rx_dir_tag) = if is_client_initiator {
            (c2d_key, d2c_key, 0x01, 0x02)
        } else {
            (d2c_key, c2d_key, 0x02, 0x01)
        };
        let tx_cipher = ChaCha20Poly1305::new(Key::from_slice(&tx_raw));
        let rx_cipher = ChaCha20Poly1305::new(Key::from_slice(&rx_raw));
        zeroize_bytes(&mut tx_raw);
        zeroize_bytes(&mut rx_raw);
        zeroize_bytes(&mut c2d_key);
        zeroize_bytes(&mut d2c_key);

        Ok(Self {
            tx_cipher,
            rx_cipher,
            tx_dir_tag,
            rx_dir_tag,
            next_tx_seq: 1,
            rx_window: ReplayWindow::default(),
            peer_lacks_zstd: false,
            aad_binding: None,
            session_nonce_b64: None,
        })
    }

    /// Direct P2P v2 hybrid static+ephemeral X25519 derivation with forward secrecy and identity-bound HKDF info.
    #[allow(clippy::too_many_arguments)]
    pub fn derive_ephemeral_p2p(
        my_static_secret: &X25519Secret,
        peer_static_public_b64: &str,
        my_ephemeral_secret: &X25519Secret,
        peer_ephemeral_public_b64: &str,
        client_device_id: Uuid,
        daemon_device_id: Uuid,
        is_client_initiator: bool,
        peer_supports_zstd: bool,
    ) -> Result<Self> {
        let (static_shared, _) = x25519_dh_checked(
            my_static_secret,
            peer_static_public_b64,
            "peer static x25519 public key",
        )?;
        let (eph_shared, peer_eph_raw) = x25519_dh_checked(
            my_ephemeral_secret,
            peer_ephemeral_public_b64,
            "peer ephemeral x25519 public key",
        )?;
        let my_eph_raw = X25519Public::from(my_ephemeral_secret).to_bytes();

        let (client_eph_raw, daemon_eph_raw) = if is_client_initiator {
            (my_eph_raw, peer_eph_raw)
        } else {
            (peer_eph_raw, my_eph_raw)
        };

        let mut salt_hasher = Sha256::new();
        salt_hasher.update(client_eph_raw);
        salt_hasher.update(daemon_eph_raw);
        let salt: [u8; 32] = salt_hasher.finalize().into();

        let mut ikm = [0u8; 64];
        ikm[..32].copy_from_slice(&static_shared);
        ikm[32..].copy_from_slice(&eph_shared);

        let hk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
        let c_str = client_device_id.hyphenated().to_string();
        let d_str = daemon_device_id.hyphenated().to_string();
        let info_c2d = format!("coda-p2p-v2-c2d:{c_str}:{d_str}");
        let info_d2c = format!("coda-p2p-v2-d2c:{c_str}:{d_str}");
        let mut c2d_key = [0u8; 32];
        let mut d2c_key = [0u8; 32];
        hk.expand(info_c2d.as_bytes(), &mut c2d_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;
        hk.expand(info_d2c.as_bytes(), &mut d2c_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;

        let (mut tx_raw, mut rx_raw, tx_dir_tag, rx_dir_tag) = if is_client_initiator {
            (c2d_key, d2c_key, 0x01, 0x02)
        } else {
            (d2c_key, c2d_key, 0x02, 0x01)
        };
        let tx_cipher = ChaCha20Poly1305::new(Key::from_slice(&tx_raw));
        let rx_cipher = ChaCha20Poly1305::new(Key::from_slice(&rx_raw));
        zeroize_bytes(&mut tx_raw);
        zeroize_bytes(&mut rx_raw);
        zeroize_bytes(&mut c2d_key);
        zeroize_bytes(&mut d2c_key);
        zeroize_bytes(&mut ikm);

        let mut binding = [0u8; 16];
        binding.copy_from_slice(&salt[..16]);

        Ok(Self {
            tx_cipher,
            rx_cipher,
            tx_dir_tag,
            rx_dir_tag,
            next_tx_seq: 1,
            rx_window: ReplayWindow::default(),
            peer_lacks_zstd: !peer_supports_zstd,
            aad_binding: Some(binding),
            session_nonce_b64: None,
        })
    }

    /// Relay v2 salted X25519 session derivation bound to `session_nonce` and endpoint device IDs.
    pub fn derive_relay_session(
        my_static_secret: &X25519Secret,
        peer_static_public_b64: &str,
        session_nonce_b64: &str,
        client_device_id: Uuid,
        daemon_device_id: Uuid,
        is_client_initiator: bool,
        peer_supports_zstd: bool,
    ) -> Result<Self> {
        let nonce_raw = B64
            .decode(session_nonce_b64)
            .context("invalid relay session nonce base64")?;
        let nonce_arr: [u8; RELAY_SESSION_NONCE_LEN] = nonce_raw
            .try_into()
            .map_err(|_| anyhow!("invalid relay session nonce length (expected 16 bytes)"))?;
        let canonical_nonce_b64 = B64.encode(nonce_arr);
        let (mut static_shared, _) = x25519_dh_checked(
            my_static_secret,
            peer_static_public_b64,
            "peer static x25519 public key",
        )?;

        let hk = Hkdf::<Sha256>::new(Some(&nonce_arr), &static_shared);
        let c_str = client_device_id.hyphenated().to_string();
        let d_str = daemon_device_id.hyphenated().to_string();
        let info_c2d = format!("coda-relay-v2-c2d:{c_str}:{d_str}");
        let info_d2c = format!("coda-relay-v2-d2c:{c_str}:{d_str}");
        let mut c2d_key = [0u8; 32];
        let mut d2c_key = [0u8; 32];
        hk.expand(info_c2d.as_bytes(), &mut c2d_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;
        hk.expand(info_d2c.as_bytes(), &mut d2c_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;

        let (mut tx_raw, mut rx_raw, tx_dir_tag, rx_dir_tag) = if is_client_initiator {
            (c2d_key, d2c_key, 0x01, 0x02)
        } else {
            (d2c_key, c2d_key, 0x02, 0x01)
        };
        let tx_cipher = ChaCha20Poly1305::new(Key::from_slice(&tx_raw));
        let rx_cipher = ChaCha20Poly1305::new(Key::from_slice(&rx_raw));
        zeroize_bytes(&mut tx_raw);
        zeroize_bytes(&mut rx_raw);
        zeroize_bytes(&mut c2d_key);
        zeroize_bytes(&mut d2c_key);
        zeroize_bytes(&mut static_shared);

        Ok(Self {
            tx_cipher,
            rx_cipher,
            tx_dir_tag,
            rx_dir_tag,
            next_tx_seq: 1,
            rx_window: ReplayWindow::default(),
            peer_lacks_zstd: !peer_supports_zstd,
            aad_binding: Some(nonce_arr),
            session_nonce_b64: Some(canonical_nonce_b64),
        })
    }

    pub fn set_peer_lacks_zstd(&mut self, lacks: bool) {
        self.peer_lacks_zstd = lacks;
    }

    pub fn session_nonce_b64(&self) -> Option<&str> {
        self.session_nonce_b64.as_deref()
    }

    fn make_nonce(dir_tag: u8, seq: u64) -> Nonce {
        let mut n = [0u8; 12];
        n[0] = dir_tag;
        n[4..12].copy_from_slice(&seq.to_be_bytes());
        *Nonce::from_slice(&n)
    }

    fn make_aad(&self, compressed: bool, dir_tag: u8) -> Vec<u8> {
        let c_byte = if compressed { 1u8 } else { 0u8 };
        match self.aad_binding {
            Some(binding) => {
                let mut aad = Vec::with_capacity(18);
                aad.push(c_byte);
                aad.push(dir_tag);
                aad.extend_from_slice(&binding);
                aad
            }
            None => vec![c_byte],
        }
    }

    pub fn seal_message(&mut self, msg: &PeerMessage) -> Result<EncryptedPayload> {
        let raw_json = serde_json::to_vec(msg)?;
        let (body, compressed) = if !self.peer_lacks_zstd && raw_json.len() >= ZSTD_COMPRESS_THRESHOLD {
            let encoded = zstd::stream::encode_all(Cursor::new(&raw_json), 3)?;
            if encoded.len() + 16 < raw_json.len() {
                (encoded, true)
            } else {
                (raw_json, false)
            }
        } else {
            (raw_json, false)
        };

        let seq = self.next_tx_seq;
        self.next_tx_seq = self
            .next_tx_seq
            .checked_add(1)
            .ok_or_else(|| anyhow!("E2EE tx sequence exhausted"))?;
        let nonce = Self::make_nonce(self.tx_dir_tag, seq);
        let aad = self.make_aad(compressed, self.tx_dir_tag);
        let ct = self
            .tx_cipher
            .encrypt(&nonce, Payload { msg: &body, aad: &aad })
            .map_err(|_| anyhow!("AEAD ChaCha20Poly1305 encryption failed"))?;

        Ok(EncryptedPayload {
            seq,
            compressed,
            ciphertext_b64: B64.encode(ct),
            session_nonce_b64: self.session_nonce_b64.clone(),
        })
    }

    pub fn open_message(&mut self, payload: &EncryptedPayload) -> Result<PeerMessage> {
        if let Some(expected_nonce) = self.session_nonce_b64.as_deref() {
            if payload.session_nonce_b64.as_deref() != Some(expected_nonce) {
                bail!("missing or mismatched relay session_nonce_b64 in encrypted payload");
            }
        }
        if payload.ciphertext_b64.len() > MAX_B64_PAYLOAD_LEN {
            bail!("encrypted payload exceeds maximum wire size");
        }
        self.rx_window.check(payload.seq)?;
        let ct = B64.decode(&payload.ciphertext_b64)?;
        let nonce = Self::make_nonce(self.rx_dir_tag, payload.seq);
        let aad = self.make_aad(payload.compressed, self.rx_dir_tag);
        let pt = self
            .rx_cipher
            .decrypt(&nonce, Payload { msg: &ct, aad: &aad })
            .map_err(|_| anyhow!("AEAD decryption failed (wrong key or tampered frame)"))?;

        let json_bytes = if payload.compressed {
            zstd_decode_bounded(&pt, MAX_DECOMPRESSED_BYTES)?
        } else {
            if pt.len() > MAX_DECOMPRESSED_BYTES {
                bail!(
                    "uncompressed E2EE payload exceeds limit ({} bytes)",
                    MAX_DECOMPRESSED_BYTES
                );
            }
            pt
        };
        let msg: PeerMessage = serde_json::from_slice(&json_bytes)?;
        // Only commit replay window after AEAD decryption, bounded decompression, and JSON decode succeed.
        self.rx_window.commit(payload.seq);
        if msg.request_id().is_some_and(|id| id.starts_with("swift-")) {
            self.peer_lacks_zstd = true;
        }
        Ok(msg)
    }
}

/// Decompresses raw `zstd` bytes with a hard ceiling of `max_bytes` to defend against zip-bombs.
pub fn zstd_decode_bounded(raw: &[u8], max_bytes: usize) -> Result<Vec<u8>> {
    let decoder = zstd::stream::Decoder::new(Cursor::new(raw))?;
    let mut out = Vec::new();
    let mut limited = decoder.take((max_bytes as u64).saturating_add(1));
    limited.read_to_end(&mut out)?;
    if out.len() > max_bytes {
        bail!(
            "zstd decompressed payload exceeds limit ({} bytes)",
            max_bytes
        );
    }
    Ok(out)
}

/// Decodes base64-encoded `zstd` payload into raw bytes with zip-bomb protection.
pub fn zstd_decompress_b64_to_bytes(compressed_b64: &str) -> Result<Vec<u8>> {
    if compressed_b64.len() > MAX_B64_PAYLOAD_LEN {
        bail!("base64 zstd payload exceeds maximum wire size");
    }
    let raw = B64.decode(compressed_b64)?;
    zstd_decode_bounded(&raw, MAX_DECOMPRESSED_BYTES)
}

/// Decodes base64-encoded `zstd` payload and returns base64-encoded raw bytes (binary/NUL-safe across C FFI).
pub fn zstd_decompress_b64_to_b64(compressed_b64: &str) -> Result<String> {
    let decompressed = zstd_decompress_b64_to_bytes(compressed_b64)?;
    Ok(B64.encode(decompressed))
}

/// Decodes base64-encoded `zstd` payload and returns UTF-8 JSON string (used by Apple FFI).
pub fn zstd_decompress_b64_to_utf8(compressed_b64: &str) -> Result<String> {
    let decompressed = zstd_decompress_b64_to_bytes(compressed_b64)?;
    Ok(String::from_utf8(decompressed)?)
}

/// Encrypts a sensitive short-lived secret (such as a pending RFC 8628 session token at rest in DB)
/// using HKDF-SHA256 + ChaCha20Poly1305 keyed from `wrapping_secret`.
pub fn seal_opaque_secret(wrapping_secret: &[u8], plaintext: &str) -> Result<String> {
    let hk = Hkdf::<Sha256>::new(Some(b"coda-opaque-secret-v1"), wrapping_secret);
    let mut key_bytes = [0u8; 32];
    hk.expand(b"coda-db-token-wrap-aead", &mut key_bytes)
        .map_err(|_| anyhow!("hkdf expand failed"))?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    zeroize_bytes(&mut key_bytes);
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: b"coda-device-auth-token-v1",
            },
        )
        .map_err(|_| anyhow!("failed sealing opaque secret"))?;
    Ok(format!("enc:v1:{}.{}", B64.encode(nonce_bytes), B64.encode(ct)))
}

/// Decrypts an opaque secret sealed with `seal_opaque_secret`.
pub fn open_opaque_secret(wrapping_secret: &[u8], sealed: &str) -> Result<String> {
    let rest = sealed
        .strip_prefix("enc:v1:")
        .ok_or_else(|| anyhow!("unsealed or malformed opaque secret"))?;
    let (nonce_b64, ct_b64) = rest
        .split_once('.')
        .ok_or_else(|| anyhow!("malformed opaque secret envelope"))?;
    let nonce_raw = B64.decode(nonce_b64)?;
    let nonce_arr: [u8; 12] = nonce_raw
        .try_into()
        .map_err(|_| anyhow!("invalid opaque secret nonce length"))?;
    let ct = B64.decode(ct_b64)?;
    let hk = Hkdf::<Sha256>::new(Some(b"coda-opaque-secret-v1"), wrapping_secret);
    let mut key_bytes = [0u8; 32];
    hk.expand(b"coda-db-token-wrap-aead", &mut key_bytes)
        .map_err(|_| anyhow!("hkdf expand failed"))?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    zeroize_bytes(&mut key_bytes);
    let pt = cipher
        .decrypt(
            Nonce::from_slice(&nonce_arr),
            Payload {
                msg: &ct,
                aad: b"coda-device-auth-token-v1",
            },
        )
        .map_err(|_| anyhow!("failed decrypting opaque secret"))?;
    Ok(String::from_utf8(pt)?)
}

// ============================================================================
// 2. Stateless HMAC Relay Ticket (O(1) Admission without DB Lookup on Relays)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayTicketClaims {
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub peer_device_id: Option<Uuid>,
    pub role: String,
    pub quota_mbps: u32,
    pub exp_unix: i64,
}

impl RelayTicketClaims {
    pub fn sign(&self, relay_secret: &[u8]) -> Result<String> {
        let json = serde_json::to_vec(self)?;
        let body_b64 = B64.encode(&json);
        let mut mac = <HmacSha256 as Mac>::new_from_slice(relay_secret)
            .map_err(|_| anyhow!("invalid relay HMAC secret"))?;
        mac.update(body_b64.as_bytes());
        let tag_b64 = B64.encode(mac.finalize().into_bytes());
        Ok(format!("{body_b64}.{tag_b64}"))
    }

    pub fn verify(token: &str, relay_secret: &[u8], now_unix: i64) -> Result<Self> {
        let (body_b64, tag_b64) = token
            .split_once('.')
            .ok_or_else(|| anyhow!("malformed relay ticket"))?;
        let tag_bytes = B64.decode(tag_b64)?;
        let mut mac = <HmacSha256 as Mac>::new_from_slice(relay_secret)
            .map_err(|_| anyhow!("invalid relay HMAC secret"))?;
        mac.update(body_b64.as_bytes());
        mac.verify_slice(&tag_bytes)
            .map_err(|_| anyhow!("relay ticket HMAC verification failed"))?;

        let claims: Self = serde_json::from_slice(&B64.decode(body_b64)?)?;
        if claims.exp_unix < now_unix {
            bail!("relay ticket expired");
        }
        Ok(claims)
    }
}

// ============================================================================
// 3. Dual-Lane QoS Wire Envelope (Used across Raw TCP No-SNI, WSS, and P2P)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficLane {
    /// Highest priority: keystrokes, OSC 7501 agent status, approvals, heartbeats.
    /// Never throttled by bulk token bucket; bypasses bulk queue!
    ControlP0,
    /// Bulk data: large scrollback dumps, Git diffs, remote localhost web previews.
    BulkP1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "frame_type", rename_all = "snake_case")]
pub enum RelayWireFrame {
    /// First frame sent when connecting to `coda-relay` (over Raw TCP :24443 or CDN WSS).
    AuthBind {
        ticket: String,
        lane: TrafficLane,
        /// Monotonic connection generation for 75s CDN Make-Before-Break seamless handoff!
        generation: u64,
    },
    AuthAck {
        ok: bool,
        device_id: Uuid,
        server_region: String,
        message: String,
    },
    /// Blindly routed encrypted frame (`coda-relay` only inspects `src`/`dst`/`lane`, NEVER decrypts `e2ee`).
    RoutePacket {
        src_device_id: Uuid,
        dst_device_id: Uuid,
        lane: TrafficLane,
        e2ee: EncryptedPayload,
        /// Sender's ephemeral or static X25519 public key (legacy compatibility field).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_x25519_pub_b64: Option<String>,
        /// Signed Relay v2 session authentication envelope (nonce + Ed25519 signature over endpoints).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_auth: Option<RelaySessionAuth>,
    },
    PeerPresence {
        device_id: Uuid,
        is_online: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoints: Option<Vec<NetworkEndpointCandidate>>,
    },
    /// Dynamic LAN / IPv6 P2P endpoint refresh pushed by `coda-daemon` or embedded Mac host on connect & Wi-Fi changes.
    AdvertiseEndpoints {
        endpoints: Vec<NetworkEndpointCandidate>,
    },
    /// Phase 3 Direct P2P (LAN IPv4 / Mainland China Public IPv6) zero-relay handshake initiation.
    DirectP2pHello {
        client_device_id: Uuid,
        target_device_id: Uuid,
        client_ed25519_pub_b64: String,
        client_x25519_pub_b64: String,
        #[serde(default)]
        client_ephemeral_x25519_pub_b64: String,
        #[serde(default = "default_true")]
        supports_zstd: bool,
        timestamp_ms: u64,
        signature_b64: String,
    },
    /// Phase 3 Direct P2P handshake acknowledgment from `coda-daemon`.
    DirectP2pAck {
        ok: bool,
        daemon_device_id: Uuid,
        daemon_x25519_pub_b64: String,
        #[serde(default)]
        daemon_ed25519_pub_b64: String,
        #[serde(default)]
        daemon_ephemeral_x25519_pub_b64: String,
        #[serde(default)]
        timestamp_ms: u64,
        #[serde(default)]
        signature_b64: String,
        path_kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    KeepAlive {
        ts_ms: u64,
    },
    /// Unencrypted control signal routed from target back to sender when the target restarted or
    /// reconnected and no longer holds the sender's Relay v2 E2EE session state.
    SessionReset {
        src_device_id: Uuid,
        dst_device_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_nonce_b64: Option<String>,
        reason: String,
    },
    Error {
        code: String,
        message: String,
    },
}

impl RelayWireFrame {
    /// Encodes frame for Domestic Cloud No-ICP Raw TCP (`[u32_be_len][json_bytes]`) — zero HTTP/SNI!
    pub fn encode_length_delimited(&self) -> Result<Vec<u8>> {
        let payload = serde_json::to_vec(self)?;
        if payload.len() > MAX_WIRE_FRAME_BYTES {
            bail!("frame exceeds max wire size ({} bytes)", payload.len());
        }
        let mut out = Vec::with_capacity(4 + payload.len());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }
}

// ============================================================================
// 4. End-to-End Encrypted Inner Application Messages (`PeerMessage`)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkEndpointCandidate {
    pub kind: String, // "lan_ipv4" | "lan_ws" | "cn_public_ipv6" | "ipv6_ws" | "stun_ipv4"
    pub addr: String,
}

/// Validates a peer-advertised Direct P2P endpoint candidate to prevent SSRF / arbitrary URL probing.
pub fn is_valid_network_endpoint(ep: &NetworkEndpointCandidate) -> bool {
    let kind = ep.kind.trim();
    let addr = ep.addr.trim();
    if addr.is_empty() || addr.len() > 128 || addr.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    match kind {
        "lan_ipv4" | "cn_public_ipv6" | "lan_ipv6" | "stun_ipv4" => {
            addr.parse::<std::net::SocketAddr>()
                .map(|sa| sa.port() > 0 && !sa.ip().is_unspecified())
                .unwrap_or(false)
        }
        "lan_ws" | "ipv6_ws" => {
            let rest = addr
                .strip_prefix("ws://")
                .or_else(|| addr.strip_prefix("wss://"));
            let Some(host_port_path) = rest else {
                return false;
            };
            let host_port = host_port_path.strip_suffix('/').unwrap_or(host_port_path);
            if host_port.contains('/') || host_port.contains('?') || host_port.contains('#') || host_port.contains('@') {
                return false;
            }
            host_port
                .parse::<std::net::SocketAddr>()
                .map(|sa| sa.port() > 0 && !sa.ip().is_unspecified())
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// Filters and bounds a list of peer endpoint candidates (at most 16 valid entries).
pub fn sanitize_network_endpoints(eps: Vec<NetworkEndpointCandidate>) -> Vec<NetworkEndpointCandidate> {
    eps.into_iter()
        .filter(is_valid_network_endpoint)
        .take(16)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HerdrAgentStatus {
    pub state: String, // "idle" | "working" | "blocked" | "done" | "error"
    pub kind: Option<String>, // "permission" | "question" | "auth"
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
    pub progress: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HerdrTerminalSummary {
    pub address: String,
    pub name: Option<String>,
    pub argv: Vec<String>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub alive: bool,
    pub status: Option<HerdrAgentStatus>,
}

/// Git branch or worktree item inside a repository (displayed in Left Sidebar under `● <repo> · <machine>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchWorktreeItem {
    pub branch: String,
    pub worktree_path: String,
    pub diff_added: u32,
    pub diff_deleted: u32,
    pub active_terminals: Vec<HerdrTerminalSummary>,
    pub status_summary: String,
}

/// Repository workspace grouping all its Git worktrees/branches and active `herdr` terminals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoWorkspaceSummary {
    pub repo_name: String,
    pub root_path: String,
    pub machine_name: String,
    pub branches: Vec<BranchWorktreeItem>,
}

/// Directory entry returned from a remote daemon host for the desktop `Files` / `Code` explorer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteFileItem {
    pub name: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

/// Voice Model configuration (`provider + baseurl + key`) persisted in `herdr-core` `doc.set`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceModelConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PeerMessage {
    Ping {
        req_id: String,
        sent_ms: u64,
    },
    Pong {
        req_id: String,
        sent_ms: u64,
        daemon_hostname: String,
        herdr_version: String,
    },
    ListTerminals {
        req_id: String,
        include_finished: bool,
    },
    TerminalsSnapshot {
        req_id: String,
        core_name: String,
        core_id: String,
        terminals: Vec<HerdrTerminalSummary>,
    },
    /// Scans Git repositories, worktrees, `+added / -deleted` diff stats, and correlates `herdr` terminals.
    ListWorkspaces {
        req_id: String,
        root_dirs: Vec<String>,
    },
    WorkspacesSnapshot {
        req_id: String,
        machine_name: String,
        repos: Vec<RepoWorkspaceSummary>,
        voice_config: Option<VoiceModelConfig>,
    },
    LaunchAgent {
        req_id: String,
        /// Preset or binary: "claude", "codex", "grok", "agy", "gemini", or "shell"/"zsh"/"bash"
        agent: String,
        name: Option<String>,
        cwd: Option<String>,
        initial_prompt: Option<String>,
        cols: u16,
        rows: u16,
    },
    LaunchAgentResult {
        req_id: String,
        ok: bool,
        terminal: Option<HerdrTerminalSummary>,
        error: Option<String>,
    },
    /// Full-Duplex PTY Attach: spawns `hc attach <terminal>` in a real PTY and streams 24-bit ANSI/VT bytes.
    PtyAttach {
        req_id: String,
        terminal: String,
        cols: u16,
        rows: u16,
    },
    PtyAttached {
        req_id: String,
        terminal: String,
        ok: bool,
        error: Option<String>,
    },
    /// Raw stdin bytes (`base64`) sent to an attached PTY (`hc attach`), including Kitty Keyboard sequences.
    PtyInput {
        terminal: String,
        data_b64: String,
    },
    /// Dynamic window resize (`TIOCSWINSZ`) when iPhone Duo unfolds/folds or window resizes.
    PtyResize {
        terminal: String,
        cols: u16,
        rows: u16,
    },
    /// Cleanly detaches the PTY bridge (`0x1c` `Ctrl+\`) without terminating the underlying `herdr` terminal.
    PtyDetach {
        req_id: String,
        terminal: String,
    },
    /// Live ANSI/VT output byte stream chunk pushed from `coda-daemon` to attached clients.
    PtyOutputChunk {
        terminal: String,
        seq: u64,
        data_b64: String,
    },
    /// Uploads an image or file from `[ 📎 ]` directly into the remote terminal workspace via `hc terminal.put`.
    UploadTerminalFile {
        req_id: String,
        terminal: String,
        filename: String,
        data_b64: String,
    },
    UploadTerminalFileResult {
        req_id: String,
        terminal: String,
        ok: bool,
        remote_path: Option<String>,
        bytes: usize,
        error: Option<String>,
    },
    /// Configures the Voice Model (`provider + baseurl + key`) stored in `herdr-core` `doc.set`.
    SetVoiceConfig {
        req_id: String,
        config: VoiceModelConfig,
    },
    VoiceConfigAck {
        req_id: String,
        ok: bool,
        config: VoiceModelConfig,
    },
    SendTerminalText {
        req_id: String,
        terminal: String,
        text: String,
    },
    SendTerminalKeys {
        req_id: String,
        terminal: String,
        keys: Vec<String>,
    },
    InterruptTerminal {
        req_id: String,
        terminal: String,
    },
    ReadTerminalScreen {
        req_id: String,
        terminal: String,
        history_lines: u32,
    },
    TerminalScreenSnapshot {
        req_id: String,
        terminal: String,
        text: String,
        status: Option<HerdrAgentStatus>,
    },
    ActionAck {
        req_id: String,
        ok: bool,
        detail: String,
    },
    /// Live event pushed asynchronously from `herdr-core` `events.subscribe` (OSC 7501 updates!)
    HerdrLiveEvent {
        seq: u64,
        event_kind: String,
        terminal: Option<HerdrTerminalSummary>,
        raw_event: serde_json::Value,
    },
    /// Git working tree diff summary for Mobile / iPhone Duo Studio Split review
    GitDiffRequest {
        req_id: String,
        cwd: String,
    },
    GitDiffResponse {
        req_id: String,
        branch: String,
        status_short: String,
        diff_unified: String,
    },
    /// Cleanly terminates and removes a remote `herdr` terminal session.
    KillTerminal {
        req_id: String,
        terminal: String,
    },
    /// Lists a remote workspace directory (`sub_path` relative to `root_path`).
    ListDirectory {
        req_id: String,
        root_path: String,
        sub_path: String,
    },
    DirectoryListing {
        req_id: String,
        root_path: String,
        sub_path: String,
        entries: Vec<RemoteFileItem>,
        error: Option<String>,
    },
    /// Reads a UTF-8 preview of a file on the remote host.
    ReadFilePreview {
        req_id: String,
        path: String,
        max_bytes: usize,
    },
    FilePreviewResponse {
        req_id: String,
        path: String,
        content: String,
        error: Option<String>,
    },
    /// Zero-config remote localhost web port preview (`127.0.0.1:PORT` -> Mobile/Web preview sheet)
    HttpLocalPreviewRequest {
        req_id: String,
        port: u16,
        path: String,
    },
    HttpLocalPreviewResponse {
        req_id: String,
        status_code: u16,
        content_type: String,
        body_b64: String,
    },
}

impl PeerMessage {
    /// Returns the correlation `req_id` when this variant represents a client request.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Ping { req_id, .. }
            | Self::ListTerminals { req_id, .. }
            | Self::ListWorkspaces { req_id, .. }
            | Self::LaunchAgent { req_id, .. }
            | Self::PtyAttach { req_id, .. }
            | Self::PtyDetach { req_id, .. }
            | Self::UploadTerminalFile { req_id, .. }
            | Self::SetVoiceConfig { req_id, .. }
            | Self::SendTerminalText { req_id, .. }
            | Self::SendTerminalKeys { req_id, .. }
            | Self::InterruptTerminal { req_id, .. }
            | Self::ReadTerminalScreen { req_id, .. }
            | Self::GitDiffRequest { req_id, .. }
            | Self::KillTerminal { req_id, .. }
            | Self::ListDirectory { req_id, .. }
            | Self::ReadFilePreview { req_id, .. }
            | Self::HttpLocalPreviewRequest { req_id, .. } => Some(req_id.as_str()),
            _ => None,
        }
    }

    /// Returns the correlation `req_id` when this variant represents a daemon response.
    pub fn response_req_id(&self) -> Option<&str> {
        match self {
            Self::Pong { req_id, .. }
            | Self::TerminalsSnapshot { req_id, .. }
            | Self::WorkspacesSnapshot { req_id, .. }
            | Self::LaunchAgentResult { req_id, .. }
            | Self::PtyAttached { req_id, .. }
            | Self::UploadTerminalFileResult { req_id, .. }
            | Self::VoiceConfigAck { req_id, .. }
            | Self::TerminalScreenSnapshot { req_id, .. }
            | Self::ActionAck { req_id, .. }
            | Self::GitDiffResponse { req_id, .. }
            | Self::DirectoryListing { req_id, .. }
            | Self::FilePreviewResponse { req_id, .. }
            | Self::HttpLocalPreviewResponse { req_id, .. } => Some(req_id.as_str()),
            _ => None,
        }
    }
}

/// Real-time control-plane notification pushed by `coda-server` over `/api/v1/events/ws`
/// whenever a device is enrolled, paired, removed, or changes online relay presence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountLiveEvent {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_knowledge_e2ee_and_zstd_roundtrip() {
        let client_id = StoredDeviceIdentity::generate("iPhone Duo");
        let daemon_id = StoredDeviceIdentity::generate("Home Mac Studio");

        let mut client_sess = E2eeSession::derive(
            &client_id.x25519_secret().unwrap(),
            &daemon_id.x25519_public_b64,
            true,
        )
        .unwrap();
        let mut daemon_sess = E2eeSession::derive(
            &daemon_id.x25519_secret().unwrap(),
            &client_id.x25519_public_b64,
            false,
        )
        .unwrap();

        // Large terminal screen with repetitive ANSI sequences triggers zstd compression
        let big_screen = "\x1b[32m● Working\x1b[0m Building crate coda-server...\n".repeat(40);
        let msg = PeerMessage::TerminalScreenSnapshot {
            req_id: "r-1".into(),
            terminal: "t1".into(),
            text: big_screen.clone(),
            status: Some(HerdrAgentStatus {
                state: "blocked".into(),
                kind: Some("permission".into()),
                app: Some("claude".into()),
                title: Some("Run cargo test".into()),
                msg: Some("Allow bash execution?".into()),
                progress: Some(75),
            }),
        };

        let sealed = daemon_sess.seal_message(&msg).unwrap();
        assert!(sealed.compressed, "repetitive terminal output should auto-compress with zstd");
        assert!(sealed.ciphertext_b64.len() < big_screen.len());

        let opened = client_sess.open_message(&sealed).unwrap();
        match opened {
            PeerMessage::TerminalScreenSnapshot { text, status, .. } => {
                assert_eq!(text, big_screen);
                assert_eq!(status.unwrap().kind.as_deref(), Some("permission"));
            }
            _ => panic!("unexpected variant"),
        }

        // Replay attack must fail
        assert!(client_sess.open_message(&sealed).is_err());
    }

    #[test]
    fn test_stateless_hmac_relay_ticket() {
        let secret = b"super-secret-relay-hmac-key-2026";
        let claims = RelayTicketClaims {
            user_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            peer_device_id: Some(Uuid::new_v4()),
            role: "client".into(),
            quota_mbps: 25,
            exp_unix: 2_000_000_000,
        };
        let token = claims.sign(secret).unwrap();
        let verified = RelayTicketClaims::verify(&token, secret, 1_900_000_000).unwrap();
        assert_eq!(verified.user_id, claims.user_id);
        assert!(RelayTicketClaims::verify(&token, b"wrong-key", 1_900_000_000).is_err());
        assert!(RelayTicketClaims::verify(&token, secret, 2_100_000_000).is_err());
    }

    #[test]
    fn test_swift_req_id_negotiates_uncompressed_reply() {
        let client_id = StoredDeviceIdentity::generate("iPhone App");
        let daemon_id = StoredDeviceIdentity::generate("Studio Host");

        let mut client_sess = E2eeSession::derive(
            &client_id.x25519_secret().unwrap(),
            &daemon_id.x25519_public_b64,
            true,
        )
        .unwrap();
        let mut daemon_sess = E2eeSession::derive(
            &daemon_id.x25519_secret().unwrap(),
            &client_id.x25519_public_b64,
            false,
        )
        .unwrap();

        let req = PeerMessage::ReadTerminalScreen {
            req_id: "swift-1".into(),
            terminal: "t1".into(),
            history_lines: 180,
        };
        let sealed_req = client_sess.seal_message(&req).unwrap();
        let opened_req = daemon_sess.open_message(&sealed_req).unwrap();
        assert_eq!(opened_req.request_id(), Some("swift-1"));

        let big_screen = "Large terminal line for Swift fallback check\n".repeat(80);
        let reply = PeerMessage::TerminalScreenSnapshot {
            req_id: "swift-1".into(),
            terminal: "t1".into(),
            text: big_screen.clone(),
            status: None,
        };
        let sealed_reply = daemon_sess.seal_message(&reply).unwrap();
        assert!(
            !sealed_reply.compressed,
            "daemon should skip zstd compression once peer sends swift-* req_id"
        );
        let opened_reply = client_sess.open_message(&sealed_reply).unwrap();
        match opened_reply {
            PeerMessage::TerminalScreenSnapshot { text, .. } => assert_eq!(text, big_screen),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn test_response_req_id_and_account_live_event_roundtrip() {
        let ping = PeerMessage::Ping {
            req_id: "r-ping".into(),
            sent_ms: 42,
        };
        assert_eq!(ping.request_id(), Some("r-ping"));
        assert_eq!(ping.response_req_id(), None);

        let pong = PeerMessage::Pong {
            req_id: "r-ping".into(),
            sent_ms: 42,
            daemon_hostname: "linux-box".into(),
            herdr_version: "0.1.21".into(),
        };
        assert_eq!(pong.request_id(), None);
        assert_eq!(pong.response_req_id(), Some("r-ping"));

        let push = PeerMessage::PtyOutputChunk {
            terminal: "t1".into(),
            seq: 1,
            data_b64: "".into(),
        };
        assert_eq!(push.request_id(), None);
        assert_eq!(push.response_req_id(), None);

        let ev = AccountLiveEvent {
            kind: "devices_changed".into(),
            device_id: Some(Uuid::new_v4()),
            device_name: Some("openEuler Server".into()),
            role: Some("host".into()),
            online: Some(true),
            timestamp_ms: Some(1_700_000_000_000),
        };
        let encoded = serde_json::to_string(&ev).unwrap();
        let decoded: AccountLiveEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, ev);
    }

    #[test]
    fn test_ephemeral_p2p_v2_relay_v2_replay_window_and_zipbomb() {
        let mut client_id = StoredDeviceIdentity::generate("iPhone App");
        let mut daemon_id = StoredDeviceIdentity::generate("Studio Host");
        let cid = Uuid::new_v4();
        let did = Uuid::new_v4();
        client_id.device_id = Some(cid);
        daemon_id.device_id = Some(did);

        // 1. Ephemeral P2P v2 roundtrip + sliding window + forged-seq DoS resistance
        let (c_eph_sec, c_eph_pub) = StoredDeviceIdentity::generate_ephemeral_x25519();
        let (d_eph_sec, d_eph_pub) = StoredDeviceIdentity::generate_ephemeral_x25519();
        let now = current_unix_ms();

        let hello_bytes = p2p_hello_v2_signing_bytes(
            cid,
            did,
            &client_id.x25519_public_b64,
            &c_eph_pub,
            now,
        );
        let hello_sig = client_id.sign(&hello_bytes).unwrap();
        StoredDeviceIdentity::verify_signature(
            &client_id.ed25519_public_b64,
            &hello_bytes,
            &hello_sig,
        )
        .unwrap();

        let ack_bytes = p2p_ack_v2_signing_bytes(cid, did, &c_eph_pub, &d_eph_pub, now);
        let ack_sig = daemon_id.sign(&ack_bytes).unwrap();
        StoredDeviceIdentity::verify_signature(
            &daemon_id.ed25519_public_b64,
            &ack_bytes,
            &ack_sig,
        )
        .unwrap();

        let mut p2p_c = E2eeSession::derive_ephemeral_p2p(
            &client_id.x25519_secret().unwrap(),
            &daemon_id.x25519_public_b64,
            &c_eph_sec,
            &d_eph_pub,
            cid,
            did,
            true,
            true,
        )
        .unwrap();
        let mut p2p_d = E2eeSession::derive_ephemeral_p2p(
            &daemon_id.x25519_secret().unwrap(),
            &client_id.x25519_public_b64,
            &d_eph_sec,
            &c_eph_pub,
            cid,
            did,
            false,
            false,
        )
        .unwrap();

        let f1 = p2p_c
            .seal_message(&PeerMessage::Ping {
                req_id: "r1".into(),
                sent_ms: 1,
            })
            .unwrap();
        let f2 = p2p_c
            .seal_message(&PeerMessage::Ping {
                req_id: "r2".into(),
                sent_ms: 2,
            })
            .unwrap();
        let f3 = p2p_c
            .seal_message(&PeerMessage::Ping {
                req_id: "r3".into(),
                sent_ms: 3,
            })
            .unwrap();

        // Out-of-order delivery (f3 then f1 then f2) succeeds under 64-bit sliding window
        assert_eq!(p2p_d.open_message(&f3).unwrap().request_id(), Some("r3"));
        assert_eq!(p2p_d.open_message(&f1).unwrap().request_id(), Some("r1"));

        // Attacker injects forged seq=9999999 frame: fails AEAD and MUST NOT advance replay window!
        let mut forged = f2.clone();
        forged.seq = 9_999_999;
        assert!(p2p_d.open_message(&forged).is_err());

        // Legitimate f2 still decrypts after forged-seq injection
        assert_eq!(p2p_d.open_message(&f2).unwrap().request_id(), Some("r2"));
        // Replaying f1 fails
        assert!(p2p_d.open_message(&f1).is_err());

        // Daemon honors peer_supports_zstd=false upfront
        let big = "Line for upfront zstd negotiation\n".repeat(40);
        let d_reply = p2p_d
            .seal_message(&PeerMessage::TerminalScreenSnapshot {
                req_id: "custom-1".into(),
                terminal: "t1".into(),
                text: big.clone(),
                status: None,
            })
            .unwrap();
        assert!(!d_reply.compressed);
        assert!(p2p_c.open_message(&d_reply).is_ok());

        // 2. Relay v2 session auth + salted session derivation
        let auth = RelaySessionAuth::create(&client_id, did, true, now).unwrap();
        let nonce_arr = auth
            .verify(
                cid,
                did,
                &client_id.ed25519_public_b64,
                &client_id.x25519_public_b64,
                now,
                HANDSHAKE_MAX_SKEW_MS,
            )
            .unwrap();
        assert_eq!(nonce_arr.len(), RELAY_SESSION_NONCE_LEN);

        let mut rel_c = E2eeSession::derive_relay_session(
            &client_id.x25519_secret().unwrap(),
            &daemon_id.x25519_public_b64,
            &auth.session_nonce_b64,
            cid,
            did,
            true,
            true,
        )
        .unwrap();
        let mut rel_d = E2eeSession::derive_relay_session(
            &daemon_id.x25519_secret().unwrap(),
            &client_id.x25519_public_b64,
            &auth.session_nonce_b64,
            cid,
            did,
            false,
            true,
        )
        .unwrap();
        let rf = rel_c
            .seal_message(&PeerMessage::Ping {
                req_id: "rel-1".into(),
                sent_ms: 9,
            })
            .unwrap();
        assert_eq!(
            rf.session_nonce_b64.as_deref(),
            Some(auth.session_nonce_b64.as_str())
        );
        assert_eq!(rel_d.open_message(&rf).unwrap().request_id(), Some("rel-1"));

        // Wrong session nonce fails AEAD
        let bad_rel = E2eeSession::derive_relay_session(
            &daemon_id.x25519_secret().unwrap(),
            &client_id.x25519_public_b64,
            &B64.encode([7u8; 16]),
            cid,
            did,
            false,
            true,
        )
        .unwrap();
        let mut bad_rel = bad_rel;
        assert!(bad_rel.open_message(&rf).is_err());

        // Low-order all-zero X25519 public key rejected
        assert!(validate_x25519_pub_b64(&B64.encode([0u8; 32])).is_err());

        // Bounded zstd rejects oversized payloads
        let zeros = vec![0u8; 2048];
        let compressed = zstd::stream::encode_all(Cursor::new(&zeros), 3).unwrap();
        assert!(zstd_decode_bounded(&compressed, 1024).is_err());
        assert_eq!(zstd_decode_bounded(&compressed, 4096).unwrap().len(), 2048);

        // Tampering `supports_zstd` on RelaySessionAuth must fail Ed25519 verification
        let mut tampered_auth = RelaySessionAuth::create(&client_id, did, false, now).unwrap();
        tampered_auth.supports_zstd = true;
        assert!(tampered_auth
            .verify(
                cid,
                did,
                &client_id.ed25519_public_b64,
                &client_id.x25519_public_b64,
                now,
                HANDSHAKE_MAX_SKEW_MS,
            )
            .is_err());

        // Stripping session_nonce_b64 from Relay v2 payload must fail open_message
        let mut missing_nonce_rf = rel_c
            .seal_message(&PeerMessage::Ping {
                req_id: "rel-2".into(),
                sent_ms: 10,
            })
            .unwrap();
        missing_nonce_rf.session_nonce_b64 = None;
        assert!(rel_d.open_message(&missing_nonce_rf).is_err());

        // Endpoint validator allows valid LAN/IPv6 addresses and rejects SSRF URLs
        assert!(is_valid_network_endpoint(&NetworkEndpointCandidate {
            kind: "lan_ipv4".into(),
            addr: "192.168.1.20:24444".into(),
        }));
        assert!(is_valid_network_endpoint(&NetworkEndpointCandidate {
            kind: "ipv6_ws".into(),
            addr: "ws://[2408:8207::1]:24444".into(),
        }));
        assert!(!is_valid_network_endpoint(&NetworkEndpointCandidate {
            kind: "lan_ws".into(),
            addr: "ws://evil.example.com/steal?x=1".into(),
        }));
        assert!(!is_valid_network_endpoint(&NetworkEndpointCandidate {
            kind: "lan_ipv4".into(),
            addr: "0.0.0.0:24444".into(),
        }));
    }
}
