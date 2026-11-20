import { describe, expect, test } from "bun:test";
import { ed25519, x25519 } from "@noble/curves/ed25519";
import {
  BrowserE2EESession,
  MAX_HANDSHAKE_SKEW_MS,
  RelayNonceReplayCache,
  ReplayWindow,
  type WebDeviceIdentity,
  createRelaySessionAuth,
  decompressZstdBounded,
  fromB64,
  generateEphemeralX25519,
  loadOrCreateWebIdentity,
  p2pAckV2SigningText,
  p2pHelloV2SigningText,
  p2pHelloV2SigningTextWithZstd,
  signWebMessage,
  toB64,
  verifyEd25519Signature,
  verifyRelaySessionAuth,
} from "./crypto";
import { WebVtTerminal, escapeHtml, safeHref } from "./vt";

function makeTestIdentity(name: string): WebDeviceIdentity {
  const edPriv = ed25519.utils.randomPrivateKey();
  const edPub = ed25519.getPublicKey(edPriv);
  const dhPriv = x25519.utils.randomPrivateKey();
  const dhPub = x25519.getPublicKey(dhPriv);
  return {
    name,
    ed25519SecretB64: toB64(edPriv),
    ed25519PublicB64: toB64(edPub),
    x25519SecretB64: toB64(dhPriv),
    x25519PublicB64: toB64(dhPub),
  };
}

describe("web XSS & URL sanitization", () => {
  test("escapeHtml neutralizes tags and attribute quotes", () => {
    const raw = `<img src="x" onerror='alert(1)'> & "quoted"`;
    const esc = escapeHtml(raw);
    expect(esc).toBe("&lt;img src=&quot;x&quot; onerror=&#39;alert(1)&#39;&gt; &amp; &quot;quoted&quot;");
    expect(esc.includes("<")).toBe(false);
    expect(esc.includes('"')).toBe(false);
  });

  test("safeHref allows safe schemes and blocks javascript/data/protocol-relative", () => {
    expect(safeHref("https://example.com/app.dmg")).toBe("https://example.com/app.dmg");
    expect(safeHref("http://127.0.0.1:24444/ws")).toBe("http://127.0.0.1:24444/ws");
    expect(safeHref("/install.sh")).toBe("/install.sh");
    expect(safeHref("#downloads")).toBe("#downloads");

    expect(safeHref("javascript:alert(1)")).toBe("#");
    expect(safeHref("  JaVaScRiPt:alert(document.cookie)")).toBe("#");
    expect(safeHref("data:text/html,<script>alert(1)</script>")).toBe("#");
    expect(safeHref("//evil.example.com/payload.js")).toBe("#");
    expect(safeHref("")).toBe("#");
    expect(safeHref(null)).toBe("#");
  });
});

