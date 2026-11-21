use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use base64::{
    engine::general_purpose::{STANDARD as B64_STD, URL_SAFE_NO_PAD as B64_URL},
    Engine as _,
};
use ring::{
    aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_128_GCM},
    agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, ECDH_P256},
    hkdf::{KeyType, Salt, HKDF_SHA256},
    rand::{SecureRandom, SystemRandom},
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::{Db, WebPushSubscriptionRecord};

pub const KV_VAPID_PKCS8_B64: &str = "vapid_pkcs8_b64";
pub const KV_VAPID_PUB_B64URL: &str = "vapid_pub_b64url";
pub const VAPID_SUBJECT: &str = "mailto:noreply@codahub.dev";
pub const VAPID_EXP_SECS: u64 = 43_200;
pub const AES128GCM_RECORD_SIZE: u32 = 4096;
pub const AES128GCM_HEADER_LEN: usize = 86;
pub const P256_UNCOMPRESSED_PUBKEY_LEN: usize = 65;
pub const WEB_PUSH_AUTH_SECRET_LEN: usize = 16;
pub const MAX_WEB_PUSH_PLAINTEXT_BYTES: usize = 3800;
pub const MAX_WEB_PUSH_SUBSCRIPTIONS_PER_USER: usize = 16;

struct HkdfOutLen(usize);

impl KeyType for HkdfOutLen {
    fn len(&self) -> usize {
        self.0
    }
}

struct VapidInner {
    key_pair: EcdsaKeyPair,
    public_key_b64url: String,
}

#[derive(Clone, Default)]
pub struct WebPushService {
    inner: Option<Arc<VapidInner>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VapidPublicKeyResponse {
    pub enabled: bool,
    pub public_key: String,
}

#[derive(Debug, Clone)]
pub enum WebPushDeliveryOutcome {
    Delivered(u16),
    Gone(u16),
    Failed(String),
}

impl WebPushService {
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    pub fn from_pkcs8_der(pkcs8_der: &[u8]) -> Result<Self> {
        let rng = SystemRandom::new();
        let key_pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8_der, &rng)
                .map_err(|_| anyhow!("invalid P-256 VAPID PKCS#8 keypair"))?;
        let pub_bytes = key_pair.public_key().as_ref();
        if pub_bytes.len() != P256_UNCOMPRESSED_PUBKEY_LEN || pub_bytes[0] != 0x04 {
            bail!("unexpected VAPID P-256 public key encoding");
        }
        let public_key_b64url = B64_URL.encode(pub_bytes);
        Ok(Self {
            inner: Some(Arc::new(VapidInner {
                key_pair,
                public_key_b64url,
            })),
        })
    }

