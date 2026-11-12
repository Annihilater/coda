import { ed25519, x25519 } from "@noble/curves/ed25519";
import { hkdf } from "@noble/hashes/hkdf";
import { sha256 } from "@noble/hashes/sha256";
import { chacha20poly1305 } from "@noble/ciphers/chacha";
import { decompress as zstdDecompress } from "fzstd";

const enc = new TextEncoder();
const dec = new TextDecoder();

export function toB64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
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

export function loadOrCreateWebIdentity(): WebDeviceIdentity {
  const cached = localStorage.getItem("coda_web_identity_v1");
  if (cached) {
    try {
      return JSON.parse(cached);
    } catch {}
  }
  const edPriv = ed25519.utils.randomPrivateKey();
  const edPub = ed25519.getPublicKey(edPriv);
  const dhPriv = x25519.utils.randomPrivateKey();
  const dhPub = x25519.getPublicKey(dhPriv);

  const id: WebDeviceIdentity = {
    name: "Coda Web Console",
    ed25519SecretB64: toB64(edPriv),
    ed25519PublicB64: toB64(edPub),
    x25519SecretB64: toB64(dhPriv),
    x25519PublicB64: toB64(dhPub),
  };
  localStorage.setItem("coda_web_identity_v1", JSON.stringify(id));
  return id;
}

/**
 * Browser-side Zero-Knowledge E2EE Session matching `crates/coda-protocol` & Swift `CryptoKit` byte-for-byte.
 */
export class BrowserE2EESession {
  private txKey: Uint8Array;
  private rxKey: Uint8Array;
  private nextTxSeq = 1;

  constructor(myX25519SecretB64: string, peerX25519PublicB64: string) {
    const shared = x25519.getSharedSecret(fromB64(myX25519SecretB64), fromB64(peerX25519PublicB64));
    const salt = enc.encode("coda-zk-mesh-salt-v1");
    this.txKey = hkdf(sha256, shared, salt, enc.encode("coda-client-to-daemon-aead"), 32);
    this.rxKey = hkdf(sha256, shared, salt, enc.encode("coda-daemon-to-client-aead"), 32);
  }

  private makeNonce(dirTag: number, seq: number): Uint8Array {
    const n = new Uint8Array(12);
    n[0] = dirTag;
    const view = new DataView(n.buffer);
    view.setBigUint64(4, BigInt(seq), false);
    return n;
  }

  sealMessage(msg: unknown): { seq: number; compressed: boolean; ciphertext_b64: string } {
    const seq = this.nextTxSeq++;
    const nonce = this.makeNonce(0x01, seq);
    const aad = new Uint8Array([0]);
    const pt = enc.encode(JSON.stringify(msg));
    const ct = chacha20poly1305(this.txKey, nonce, aad).encrypt(pt);
    return {
      seq,
      compressed: false,
      ciphertext_b64: toB64(ct),
    };
  }

  openMessage(payload: { seq: number; compressed: boolean; ciphertext_b64: string }): any {
    const nonce = this.makeNonce(0x02, payload.seq);
    const aad = new Uint8Array([payload.compressed ? 1 : 0]);
    const rawPt = chacha20poly1305(this.rxKey, nonce, aad).decrypt(fromB64(payload.ciphertext_b64));
    const jsonBytes = payload.compressed ? zstdDecompress(rawPt) : rawPt;
    return JSON.parse(dec.decode(jsonBytes));
  }
}