describe("web crypto helpers, ReplayWindow, Zstd bounds & E2EE v2", () => {
  test("toB64 and fromB64 round-trip large (>128 KiB) binary buffers without stack overflow", () => {
    const src = new Uint8Array(192 * 1024);
    for (let i = 0; i < src.length; i += 1) {
      src[i] = (i * 131 + 17) & 0xff;
    }
    const b64 = toB64(src);
    const dst = fromB64(b64);
    expect(dst.length).toBe(src.length);
    expect(dst[0]).toBe(src[0]);
    expect(dst[dst.length - 1]).toBe(src[src.length - 1]);
  });

  test("loadOrCreateWebIdentity persists singleton identity across calls", () => {
    const first = loadOrCreateWebIdentity();
    const second = loadOrCreateWebIdentity();
    expect(first.ed25519PublicB64).toBe(second.ed25519PublicB64);
    expect(first.x25519PublicB64).toBe(second.x25519PublicB64);
  });

  test("ReplayWindow accepts out-of-order packets and rejects duplicates/stale/zero sequences", () => {
    const win = new ReplayWindow();
    expect(() => win.check(0)).toThrow(/invalid/i);
    expect(() => win.check(-1)).toThrow(/invalid/i);

    win.check(1);
    win.commit(1);

    // Out-of-order within 64-packet window
    win.check(4);
    win.commit(4);
    win.check(2);
    win.commit(2);
    win.check(3);
    win.commit(3);

    // Replay rejected
    expect(() => win.check(2)).toThrow(/duplicate/i);
    expect(() => win.check(4)).toThrow(/duplicate/i);

    // Slide window past seq=1
    win.check(100);
    win.commit(100);
    expect(() => win.check(10)).toThrow(/stale/i);
    win.check(99);
    win.commit(99);
    expect(() => win.check(99)).toThrow(/duplicate/i);
  });

  test("decompressZstdBounded decompresses valid frames and rejects oversized frame headers", () => {
    // Valid RFC 8878 single-segment raw zstd frame containing "hello":
    // Magic: 28 b5 2f fd, FHD: 20 (single segment, 1-byte FCS), FCS: 05,
    // Block header (last=1, type=0 raw, size=5): (5 << 3) | 1 = 0x29 => 29 00 00, payload: "hello"
    const validFrame = new Uint8Array([
      0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x05, 0x29, 0x00, 0x00, 0x68, 0x65, 0x6c, 0x6c, 0x6f,
    ]);
    const out = decompressZstdBounded(validFrame);
    expect(new TextDecoder().decode(out)).toBe("hello");

    // Crafted frame header advertising 32 MiB uncompressed size
    const oversizedHeader = new Uint8Array([
      0x28, 0xb5, 0x2f, 0xfd, 0xa0, 0x00, 0x00, 0x00, 0x02, 0x01, 0x00, 0x00,
    ]);
    expect(() => decompressZstdBounded(oversizedHeader)).toThrow(/limit|16|Zstd|zstd/i);
  });

  test("DirectP2P v2 signatures, contributory check, directional AEAD & post-AEAD replay protection", () => {
    const alice = makeTestIdentity("alice");
    const bob = makeTestIdentity("bob");

    const ephA = generateEphemeralX25519();
    const ephB = generateEphemeralX25519();
    const now = Date.now();

    const helloText = p2pHelloV2SigningText(
      "client-1",
      "daemon-1",
      alice.x25519PublicB64,
      ephA.publicB64,
      now
    );
    const helloSig = signWebMessage(alice, helloText);
    expect(verifyEd25519Signature(alice.ed25519PublicB64, helloText, helloSig)).toBe(true);
    expect(verifyEd25519Signature(bob.ed25519PublicB64, helloText, helloSig)).toBe(false);

    const ackText = p2pAckV2SigningText("client-1", "daemon-1", ephA.publicB64, ephB.publicB64, now);
    const ackSig = signWebMessage(bob, ackText);
    expect(verifyEd25519Signature(bob.ed25519PublicB64, ackText, ackSig)).toBe(true);

    // Contributory X25519 check rejects low-order all-zero public key
    const zeroPubB64 = toB64(new Uint8Array(32));
    expect(() =>
      BrowserE2EESession.deriveDirectP2Pv2({
        myStaticSecretB64: alice.x25519SecretB64,
        peerStaticPublicB64: bob.x25519PublicB64,
        myEphemeralSecret: ephA.secret,
        peerEphemeralPublicB64: zeroPubB64,
        clientDeviceId: "client-1",
        daemonDeviceId: "daemon-1",
        isInitiator: true,
      })
    ).toThrow(/low-order|non-contributory|invalid private or public key/i);

    const clientSession = BrowserE2EESession.deriveDirectP2Pv2({
      myStaticSecretB64: alice.x25519SecretB64,
      peerStaticPublicB64: bob.x25519PublicB64,
      myEphemeralSecret: ephA.secret,
      peerEphemeralPublicB64: ephB.publicB64,
      clientDeviceId: "client-1",
      daemonDeviceId: "daemon-1",
      isInitiator: true,
    });

    const daemonSession = BrowserE2EESession.deriveDirectP2Pv2({
      myStaticSecretB64: bob.x25519SecretB64,
      peerStaticPublicB64: alice.x25519PublicB64,
      myEphemeralSecret: ephB.secret,
      peerEphemeralPublicB64: ephA.publicB64,
      clientDeviceId: "client-1",
      daemonDeviceId: "daemon-1",
      isInitiator: false,
    });

    const sealed = clientSession.sealMessage({ op: "ping", req_id: "r1", sent_ms: now });
    const opened = daemonSession.openMessage(sealed);
    expect(opened.op).toBe("ping");
    expect(opened.req_id).toBe("r1");

    // Replaying exact same packet must fail
    expect(() => daemonSession.openMessage(sealed)).toThrow(/duplicate|replay/i);
  });

  test("Relay v2 session_auth validation and symmetric encrypted exchange", () => {
    const client = makeTestIdentity("web-client");
    const daemon = makeTestIdentity("linux-daemon");

    const auth = createRelaySessionAuth(client, "c-100", "d-200");
    const verifiedNonce = verifyRelaySessionAuth(
      auth,
      "c-100",
      "d-200",
      client.ed25519PublicB64,
      client.x25519PublicB64
    );
    expect(verifiedNonce.length).toBe(16);

    // Target mismatch throws verification error
    expect(() =>
      verifyRelaySessionAuth(
        auth,
        "c-100",
        "d-OTHER",
        client.ed25519PublicB64,
        client.x25519PublicB64
      )
    ).toThrow(/signature verification failed/i);

    const sClient = BrowserE2EESession.deriveRelayV2({
      myStaticSecretB64: client.x25519SecretB64,
      peerStaticPublicB64: daemon.x25519PublicB64,
      sessionNonceB64: auth.session_nonce_b64,
      clientDeviceId: "c-100",
      daemonDeviceId: "d-200",
      isInitiator: true,
    });
    const sDaemon = BrowserE2EESession.deriveRelayV2({
      myStaticSecretB64: daemon.x25519SecretB64,
      peerStaticPublicB64: client.x25519PublicB64,
      sessionNonceB64: auth.session_nonce_b64,
      clientDeviceId: "c-100",
      daemonDeviceId: "d-200",
      isInitiator: false,
    });

    const pkt = sDaemon.sealMessage({ op: "pong", req_id: "p1", sent_ms: 123456 });
    const dec = sClient.openMessage(pkt);
    expect(dec.op).toBe("pong");
    expect(dec.sent_ms).toBe(123456);
    expect(() => sClient.openMessage(pkt)).toThrow(/duplicate|replay/i);
  });
});