    pub fn generate_ephemeral() -> Result<(Self, String)> {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| anyhow!("failed generating P-256 VAPID PKCS#8 keypair"))?;
        let pkcs8_b64 = B64_STD.encode(doc.as_ref());
        let svc = Self::from_pkcs8_der(doc.as_ref())?;
        Ok((svc, pkcs8_b64))
    }

    /// Loads or atomically initializes the persistent P-256 VAPID keypair in `server_kv`.
    pub async fn load_or_init(db: &Db) -> Result<Self> {
        if let Some(existing_pkcs8_b64) = db.get_server_kv(KV_VAPID_PKCS8_B64).await? {
            let pkcs8_der = decode_flexible_b64(&existing_pkcs8_b64)
                .context("corrupted vapid_pkcs8_b64 in server_kv")?;
            let svc = Self::from_pkcs8_der(&pkcs8_der)?;
            let pub_b64url = svc.public_key_b64url().to_string();
            if db.get_server_kv(KV_VAPID_PUB_B64URL).await?.as_deref() != Some(pub_b64url.as_str())
            {
                db.set_server_kv(KV_VAPID_PUB_B64URL, &pub_b64url).await?;
            }
            return Ok(svc);
        }

        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| anyhow!("failed generating P-256 VAPID PKCS#8 keypair"))?;
        let candidate_pkcs8_b64 = B64_STD.encode(doc.as_ref());

        // Race-safe insert: if another coordinator node initialized concurrently, keep the winner.
        let winning_pkcs8_b64 = db
            .insert_or_get_server_kv(KV_VAPID_PKCS8_B64, &candidate_pkcs8_b64)
            .await?;
        let pkcs8_der = decode_flexible_b64(&winning_pkcs8_b64)
            .context("failed decoding stored vapid_pkcs8_b64")?;
        let svc = Self::from_pkcs8_der(&pkcs8_der)?;
        db.set_server_kv(KV_VAPID_PUB_B64URL, svc.public_key_b64url())
            .await?;
        Ok(svc)
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn public_key_b64url(&self) -> &str {
        self.inner
            .as_ref()
            .map(|i| i.public_key_b64url.as_str())
            .unwrap_or("")
    }

    pub fn public_key_response(&self) -> VapidPublicKeyResponse {
        VapidPublicKeyResponse {
            enabled: self.is_enabled(),
            public_key: self.public_key_b64url().to_string(),
        }
    }

    /// Builds an RFC 8292 VAPID `Authorization: vapid t=<ES256_JWT>, k=<vapid_pub_b64url>` header value.
    pub fn build_vapid_authorization(&self, endpoint: &str, now_unix_secs: u64) -> Result<String> {
        let Some(inner) = self.inner.as_ref() else {
            bail!("VAPID Web Push is not enabled on this server");
        };
        let aud = endpoint_origin_audience(endpoint)?;
        let exp = now_unix_secs.saturating_add(VAPID_EXP_SECS);
        let header_b64 = B64_URL.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims_json = serde_json::to_vec(&json!({
            "aud": aud,
            "exp": exp,
            "sub": VAPID_SUBJECT,
        }))?;
        let claims_b64 = B64_URL.encode(&claims_json);
        let signing_input = format!("{header_b64}.{claims_b64}");

        let rng = SystemRandom::new();
        let sig = inner
            .key_pair
            .sign(&rng, signing_input.as_bytes())
            .map_err(|_| anyhow!("VAPID ES256 signing failed"))?;
        let sig_b64 = B64_URL.encode(sig.as_ref());
        Ok(format!(
            "vapid t={signing_input}.{sig_b64}, k={}",
            inner.public_key_b64url
        ))
    }
}

/// Extracts `<scheme>://<host>[:non_default_port]` per RFC 8292 §2 (`aud` claim).
pub fn endpoint_origin_audience(endpoint: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(endpoint.trim())
        .map_err(|e| anyhow!("invalid push endpoint URL: {e}"))?;
    let scheme = parsed.scheme();
    if !matches!(scheme, "https" | "http") {
        bail!("unsupported push endpoint scheme: {scheme}");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("push endpoint missing host"))?;
    match parsed.port() {
        Some(port) => Ok(format!("{scheme}://{host}:{port}")),
        None => Ok(format!("{scheme}://{host}")),
    }
}

pub fn decode_flexible_b64(raw: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim().trim_end_matches('=');
    if trimmed.is_empty() {
        bail!("empty base64 input");
    }
    let normalized: String = trimmed
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    B64_URL
        .decode(normalized.as_bytes())
        .map_err(|e| anyhow!("invalid base64/base64url encoding: {e}"))
}

/// Validates subscriber `p256dh` (65-byte uncompressed P-256 curve point) and `auth` (16-byte secret),
/// returning canonical `base64url` (unpadded) strings.
pub fn validate_and_normalize_subscription_keys(
    p256dh_raw: &str,
    auth_raw: &str,
) -> Result<(String, String)> {
    let p256dh_bytes = decode_flexible_b64(p256dh_raw).context("invalid keys.p256dh")?;
    if p256dh_bytes.len() != P256_UNCOMPRESSED_PUBKEY_LEN || p256dh_bytes[0] != 0x04 {
        bail!("keys.p256dh must be a 65-byte uncompressed P-256 public key (0x04 || X || Y)");
    }
    let auth_bytes = decode_flexible_b64(auth_raw).context("invalid keys.auth")?;
    if auth_bytes.len() != WEB_PUSH_AUTH_SECRET_LEN {
        bail!("keys.auth must decode to exactly 16 bytes");
    }

    // Verify `p256dh` lies on the NIST P-256 curve via trial ECDH agreement.
    let rng = SystemRandom::new();
    let probe_priv = EphemeralPrivateKey::generate(&ECDH_P256, &rng)
        .map_err(|_| anyhow!("failed generating P-256 validation key"))?;
    let peer_pub = UnparsedPublicKey::new(&ECDH_P256, &p256dh_bytes);
    agree_ephemeral(probe_priv, &peer_pub, |_| ())
        .map_err(|_| anyhow!("keys.p256dh is not a valid NIST P-256 public key"))?;

    Ok((B64_URL.encode(&p256dh_bytes), B64_URL.encode(&auth_bytes)))
}

