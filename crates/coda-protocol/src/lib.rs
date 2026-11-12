//! Coda Wire Protocol, Zero-Knowledge E2EE (`X25519 + HKDF-SHA256 + ChaCha20Poly1305`),
//! `zstd` Adaptive Compression, Stateless HMAC Relay Tickets, and Dual-Lane QoS Framing.

use std::io::Cursor;

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret as X25519Secret};

type HmacSha256 = Hmac<Sha256>;

/// Minimum plaintext byte size before `zstd` compression kicks in.
const ZSTD_COMPRESS_THRESHOLD: usize = 256;
/// Maximum raw TCP frame size (8 MiB) to prevent memory exhaustion on stateless relays.
pub const MAX_WIRE_FRAME_BYTES: usize = 8 * 1024 * 1024;

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
        let arr: [u8; 32] = B64
            .decode(&self.x25519_secret_b64)?
            .try_into()
            .map_err(|_| anyhow!("invalid x25519 secret length"))?;
        Ok(X25519Secret::from(arr))
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
    highest_rx_seq: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub seq: u64,
    pub compressed: bool,
    pub ciphertext_b64: String,
}

impl E2eeSession {
    /// Derives directional keys from static/ephemeral X25519 keys.
    /// `is_client_initiator = true` on the controlling client (iPhone/Mac/Web), `false` on `coda-daemon`.
    pub fn derive(
        my_secret: &X25519Secret,
        peer_public_b64: &str,
        is_client_initiator: bool,
    ) -> Result<Self> {
        let peer_bytes: [u8; 32] = B64
            .decode(peer_public_b64)?
            .try_into()
            .map_err(|_| anyhow!("invalid peer x25519 public key"))?;
        let peer_pub = X25519Public::from(peer_bytes);
        let shared = my_secret.diffie_hellman(&peer_pub);

        let hk = Hkdf::<Sha256>::new(Some(b"coda-zk-mesh-salt-v1"), shared.as_bytes());
        let mut c2d_key = [0u8; 32];
        let mut d2c_key = [0u8; 32];
        hk.expand(b"coda-client-to-daemon-aead", &mut c2d_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;
        hk.expand(b"coda-daemon-to-client-aead", &mut d2c_key)
            .map_err(|_| anyhow!("hkdf expand failed"))?;

        let (tx_raw, rx_raw, tx_dir_tag, rx_dir_tag) = if is_client_initiator {
            (c2d_key, d2c_key, 0x01, 0x02)
        } else {
            (d2c_key, c2d_key, 0x02, 0x01)
        };

        Ok(Self {
            tx_cipher: ChaCha20Poly1305::new(Key::from_slice(&tx_raw)),
            rx_cipher: ChaCha20Poly1305::new(Key::from_slice(&rx_raw)),
            tx_dir_tag,
            rx_dir_tag,
            next_tx_seq: 1,
            highest_rx_seq: None,
        })
    }

    fn make_nonce(dir_tag: u8, seq: u64) -> Nonce {
        let mut n = [0u8; 12];
        n[0] = dir_tag;
        n[4..12].copy_from_slice(&seq.to_be_bytes());
        *Nonce::from_slice(&n)
    }

    pub fn seal_message(&mut self, msg: &PeerMessage) -> Result<EncryptedPayload> {
        let raw_json = serde_json::to_vec(msg)?;
        let (body, compressed) = if raw_json.len() >= ZSTD_COMPRESS_THRESHOLD {
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
        self.next_tx_seq += 1;
        let nonce = Self::make_nonce(self.tx_dir_tag, seq);
        let aad = [if compressed { 1u8 } else { 0u8 }];
        let ct = self
            .tx_cipher
            .encrypt(&nonce, Payload { msg: &body, aad: &aad })
            .map_err(|_| anyhow!("AEAD ChaCha20Poly1305 encryption failed"))?;

        Ok(EncryptedPayload {
            seq,
            compressed,
            ciphertext_b64: B64.encode(ct),
        })
    }

    pub fn open_message(&mut self, payload: &EncryptedPayload) -> Result<PeerMessage> {
        if let Some(prev) = self.highest_rx_seq {
            if payload.seq <= prev {
                bail!("replay or out-of-order E2EE frame seq={} <= {}", payload.seq, prev);
            }
        }
        let ct = B64.decode(&payload.ciphertext_b64)?;
        let nonce = Self::make_nonce(self.rx_dir_tag, payload.seq);
        let aad = [if payload.compressed { 1u8 } else { 0u8 }];
        let pt = self
            .rx_cipher
            .decrypt(&nonce, Payload { msg: &ct, aad: &aad })
            .map_err(|_| anyhow!("AEAD decryption failed (wrong key or tampered frame)"))?;

        let json_bytes = if payload.compressed {
            zstd::stream::decode_all(Cursor::new(&pt))?
        } else {
            pt
        };
        self.highest_rx_seq = Some(payload.seq);
        Ok(serde_json::from_slice(&json_bytes)?)
    }
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
        /// Optional unencrypted server-assisted dev fallback ONLY when Web UI pairs via ephemeral session;
        /// Native clients & Daemons use `e2ee`.
        e2ee: EncryptedPayload,
        /// Sender's ephemeral or static X25519 public key so stateless/Web clients can derive session keys on the fly.
        sender_x25519_pub_b64: Option<String>,
    },
    PeerPresence {
        device_id: Uuid,
        is_online: bool,
    },
    /// Phase 3 Direct P2P (LAN IPv4 / Mainland China Public IPv6) zero-relay handshake initiation.
    DirectP2pHello {
        client_device_id: Uuid,
        target_device_id: Uuid,
        client_ed25519_pub_b64: String,
        client_x25519_pub_b64: String,
        timestamp_ms: u64,
        signature_b64: String,
    },
    /// Phase 3 Direct P2P handshake acknowledgment from `coda-daemon`.
    DirectP2pAck {
        ok: bool,
        daemon_device_id: Uuid,
        daemon_x25519_pub_b64: String,
        path_kind: String,
    },
    KeepAlive {
        ts_ms: u64,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEndpointCandidate {
    pub kind: String, // "lan_ipv4" | "cn_public_ipv6" | "stun_ipv4"
    pub addr: String,
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
        /// Preset or binary: "claude", "codex", "grok", "gemini", or "zsh"/"bash"
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
}