describe("WebVtTerminal 2D VT100 engine", () => {
  test("handles split UTF-8 CJK sequences without U+FFFD replacement", () => {
    const vt = new WebVtTerminal(40, 10);
    const bytes = new TextEncoder().encode("中文OK");
    vt.feedBytes(bytes.slice(0, 2));
    vt.feedBytes(bytes.slice(2));
    expect(vt.renderPlainText()).toBe("中文OK");
    expect(vt.renderPlainText().includes("\uFFFD")).toBe(false);
  });

  test("handles carriage return, backspace over wide chars, CSI erase, alt-screen & OSC 7501", () => {
    const vt = new WebVtTerminal(30, 8);
    vt.feedText("prompt> abc\rprompt> XYZ\x08W");
    expect(vt.renderPlainText()).toBe("prompt> XYW");

    // Wide char takes 2 columns: one \x08 lands on the right half (clears wide char and writes 'B' at col 2)
    const vtWide = new WebVtTerminal(30, 8);
    vtWide.feedText("A好\x08B");
    expect(vtWide.renderPlainText()).toBe("A B");

    // Erase 2 columns cleanly with \x08\x08 + overwrite
    const vtWide2 = new WebVtTerminal(30, 8);
    vtWide2.feedText("A好\x08\x08B\x1b[K");
    expect(vtWide2.renderPlainText()).toBe("AB");

    // Alternate screen isolation (?1049h / ?1049l)
    const vtAlt = new WebVtTerminal(20, 6);
    vtAlt.feedText("main-buffer");
    vtAlt.feedText("\x1b[?1049h\x1b[2J\x1b[1;1Halt-buffer");
    expect(vtAlt.renderPlainText()).toBe("alt-buffer");
    vtAlt.feedText("\x1b[?1049l");
    expect(vtAlt.renderPlainText()).toBe("main-buffer");

    // OSC 7501 status + HTML XSS escaping in output
    vtAlt.feedText("\n<script>alert(1)</script>");
    vtAlt.feedText("\x1b]7501;state=working;app=codex;title=Build Task\x07");
    expect(vtAlt.latestStatus?.state).toBe("working");
    expect(vtAlt.latestStatus?.app).toBe("codex");
    const html = vtAlt.renderHtml();
    expect(html.includes("<script>")).toBe(false);
    expect(html.includes("&lt;script&gt;")).toBe(true);
  });

  test("handles strict CSI prefixes, colon truecolor SGR, DCS/APC skip, combining marks & alt-screen margins", () => {
    const vt = new WebVtTerminal(40, 10);
    // Non-`?` CSI private sequences & intermediates must not pollute SGR or cursor
    vt.feedText("\x1b[>4;2m\x1b[>1u\x1b[2 qHello");
    // DCS and APC strings must be skipped until ST
    vt.feedText("\x1bP+q544e\x1b\\\x1b_Gf=24;AAAA\x1b\\ ");
    // Colon truecolor SGR with empty colorspace `38:2::56:189:248`
    vt.feedText("\x1b[38:2::56:189:248mBLUE\x1b[0m");
    // Combining acute accent attaches to 'e' without advancing cursor
    vt.feedText(" e\u0301");
    expect(vt.renderPlainText()).toBe("Hello BLUE e\u0301");
    const html = vt.renderHtml(true);
    expect(html.includes("rgb(56,189,248)")).toBe(true);
    expect(html.includes("term-cursor")).toBe(true);

    // Hide cursor (?25l) removes cursor span; bracketed paste & DECCKM tracked
    vt.feedText("\x1b[?25l\x1b[?2004h\x1b[?1h");
    expect(vt.cursorVisible).toBe(false);
    expect(vt.bracketedPaste).toBe(true);
    expect(vt.appCursorKeys).toBe(true);
    expect(vt.renderHtml(true).includes("term-cursor")).toBe(false);

    // Alt-screen isolates scroll margins
    vt.feedText("\x1b[2;8r\x1b[?1049h\x1b[1;4rALT\x1b[?1049l");
    expect(vt.renderPlainText()).toBe("Hello BLUE e\u0301");
  });

  test("Relay v2 supports_zstd signature binding & RelayNonceReplayCache reject tampering/replays", () => {
    expect(MAX_HANDSHAKE_SKEW_MS).toBe(120_000);
    const client = makeTestIdentity("web-zstd");
    const auth = createRelaySessionAuth(client, "c-1", "d-1", Date.now(), true);
    expect(auth.supports_zstd).toBe(true);
    // Flipping supports_zstd breaks signature verification
    expect(() =>
      verifyRelaySessionAuth(
        { ...auth, supports_zstd: false },
        "c-1",
        "d-1",
        client.ed25519PublicB64,
        client.x25519PublicB64
      )
    ).toThrow(/signature verification failed/i);

    const cache = new RelayNonceReplayCache();
    cache.checkAndRemember("d-1", auth.session_nonce_b64);
    expect(() => cache.checkAndRemember("d-1", auth.session_nonce_b64)).toThrow(/replayed/i);

    const hText = p2pHelloV2SigningTextWithZstd("c-1", "d-1", client.x25519PublicB64, client.x25519PublicB64, 1234, true);
    expect(hText.endsWith(":zstd=1")).toBe(true);
  });
});