pub fn validate_web_push_endpoint(raw: &str, allow_local_test: bool) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("web push endpoint must not be empty");
    }
    crate::validate_webhook_url(trimmed, "web_push_endpoint", allow_local_test)
}

fn hkdf_expand_exact(prk: &ring::hkdf::Prk, info: &[u8], out: &mut [u8]) -> Result<()> {
    let info_slices = [info];
    let okm = prk
        .expand(&info_slices, HkdfOutLen(out.len()))
        .map_err(|_| anyhow!("HKDF-SHA256 expand failed"))?;
    okm.fill(out)
        .map_err(|_| anyhow!("HKDF-SHA256 output fill failed"))?;
    Ok(())
}

/// Derives RFC 8291 §3.3 + §3.4 `(CEK[16], NONCE[12])` from `(ecdh_secret, auth_secret, ua_public, as_public, salt)`.
pub fn derive_rfc8291_cek_and_nonce(
    ecdh_secret: &[u8],
    auth_secret: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8; 16],
) -> Result<([u8; 16], [u8; 12])> {
    if ua_public.len() != P256_UNCOMPRESSED_PUBKEY_LEN || ua_public[0] != 0x04 {
        bail!("invalid ua_public length or prefix");
    }
    if as_public.len() != P256_UNCOMPRESSED_PUBKEY_LEN || as_public[0] != 0x04 {
        bail!("invalid as_public length or prefix");
    }
    if auth_secret.len() != WEB_PUSH_AUTH_SECRET_LEN {
        bail!("invalid auth_secret length");
    }

    // PRK_key = HMAC-SHA-256(auth_secret, ecdh_secret)
    let prk_key = Salt::new(HKDF_SHA256, auth_secret).extract(ecdh_secret);

    // key_info = "WebPush: info" || 0x00 || ua_public || as_public
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);

    let mut ikm = [0u8; 32];
    hkdf_expand_exact(&prk_key, &key_info, &mut ikm)?;

    // PRK = HMAC-SHA-256(salt, IKM)
    let prk = Salt::new(HKDF_SHA256, salt).extract(&ikm);

    let mut cek = [0u8; 16];
    hkdf_expand_exact(&prk, b"Content-Encoding: aes128gcm\0", &mut cek)?;

    let mut nonce = [0u8; 12];
    hkdf_expand_exact(&prk, b"Content-Encoding: nonce\0", &mut nonce)?;

    Ok((cek, nonce))
}

fn seal_aes128gcm_single_record(
    ecdh_secret: &[u8],
    auth_secret: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8; 16],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    if plaintext.len() > MAX_WEB_PUSH_PLAINTEXT_BYTES {
        bail!(
            "web push payload ({} bytes) exceeds {} byte single-record limit",
            plaintext.len(),
            MAX_WEB_PUSH_PLAINTEXT_BYTES
        );
    }
    let (cek, nonce_bytes) =
        derive_rfc8291_cek_and_nonce(ecdh_secret, auth_secret, ua_public, as_public, salt)?;

    // RFC 8188 §2.1 header: salt(16) || rs(4, 4096) || idlen(1, 65) || keyid(as_public, 65)
    let mut out = Vec::with_capacity(AES128GCM_HEADER_LEN + plaintext.len() + 1 + 16);
    out.extend_from_slice(salt);
    out.extend_from_slice(&AES128GCM_RECORD_SIZE.to_be_bytes());
    out.push(P256_UNCOMPRESSED_PUBKEY_LEN as u8);
    out.extend_from_slice(as_public);
    debug_assert_eq!(out.len(), AES128GCM_HEADER_LEN);

    // Single final record: payload || 0x02 delimiter
    let record_start = out.len();
    out.extend_from_slice(plaintext);
    out.push(0x02);

    let unbound = UnboundKey::new(&AES_128_GCM, &cek)
        .map_err(|_| anyhow!("AES-128-GCM key init failed"))?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let tag = key
        .seal_in_place_separate_tag(nonce, Aad::empty(), &mut out[record_start..])
        .map_err(|_| anyhow!("AES-128-GCM encryption failed"))?;
    out.extend_from_slice(tag.as_ref());
    Ok(out)
}

