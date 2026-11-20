import { ed25519, x25519 } from "@noble/curves/ed25519";
import { hkdf } from "@noble/hashes/hkdf";
import { sha256 } from "@noble/hashes/sha256";
import { chacha20poly1305 } from "@noble/ciphers/chacha";
import { decompress as zstdDecompress } from "fzstd";

const enc = new TextEncoder();
const dec = new TextDecoder();

export const MAX_E2EE_DECOMPRESSED_BYTES = 16 * 1024 * 1024;
export const MAX_HANDSHAKE_SKEW_MS = 120_000;
export const RELAY_SESSION_NONCE_LEN = 16;
const RELAY_NONCE_TTL_MS = 10 * 60 * 1000;
const RELAY_NONCE_MAX_ENTRIES = 2048;

const memoryStorageFallback = new Map<string, string>();

function tabStorageGet(key: string): string | null {
  try {
    if (typeof sessionStorage !== "undefined") return sessionStorage.getItem(key);
  } catch {
    // ignore storage access errors
  }
  return memoryStorageFallback.get(key) ?? null;
}

function tabStorageSet(key: string, val: string): void {
  try {
    if (typeof sessionStorage !== "undefined") {
      sessionStorage.setItem(key, val);
      return;
    }
  } catch {
    // ignore storage access errors
  }
  memoryStorageFallback.set(key, val);
}

/** Chunked O(n) Base64 encoder safe for large binary buffers without call-stack overflow. */
export function toB64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  const parts: string[] = [];
  for (let i = 0; i < bytes.length; i += CHUNK) {
    parts.push(String.fromCharCode(...bytes.subarray(i, Math.min(i + CHUNK, bytes.length))));
  }
  return btoa(parts.join(""));
}