/// Encrypts `plaintext` into a complete RFC 8291 `Content-Encoding: aes128gcm` HTTP body.
pub fn encrypt_web_push_aes128gcm(
    ua_public_b64url: &str,
    auth_b64url: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let ua_public = decode_flexible_b64(ua_public_b64url).context("invalid subscriber p256dh")?;
    let auth_secret = decode_flexible_b64(auth_b64url).context("invalid subscriber auth")?;
    if ua_public.len() != P256_UNCOMPRESSED_PUBKEY_LEN || ua_public[0] != 0x04 {
        bail!("subscriber p256dh must be 65-byte uncompressed P-256 public key");
    }
    if auth_secret.len() != WEB_PUSH_AUTH_SECRET_LEN {
        bail!("subscriber auth secret must be 16 bytes");
    }

    let rng = SystemRandom::new();
    let mut salt = [0u8; 16];
    rng.fill(&mut salt)
        .map_err(|_| anyhow!("CSPRNG salt generation failed"))?;

    let as_private = EphemeralPrivateKey::generate(&ECDH_P256, &rng)
        .map_err(|_| anyhow!("ephemeral ECDH P-256 key generation failed"))?;
    let as_public = as_private
        .compute_public_key()
        .map_err(|_| anyhow!("ephemeral ECDH P-256 public key computation failed"))?;

    let peer_pub = UnparsedPublicKey::new(&ECDH_P256, &ua_public);
    let ecdh_secret = agree_ephemeral(as_private, &peer_pub, |shared| shared.to_vec())
        .map_err(|_| anyhow!("ECDH P-256 key agreement with subscriber p256dh failed"))?;

    seal_aes128gcm_single_record(
        &ecdh_secret,
        &auth_secret,
        &ua_public,
        as_public.as_ref(),
        &salt,
        plaintext,
    )
}

/// Decrypts an RFC 8291 `aes128gcm` body given the subscriber's ephemeral P-256 private key,
/// 65-byte uncompressed public key, and 16-byte `auth` secret (used in unit & e2e verification).
pub fn decrypt_web_push_aes128gcm(
    ua_private: EphemeralPrivateKey,
    ua_public: &[u8],
    auth_secret: &[u8],
    body: &[u8],
) -> Result<Vec<u8>> {
    if body.len() < AES128GCM_HEADER_LEN + 1 + 16 {
        bail!("aes128gcm body shorter than minimum header + delimiter + tag");
    }
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&body[0..16]);
    let rs = u32::from_be_bytes([body[16], body[17], body[18], body[19]]);
    if rs < 18 {
        bail!("invalid aes128gcm record size rs={rs}");
    }
    let idlen = body[20] as usize;
    if idlen != P256_UNCOMPRESSED_PUBKEY_LEN {
        bail!("unexpected aes128gcm keyid length {idlen} (expected 65)");
    }
    let as_public = &body[21..86];
    let ciphertext = &body[86..];

    let peer_pub = UnparsedPublicKey::new(&ECDH_P256, as_public);
    let ecdh_secret = agree_ephemeral(ua_private, &peer_pub, |shared| shared.to_vec())
        .map_err(|_| anyhow!("subscriber ECDH P-256 agreement failed"))?;

    open_aes128gcm_single_record(
        &ecdh_secret,
        auth_secret,
        ua_public,
        as_public,
        &salt,
        ciphertext,
    )
}

fn open_aes128gcm_single_record(
    ecdh_secret: &[u8],
    auth_secret: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8; 16],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let (cek, nonce_bytes) =
        derive_rfc8291_cek_and_nonce(ecdh_secret, auth_secret, ua_public, as_public, salt)?;
    let unbound = UnboundKey::new(&AES_128_GCM, &cek)
        .map_err(|_| anyhow!("AES-128-GCM key init failed"))?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut buf = ciphertext.to_vec();
    let plain_with_pad = key
        .open_in_place(nonce, Aad::empty(), &mut buf)
        .map_err(|_| anyhow!("AES-128-GCM authentication/decryption failed"))?;

    // Strip optional trailing 0x00 padding bytes and verify final-record 0x02 delimiter.
    let mut end = plain_with_pad.len();
    while end > 0 && plain_with_pad[end - 1] == 0x00 {
        end -= 1;
    }
    if end == 0 || plain_with_pad[end - 1] != 0x02 {
        bail!("missing RFC 8188 final-record 0x02 delimiter");
    }
    Ok(plain_with_pad[..end - 1].to_vec())
}

async fn build_ssrf_pinned_push_client(
    fallback_client: &reqwest::Client,
    endpoint: &str,
    allow_local_test: bool,
) -> std::result::Result<reqwest::Client, String> {
    let valid_url = validate_web_push_endpoint(endpoint, allow_local_test)
        .map_err(|e| format!("web_push: {e}"))?;
    let parsed = reqwest::Url::parse(&valid_url)
        .map_err(|e| format!("web_push: invalid URL ({e})"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "web_push: missing host".to_string())?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = parsed
        .port_or_known_default()
        .unwrap_or_else(|| if parsed.scheme() == "http" { 80 } else { 443 });
    let is_local_test_loopback = allow_local_test
        && matches!(parsed.scheme(), "http" | "https")
        && matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost");

    if is_local_test_loopback {
        return Ok(fallback_client.clone());
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if crate::is_blocked_ssrf_ip(ip) {
            return Err(format!("web_push: literal IP {ip} blocked by SSRF guard"));
        }
        return reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .no_proxy()
            .user_agent("coda-server-webpush")
            .build()
            .map_err(|e| format!("web_push: client build failed ({e})"));
    }

    let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("web_push: DNS lookup failed ({e})"))?
        .collect();
    if resolved.is_empty() {
        return Err("web_push: DNS returned no addresses".into());
    }
    for sa in &resolved {
        if crate::is_blocked_ssrf_ip(sa.ip()) {
            return Err(format!(
                "web_push: resolved IP {} blocked by SSRF guard",
                sa.ip()
            ));
        }
    }

    reqwest::Client::builder()
        .resolve_to_addrs(&host, &resolved)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(8))
        .no_proxy()
        .user_agent("coda-server-webpush")
        .build()
        .map_err(|e| format!("web_push: client build failed ({e})"))
}

pub async fn send_web_push_notification(
    service: &WebPushService,
    fallback_client: &reqwest::Client,
    sub: &WebPushSubscriptionRecord,
    payload_json: &Value,
    allow_local_test: bool,
) -> WebPushDeliveryOutcome {
    if !service.is_enabled() {
        return WebPushDeliveryOutcome::Failed("web_push: VAPID service disabled".into());
    }
    let client =
        match build_ssrf_pinned_push_client(fallback_client, &sub.endpoint, allow_local_test).await
        {
            Ok(c) => c,
            Err(e) => return WebPushDeliveryOutcome::Failed(e),
        };
    let plaintext = match serde_json::to_vec(payload_json) {
        Ok(b) => b,
        Err(e) => {
            return WebPushDeliveryOutcome::Failed(format!(
                "web_push: payload serialize failed ({e})"
            ))
        }
    };
    let body = match encrypt_web_push_aes128gcm(&sub.p256dh_b64url, &sub.auth_b64url, &plaintext) {
        Ok(b) => b,
        Err(e) => return WebPushDeliveryOutcome::Failed(format!("web_push: encrypt failed ({e})")),
    };
    let now_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let auth_hdr = match service.build_vapid_authorization(&sub.endpoint, now_secs) {
        Ok(h) => h,
        Err(e) => return WebPushDeliveryOutcome::Failed(format!("web_push: VAPID failed ({e})")),
    };

    let resp = match client
        .post(&sub.endpoint)
        .header(reqwest::header::AUTHORIZATION, auth_hdr)
        .header(reqwest::header::CONTENT_ENCODING, "aes128gcm")
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header("TTL", "3600")
        .header("Urgency", "high")
        .body(body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return WebPushDeliveryOutcome::Failed(format!("web_push: request failed ({e})")),
    };

    let code = resp.status().as_u16();
    match code {
        200 | 201 | 202 => WebPushDeliveryOutcome::Delivered(code),
        404 | 410 => WebPushDeliveryOutcome::Gone(code),
        _ => WebPushDeliveryOutcome::Failed(format!("web_push: push endpoint returned HTTP {code}")),
    }
}