export function fromB64(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export interface WebDeviceIdentity {
  name: string;
  ed25519SecretB64: string;
  ed25519PublicB64: string;
  x25519SecretB64: string;
  x25519PublicB64: string;
}

export interface RelaySessionAuth {
  session_nonce_b64: string;
  init_ts_ms: number;
  sender_ed25519_pub_b64: string;
  sender_x25519_pub_b64: string;
  supports_zstd: boolean;
  signature_b64: string;
}

export interface EncryptedPayload {
  seq: number;
  compressed: boolean;
  ciphertext_b64: string;
  session_nonce_b64?: string | null;
}

/**
 * Per-tab Web client identity stored in `sessionStorage` so multiple tabs do not share
 * one `client_device_id` and kick each other's Relay bindings.
 */
export function loadOrCreateWebIdentity(): WebDeviceIdentity {
  const cached = tabStorageGet("coda_web_tab_identity_v2");
  if (cached) {
    try {
      const parsed = JSON.parse(cached) as WebDeviceIdentity;
      if (parsed.ed25519SecretB64 && parsed.ed25519PublicB64 && parsed.x25519SecretB64 && parsed.x25519PublicB64) {
        return parsed;
      }
    } catch {
      // regenerate corrupted entry
    }
  }
  const edPriv = ed25519.utils.randomPrivateKey();
  const edPub = ed25519.getPublicKey(edPriv);
  const dhPriv = x25519.utils.randomPrivateKey();
  const dhPub = x25519.getPublicKey(dhPriv);

  const tabTag = Math.random().toString(36).slice(2, 6).toUpperCase();
  const id: WebDeviceIdentity = {
    name: `Coda Web (${tabTag})`,
    ed25519SecretB64: toB64(edPriv),
    ed25519PublicB64: toB64(edPub),
    x25519SecretB64: toB64(dhPriv),
    x25519PublicB64: toB64(dhPub),
  };
  tabStorageSet("coda_web_tab_identity_v2", JSON.stringify(id));
  return id;
}

export function signWebMessage(identity: WebDeviceIdentity, message: string): string {
  const sig = ed25519.sign(enc.encode(message), fromB64(identity.ed25519SecretB64));
  return toB64(sig);
}

export function verifyEd25519Signature(pubB64: string, message: string, sigB64: string): boolean {
  try {
    const pk = fromB64(pubB64);
    const sig = fromB64(sigB64);
    if (pk.length !== 32 || sig.length !== 64) return false;
    return ed25519.verify(sig, enc.encode(message), pk);
  } catch {
    return false;
  }
}

export function isTimestampFresh(tsMs: number, nowMs = Date.now(), maxSkewMs = MAX_HANDSHAKE_SKEW_MS): boolean {
  return Number.isFinite(tsMs) && tsMs > 0 && Math.abs(nowMs - tsMs) <= maxSkewMs;
}

export function generateEphemeralX25519(): { secret: Uint8Array; publicB64: string } {
  const secret = x25519.utils.randomPrivateKey();
  const pub = x25519.getPublicKey(secret);
  return { secret, publicB64: toB64(pub) };
}

export function randomSessionNonceB64(): string {
  const bytes = new Uint8Array(RELAY_SESSION_NONCE_LEN);
  crypto.getRandomValues(bytes);
  return toB64(bytes);
}

export function p2pHelloV2SigningText(
  clientDeviceId: string,
  targetDeviceId: string,
  clientStaticX25519PubB64: string,
  clientEphX25519PubB64: string,
  tsMs: number
): string {
  return `p2p-hello-v2:${clientDeviceId.toLowerCase()}:${targetDeviceId.toLowerCase()}:${clientStaticX25519PubB64}:${clientEphX25519PubB64}:${tsMs}`;
}

export function p2pHelloV2SigningTextWithZstd(
  clientDeviceId: string,
  targetDeviceId: string,
  clientStaticX25519PubB64: string,
  clientEphX25519PubB64: string,
  tsMs: number,
  supportsZstd: boolean
): string {
  return `p2p-hello-v2:${clientDeviceId.toLowerCase()}:${targetDeviceId.toLowerCase()}:${clientStaticX25519PubB64}:${clientEphX25519PubB64}:${tsMs}:zstd=${supportsZstd ? 1 : 0}`;
}

export function p2pAckV2SigningText(
  clientDeviceId: string,
  daemonDeviceId: string,
  clientEphX25519PubB64: string,
  daemonEphX25519PubB64: string,
  tsMs: number
): string {
  return `p2p-ack-v2:${clientDeviceId.toLowerCase()}:${daemonDeviceId.toLowerCase()}:${clientEphX25519PubB64}:${daemonEphX25519PubB64}:${tsMs}`;
}

export function relaySessionAuthSigningText(
  clientDeviceId: string,
  daemonDeviceId: string,
  senderX25519PubB64: string,
  sessionNonceB64: string,
  initTsMs: number
): string {
  return `relay-sess-v2:${clientDeviceId.toLowerCase()}:${daemonDeviceId.toLowerCase()}:${senderX25519PubB64}:${sessionNonceB64}:${initTsMs}`;
}

export function relaySessionAuthSigningTextWithZstd(
  clientDeviceId: string,
  daemonDeviceId: string,
  senderX25519PubB64: string,
  sessionNonceB64: string,
  initTsMs: number,
  supportsZstd: boolean
): string {
  return `relay-sess-v2:${clientDeviceId.toLowerCase()}:${daemonDeviceId.toLowerCase()}:${senderX25519PubB64}:${sessionNonceB64}:${initTsMs}:zstd=${supportsZstd ? 1 : 0}`;
}

/** Bounded 10-minute replay cache for incoming `RelaySessionAuth` session nonces. */
export class RelayNonceReplayCache {
  private seen = new Map<string, number>();

  checkAndRemember(peerId: string, nonceB64: string, nowMs = Date.now()): void {
    for (const [k, ts] of this.seen.entries()) {
      if (nowMs - ts > RELAY_NONCE_TTL_MS) {
        this.seen.delete(k);
      }
    }
    const key = `${peerId.toLowerCase()}:${nonceB64}`;
    if (this.seen.has(key)) {
      throw new Error("replayed relay session auth nonce");
    }
    if (this.seen.size >= RELAY_NONCE_MAX_ENTRIES) {
      const oldest = this.seen.keys().next().value;
      if (oldest !== undefined) this.seen.delete(oldest);
    }
    this.seen.set(key, nowMs);
  }

  clear(): void {
    this.seen.clear();
  }
}

export function createRelaySessionAuth(
  identity: WebDeviceIdentity,
  clientDeviceId: string,
  dstDaemonId: string,
  nowMs = Date.now(),
  supportsZstd = true
): RelaySessionAuth {
  const session_nonce_b64 = randomSessionNonceB64();
  const payload = relaySessionAuthSigningTextWithZstd(
    clientDeviceId,
    dstDaemonId,
    identity.x25519PublicB64,
    session_nonce_b64,
    nowMs,
    supportsZstd
  );
  return {
    session_nonce_b64,
    init_ts_ms: nowMs,
    sender_ed25519_pub_b64: identity.ed25519PublicB64,
    sender_x25519_pub_b64: identity.x25519PublicB64,
    supports_zstd: supportsZstd,
    signature_b64: signWebMessage(identity, payload),
  };
}

export function verifyRelaySessionAuth(
  auth: RelaySessionAuth,
  clientDeviceId: string,
  daemonDeviceId: string,
  expectedEd25519PubB64: string,
  expectedX25519PubB64: string,
  nowMs = Date.now(),
  maxSkewMs = MAX_HANDSHAKE_SKEW_MS
): Uint8Array {
  const actualEd = fromB64(auth.sender_ed25519_pub_b64);
  const expEd = fromB64(expectedEd25519PubB64);
  if (actualEd.length !== 32 || expEd.length !== 32 || !bytesEqual(actualEd, expEd)) {
    throw new Error("relay session auth Ed25519 key mismatch");
  }
  const actualX = fromB64(auth.sender_x25519_pub_b64);
  const expX = fromB64(expectedX25519PubB64);
  if (actualX.length !== 32 || expX.length !== 32 || !bytesEqual(actualX, expX)) {
    throw new Error("relay session auth X25519 key mismatch");
  }
  if (!isTimestampFresh(auth.init_ts_ms, nowMs, maxSkewMs)) {
    throw new Error("relay session auth timestamp out of window");
  }
  const nonceRaw = fromB64(auth.session_nonce_b64);
  if (nonceRaw.length !== RELAY_SESSION_NONCE_LEN) {
    throw new Error("invalid relay session nonce length");
  }
  const msgWithZstd = relaySessionAuthSigningTextWithZstd(
    clientDeviceId,
    daemonDeviceId,
    auth.sender_x25519_pub_b64,
    auth.session_nonce_b64,
    auth.init_ts_ms,
    Boolean(auth.supports_zstd)
  );
  const msgLegacy = relaySessionAuthSigningText(
    clientDeviceId,
    daemonDeviceId,
    auth.sender_x25519_pub_b64,
    auth.session_nonce_b64,
    auth.init_ts_ms
  );
  if (
    !verifyEd25519Signature(expectedEd25519PubB64, msgWithZstd, auth.signature_b64) &&
    !verifyEd25519Signature(expectedEd25519PubB64, msgLegacy, auth.signature_b64)
  ) {
    throw new Error("relay session auth signature verification failed");
  }
  return nonceRaw;
}

function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

function concatBytes(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function x25519DhChecked(mySecret: Uint8Array, peerPublicB64: string, label: string): { shared: Uint8Array; peerRaw: Uint8Array } {
  const peerRaw = fromB64(peerPublicB64);
  if (peerRaw.length !== 32 || mySecret.length !== 32) {
    throw new Error(`invalid ${label} key length`);
  }
  const shared = x25519.getSharedSecret(mySecret, peerRaw);
  let nonZero = 0;
  for (let i = 0; i < shared.length; i++) nonZero |= shared[i];
  if (shared.length !== 32 || nonZero === 0) {
    throw new Error(`non-contributory (low-order) ${label}`);
  }
  return { shared, peerRaw };
}

/**
 * 64-bit sliding anti-replay window matching `crates/coda-protocol::ReplayWindow`.
 * Call `check(seq)` before AEAD decryption and `commit(seq)` ONLY after AEAD succeeds.
 */
export class ReplayWindow {
  private highestSeq = 0n;
  private bitmap = 0n;

  check(seqNum: number): void {
    if (!Number.isSafeInteger(seqNum) || seqNum <= 0) {
      throw new Error(`invalid E2EE sequence number seq=${seqNum}`);
    }
    const seq = BigInt(seqNum);
    if (this.highestSeq > 0n && seq <= this.highestSeq) {
      const diff = this.highestSeq - seq;
      if (diff >= 64n) {
        throw new Error(`stale E2EE frame seq=${seqNum} below window highest=${this.highestSeq}`);
      }
      if ((this.bitmap & (1n << diff)) !== 0n) {
        throw new Error(`duplicate E2EE frame seq=${seqNum}`);
      }
    }
  }

  commit(seqNum: number): void {
    const seq = BigInt(seqNum);
    const MASK64 = (1n << 64n) - 1n;
    if (this.highestSeq === 0n) {
      this.highestSeq = seq;
      this.bitmap = 1n;
    } else if (seq > this.highestSeq) {
      const shift = seq - this.highestSeq;
      this.bitmap = shift >= 64n ? 1n : (((this.bitmap << shift) | 1n) & MASK64);
      this.highestSeq = seq;
    } else {
      const diff = this.highestSeq - seq;
      if (diff < 64n) {
        this.bitmap = (this.bitmap | (1n << diff)) & MASK64;
      }
    }
  }
}

/**
 * Inspects RFC 8878 zstd frame headers before calling `fzstd` to fail closed on zip-bombs
 * declaring >16 MiB decompressed sizes or gigantic windows, and bounds actual output length.
 */
export function decompressZstdBounded(compressed: Uint8Array, maxBytes = MAX_E2EE_DECOMPRESSED_BYTES): Uint8Array {
  if (compressed.length < 6 || compressed.length > maxBytes) {
    throw new Error("zstd frame length out of bounds");
  }
  // Standard zstd frame magic: 0xFD2FB528 little-endian [0x28, 0xb5, 0x2f, 0xfd]
  if (
    compressed[0] !== 0x28 ||
    compressed[1] !== 0xb5 ||
    compressed[2] !== 0x2f ||
    compressed[3] !== 0xfd
  ) {
    throw new Error("invalid zstd magic header");
  }
  const fhd = compressed[4];
  const fcsFlag = (fhd >> 6) & 0x03;
  const singleSegment = (fhd & 0x20) !== 0;
  const reservedBit = (fhd & 0x08) !== 0;
  if (reservedBit) {
    throw new Error("invalid zstd reserved header bit");
  }
  const dictIdFlag = fhd & 0x03;
  const windowDescSize = singleSegment ? 0 : 1;
  const dictIdSize = dictIdFlag === 0 ? 0 : dictIdFlag === 1 ? 1 : dictIdFlag === 2 ? 2 : 4;
  const fcsSize = fcsFlag === 0 ? (singleSegment ? 1 : 0) : fcsFlag === 1 ? 2 : fcsFlag === 2 ? 4 : 8;

  if (!singleSegment && compressed.length >= 6) {
    const winByte = compressed[5];
    const exp = winByte >> 3;
    const mantissa = winByte & 0x07;
    const winBase = 1n << BigInt(10 + exp);
    const windowSize = winBase + (winBase >> 3n) * BigInt(mantissa);
    if (windowSize > BigInt(Math.max(maxBytes * 2, 32 * 1024 * 1024))) {
      throw new Error("zstd window size exceeds bounded limit");
    }
  }

  const fcsOffset = 5 + windowDescSize + dictIdSize;
  if (fcsSize > 0) {
    if (compressed.length < fcsOffset + fcsSize) {
      throw new Error("truncated zstd frame content size header");
    }
    let fcs = 0n;
    for (let i = 0; i < fcsSize; i++) {
      fcs |= BigInt(compressed[fcsOffset + i]) << BigInt(i * 8);
    }
    if (fcsSize === 2) fcs += 256n;
    if (fcs > BigInt(maxBytes)) {
      throw new Error(`zstd declared frame size (${fcs}) exceeds ${maxBytes} byte limit`);
    }
  }

  const out = zstdDecompress(compressed);
  if (out.byteLength > maxBytes) {
    throw new Error(`zstd decompressed output (${out.byteLength} bytes) exceeds ${maxBytes} byte limit`);
  }
  return out;
}

/**
 * Browser-side Zero-Knowledge E2EE Session matching `crates/coda-protocol` & Swift `CryptoKit` byte-for-byte.
 * Supports DirectP2P v2 (static + ephemeral hybrid X25519 + transcript AAD) and Relay v2 (salted nonce + device IDs).
 */
export class BrowserE2EESession {
  private txKey: Uint8Array;
  private rxKey: Uint8Array;
  private txDirTag: number;
  private rxDirTag: number;
  private nextTxSeq = 1;
  private rxWindow = new ReplayWindow();
  private aadBinding: Uint8Array | null;
  public readonly sessionNonceB64: string | null;

  private constructor(
    txKey: Uint8Array,
    rxKey: Uint8Array,
    txDirTag: number,
    rxDirTag: number,
    aadBinding: Uint8Array | null,
    sessionNonceB64: string | null
  ) {
    this.txKey = txKey;
    this.rxKey = rxKey;
    this.txDirTag = txDirTag;
    this.rxDirTag = rxDirTag;
    this.aadBinding = aadBinding;
    this.sessionNonceB64 = sessionNonceB64;
  }

  /** Legacy static-key session derivation (prefer `deriveDirectP2Pv2` or `deriveRelayV2`). */
  static deriveLegacy(myX25519SecretB64: string, peerX25519PublicB64: string, isInitiator = true): BrowserE2EESession {
    const { shared } = x25519DhChecked(fromB64(myX25519SecretB64), peerX25519PublicB64, "peer x25519 public key");
    const salt = enc.encode("coda-zk-mesh-salt-v1");
    const c2d = hkdf(sha256, shared, salt, enc.encode("coda-client-to-daemon-aead"), 32);
    const d2c = hkdf(sha256, shared, salt, enc.encode("coda-daemon-to-client-aead"), 32);
    return isInitiator
      ? new BrowserE2EESession(c2d, d2c, 0x01, 0x02, null, null)
      : new BrowserE2EESession(d2c, c2d, 0x02, 0x01, null, null);
  }

  /** Direct P2P v2 hybrid static+ephemeral X25519 derivation matching `E2eeSession::derive_ephemeral_p2p`. */
  static deriveDirectP2Pv2(params: {
    myStaticSecretB64: string;
    peerStaticPublicB64: string;
    myEphemeralSecret: Uint8Array;
    peerEphemeralPublicB64: string;
    clientDeviceId: string;
    daemonDeviceId: string;
    isInitiator?: boolean;
  }): BrowserE2EESession {
    const isInitiator = params.isInitiator ?? true;
    const { shared: staticShared } = x25519DhChecked(
      fromB64(params.myStaticSecretB64),
      params.peerStaticPublicB64,
      "peer static x25519 public key"
    );
    const { shared: ephShared, peerRaw: peerEphRaw } = x25519DhChecked(
      params.myEphemeralSecret,
      params.peerEphemeralPublicB64,
      "peer ephemeral x25519 public key"
    );
    const myEphRaw = x25519.getPublicKey(params.myEphemeralSecret);
    const clientEphRaw = isInitiator ? myEphRaw : peerEphRaw;
    const daemonEphRaw = isInitiator ? peerEphRaw : myEphRaw;

    const salt = sha256(concatBytes(clientEphRaw, daemonEphRaw));
    const ikm = concatBytes(staticShared, ephShared);
    const cStr = params.clientDeviceId.toLowerCase();
    const dStr = params.daemonDeviceId.toLowerCase();
    const c2d = hkdf(sha256, ikm, salt, enc.encode(`coda-p2p-v2-c2d:${cStr}:${dStr}`), 32);
    const d2c = hkdf(sha256, ikm, salt, enc.encode(`coda-p2p-v2-d2c:${cStr}:${dStr}`), 32);

    return isInitiator
      ? new BrowserE2EESession(c2d, d2c, 0x01, 0x02, salt.slice(0, 16), null)
      : new BrowserE2EESession(d2c, c2d, 0x02, 0x01, salt.slice(0, 16), null);
  }

  /** Relay v2 salted X25519 session derivation matching `E2eeSession::derive_relay_session`. */
  static deriveRelayV2(params: {
    myStaticSecretB64: string;
    peerStaticPublicB64: string;
    sessionNonceB64: string;
    clientDeviceId: string;
    daemonDeviceId: string;
    isInitiator?: boolean;
  }): BrowserE2EESession {
    const isInitiator = params.isInitiator ?? true;
    const nonceRaw = fromB64(params.sessionNonceB64);
    if (nonceRaw.length !== RELAY_SESSION_NONCE_LEN) {
      throw new Error("invalid relay session nonce length (expected 16 bytes)");
    }
    const canonicalNonceB64 = toB64(nonceRaw);
    const { shared: staticShared } = x25519DhChecked(
      fromB64(params.myStaticSecretB64),
      params.peerStaticPublicB64,
      "peer static x25519 public key"
    );
    const cStr = params.clientDeviceId.toLowerCase();
    const dStr = params.daemonDeviceId.toLowerCase();
    const c2d = hkdf(sha256, staticShared, nonceRaw, enc.encode(`coda-relay-v2-c2d:${cStr}:${dStr}`), 32);
    const d2c = hkdf(sha256, staticShared, nonceRaw, enc.encode(`coda-relay-v2-d2c:${cStr}:${dStr}`), 32);

    return isInitiator
      ? new BrowserE2EESession(c2d, d2c, 0x01, 0x02, nonceRaw, canonicalNonceB64)
      : new BrowserE2EESession(d2c, c2d, 0x02, 0x01, nonceRaw, canonicalNonceB64);
  }

  private makeNonce(dirTag: number, seq: number): Uint8Array {
    const n = new Uint8Array(12);
    n[0] = dirTag;
    const view = new DataView(n.buffer);
    view.setBigUint64(4, BigInt(seq), false);
    return n;
  }

  private makeAad(compressed: boolean, dirTag: number): Uint8Array {
    const cByte = compressed ? 1 : 0;
    if (!this.aadBinding) {
      return new Uint8Array([cByte]);
    }
    const aad = new Uint8Array(2 + this.aadBinding.length);
    aad[0] = cByte;
    aad[1] = dirTag;
    aad.set(this.aadBinding, 2);
    return aad;
  }

  sealMessage(msg: unknown): EncryptedPayload {
    const seq = this.nextTxSeq++;
    const nonce = this.makeNonce(this.txDirTag, seq);
    const aad = this.makeAad(false, this.txDirTag);
    const pt = enc.encode(JSON.stringify(msg));
    const ct = chacha20poly1305(this.txKey, nonce, aad).encrypt(pt);
    const out: EncryptedPayload = {
      seq,
      compressed: false,
      ciphertext_b64: toB64(ct),
    };
    if (this.sessionNonceB64 !== null) {
      out.session_nonce_b64 = this.sessionNonceB64;
    }
    return out;
  }

  openMessage(payload: EncryptedPayload): any {
    if (this.sessionNonceB64 !== null && payload.session_nonce_b64 !== this.sessionNonceB64) {
      throw new Error("relay session nonce mismatch");
    }
    this.rxWindow.check(payload.seq);
    const nonce = this.makeNonce(this.rxDirTag, payload.seq);
    const aad = this.makeAad(Boolean(payload.compressed), this.rxDirTag);
    const rawPt = chacha20poly1305(this.rxKey, nonce, aad).decrypt(fromB64(payload.ciphertext_b64));
    const jsonBytes = payload.compressed
      ? decompressZstdBounded(rawPt, MAX_E2EE_DECOMPRESSED_BYTES)
      : rawPt;
    if (jsonBytes.byteLength > MAX_E2EE_DECOMPRESSED_BYTES) {
      throw new Error("decrypted E2EE payload exceeds 16 MiB limit");
    }
    const parsed = JSON.parse(dec.decode(jsonBytes));
    // Commit sequence ONLY after AEAD authentication, decompression, and JSON decode succeed.
    this.rxWindow.commit(payload.seq);
    return parsed;
  }
}