#[cfg(test)]
mod tests {
    use ring::signature::{UnparsedPublicKey as VerifyPublicKey, ECDSA_P256_SHA256_FIXED};

    use super::*;

    #[test]
    fn vapid_jwt_and_es256_signature_roundtrip_and_audience_origins() {
        let (svc, pkcs8_b64) = WebPushService::generate_ephemeral().unwrap();
        assert!(svc.is_enabled());
        assert_eq!(
            decode_flexible_b64(svc.public_key_b64url()).unwrap().len(),
            65
        );

        let reloaded =
            WebPushService::from_pkcs8_der(&decode_flexible_b64(&pkcs8_b64).unwrap()).unwrap();
        assert_eq!(reloaded.public_key_b64url(), svc.public_key_b64url());

        assert_eq!(
            endpoint_origin_audience("https://fcm.googleapis.com/fcm/send/abc").unwrap(),
            "https://fcm.googleapis.com"
        );
        assert_eq!(
            endpoint_origin_audience("https://web.push.apple.com:443/Qxyz").unwrap(),
            "https://web.push.apple.com"
        );
        assert_eq!(
            endpoint_origin_audience("http://127.0.0.1:18888/push/sub1").unwrap(),
            "http://127.0.0.1:18888"
        );

        let auth_hdr = svc
            .build_vapid_authorization("https://web.push.apple.com/Qxyz", 1_700_000_000)
            .unwrap();
        let rest = auth_hdr.strip_prefix("vapid t=").unwrap();
        let (jwt, k_param) = rest.split_once(", k=").unwrap();
        assert_eq!(k_param, svc.public_key_b64url());

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let hdr_json: Value =
            serde_json::from_slice(&decode_flexible_b64(parts[0]).unwrap()).unwrap();
        assert_eq!(hdr_json["typ"], "JWT");
        assert_eq!(hdr_json["alg"], "ES256");

        let claims_json: Value =
            serde_json::from_slice(&decode_flexible_b64(parts[1]).unwrap()).unwrap();
        assert_eq!(claims_json["aud"], "https://web.push.apple.com");
        assert_eq!(claims_json["exp"], 1_700_000_000u64 + 43_200);
        assert_eq!(claims_json["sub"], "mailto:noreply@codahub.dev");

        let sig_bytes = decode_flexible_b64(parts[2]).unwrap();
        assert_eq!(sig_bytes.len(), 64);
        let pub_bytes = decode_flexible_b64(k_param).unwrap();
        let verifier = VerifyPublicKey::new(&ECDSA_P256_SHA256_FIXED, &pub_bytes);
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        verifier
            .verify(signing_input.as_bytes(), &sig_bytes)
            .expect("VAPID ES256 signature must verify with k public key");
    }

    #[test]
    fn rfc8291_appendix_a_official_test_vector_matches_byte_for_byte() {
        // Official RFC 8291 Appendix A test vector values:
        let auth_secret = decode_flexible_b64("BTBZMqHH6r4Tts7J_aSIgg").unwrap();
        let ua_public = decode_flexible_b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        )
        .unwrap();
        let as_public = decode_flexible_b64(
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
        )
        .unwrap();
        let ecdh_secret =
            decode_flexible_b64("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs").unwrap();
        let salt_vec = decode_flexible_b64("DGv6ra1nlYgDCS1FRnbzlw").unwrap();
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&salt_vec);

        let (cek, nonce) =
            derive_rfc8291_cek_and_nonce(&ecdh_secret, &auth_secret, &ua_public, &as_public, &salt)
                .unwrap();
        assert_eq!(B64_URL.encode(cek), "oIhVW04MRdy2XN9CiKLxTg");
        assert_eq!(B64_URL.encode(nonce), "4h_95klXJ5E_qnoN");

        let plaintext = b"When I grow up, I want to be a watermelon";
        let body = seal_aes128gcm_single_record(
            &ecdh_secret,
            &auth_secret,
            &ua_public,
            &as_public,
            &salt,
            plaintext,
        )
        .unwrap();
        let expected_body = decode_flexible_b64(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
        )
        .unwrap();
        assert_eq!(body, expected_body);

        let opened = open_aes128gcm_single_record(
            &ecdh_secret,
            &auth_secret,
            &ua_public,
            &as_public,
            &salt,
            &body[AES128GCM_HEADER_LEN..],
        )
        .unwrap();
        assert_eq!(opened, plaintext);
    }

    #[test]
    fn rfc8291_live_ephemeral_encrypt_decrypt_and_tamper_rejection() {
        let rng = SystemRandom::new();
        let ua_priv = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
        let ua_pub = ua_priv.compute_public_key().unwrap();
        let mut auth_secret = [0u8; 16];
        rng.fill(&mut auth_secret).unwrap();

        let (p256dh_b64url, auth_b64url) = validate_and_normalize_subscription_keys(
            &B64_STD.encode(ua_pub.as_ref()),
            &B64_STD.encode(auth_secret),
        )
        .unwrap();

        let payload = br#"{"title":"Coda Alert","body":"Session waiting","url":"/console","tag":"t1"}"#;
        let encrypted = encrypt_web_push_aes128gcm(&p256dh_b64url, &auth_b64url, payload).unwrap();

        // Check RFC 8188 86-byte header invariants
        assert_eq!(
            encrypted.len(),
            AES128GCM_HEADER_LEN + payload.len() + 1 + 16
        );
        assert_eq!(
            u32::from_be_bytes([encrypted[16], encrypted[17], encrypted[18], encrypted[19]]),
            4096
        );
        assert_eq!(encrypted[20], 65);
        assert_eq!(encrypted[21], 0x04);

        // Tampered ciphertext must fail authentication before we consume the genuine key
        let mut tampered = encrypted.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x55;
        let ua_priv_bad = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
        let ua_pub_bad = ua_priv_bad.compute_public_key().unwrap();
        assert!(decrypt_web_push_aes128gcm(
            ua_priv_bad,
            ua_pub_bad.as_ref(),
            &auth_secret,
            &encrypted
        )
        .is_err());

        let decrypted =
            decrypt_web_push_aes128gcm(ua_priv, ua_pub.as_ref(), &auth_secret, &encrypted).unwrap();
        assert_eq!(decrypted, payload);

        // Off-curve P-256 public key must be rejected on subscription validation
        let mut bad_pub = [0u8; 65];
        bad_pub[0] = 0x04;
        bad_pub[1] = 0xFF;
        assert!(validate_and_normalize_subscription_keys(
            &B64_URL.encode(bad_pub),
            &auth_b64url
        )
        .is_err());
    }

    #[test]
    fn endpoint_ssrf_validation_enforces_https_and_blocks_private_ips() {
        assert!(
            validate_web_push_endpoint("https://fcm.googleapis.com/fcm/send/123", false).is_ok()
        );
        assert!(validate_web_push_endpoint("", false).is_err());
        assert!(
            validate_web_push_endpoint("http://fcm.googleapis.com/fcm/send/123", false).is_err()
        );
        assert!(validate_web_push_endpoint("https://127.0.0.1:8443/push", false).is_err());
        assert!(
            validate_web_push_endpoint("https://169.254.169.254/latest/meta-data", false).is_err()
        );
        assert!(
            validate_web_push_endpoint("https://[::ffff:169.254.169.254]/push", false).is_err()
        );
        assert!(validate_web_push_endpoint("http://127.0.0.1:19090/push", true).is_ok());
        assert!(
            validate_web_push_endpoint("http://169.254.169.254/latest/meta-data", true).is_err()
        );
    }
}
