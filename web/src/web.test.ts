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
import {
  WebVtTerminal,
  escapeHtml,
  safeHref,
  type VtDomContainerLike,
  type VtRowNodeLike,
} from "./vt";
import { encodeTerminalKeyEvent, wrapBracketedPaste } from "./terminal_keys";
import { TerminalNotificationTracker, isNeedsYouState } from "./notify";
import { SmartPathBackoffPolicy, collectDirectWsCandidates } from "./smart_path";
import {
  MOBILE_QUICK_CHIPS,
  buildWebSessionMarkdown,
  extractSafeTerminalUrls,
  findWebTerminalMatches,
  formatWebHostHealth,
} from "./terminal_extras";
import {
  createInitialFileBrowserState,
  joinRootAndRel,
  parentSubPath,
  renderRemoteFileBrowserModal,
} from "./file_browser";
import {
  createInitialPortPreviewState,
  decodePortPreviewBody,
  parseWebPreviewPortAndPath,
  renderPortPreviewModal,
} from "./port_preview";

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

  test("row-level dirty-diff patchDom reuses unchanged rows, protects multi-row selection & handles alt-screen", () => {
    class MockRow implements VtRowNodeLike {
      className = "";
      innerHTML = "";
      attrs = new Map<string, string>();
      private parentList: MockRow[];
      constructor(parentList: MockRow[]) {
        this.parentList = parentList;
      }
      getAttribute(name: string): string | null {
        return this.attrs.get(name) ?? null;
      }
      setAttribute(name: string, value: string): void {
        this.attrs.set(name, value);
      }
      contains(node: unknown): boolean {
        return node === this;
      }
      remove(): void {
        const idx = this.parentList.indexOf(this);
        if (idx >= 0) this.parentList.splice(idx, 1);
      }
    }

    const createMockContainer = (): VtDomContainerLike & { rows: MockRow[] } => {
      const rows: MockRow[] = [];
      const attrs = new Map<string, string>();
      const container: VtDomContainerLike & { rows: MockRow[] } = {
        rows,
        get innerHTML() {
          return rows.map((r) => r.innerHTML).join("\n");
        },
        set innerHTML(val: string) {
          if (val === "") rows.length = 0;
        },
        get children() {
          return rows;
        },
        get childElementCount() {
          return rows.length;
        },
        getAttribute(name: string) {
          return attrs.get(name) ?? null;
        },
        setAttribute(name: string, value: string) {
          attrs.set(name, value);
        },
        appendChild(node: unknown) {
          if (Array.isArray(node)) {
            rows.push(...(node as MockRow[]));
          } else {
            rows.push(node as MockRow);
          }
          return node;
        },
        ownerDocument: {
          createElement(_tag: string) {
            return new MockRow(rows);
          },
          createDocumentFragment() {
            const list: MockRow[] = [];
            return {
              appendChild(n: unknown) {
                list.push(n as MockRow);
                return n;
              },
              [Symbol.iterator]: () => list[Symbol.iterator](),
              // Allow container.appendChild to unpack fragment items
              toArray: () => list,
            } as unknown as { appendChild(node: unknown): unknown };
          },
        },
      };
      // Handle fragment object in appendChild
      const origAppend = container.appendChild.bind(container);
      container.appendChild = (node: unknown) => {
        if (node && typeof node === "object" && "toArray" in node) {
          return origAppend((node as { toArray: () => MockRow[] }).toArray());
        }
        return origAppend(node);
      };
      return container;
    };

    const vt = new WebVtTerminal(40, 8);
    vt.feedText("line-1\r\nline-2\r\nline-3");

    const initHtml = vt.renderInitialRowsHtml(true);
    expect(initHtml.includes('class="vt-row"')).toBe(true);
    expect(initHtml.includes("data-vt-key=")).toBe(true);

    const container = createMockContainer();
    container.setAttribute("data-vt-mode", "placeholder");

    // 1. Initial mount patches all 3 rows via DocumentFragment
    const s1 = vt.patchDom(container, true);
    expect(s1).toEqual({ patched: 3, reused: 0, total: 3, skippedProtected: false });
    expect(container.getAttribute("data-vt-mode")).toBe("rows");
    expect(container.rows.length).toBe(3);

    // 2. Identical frame reuses 100% of DOM rows (0 patches)
    const s2 = vt.patchDom(container, true);
    expect(s2).toEqual({ patched: 0, reused: 3, total: 3, skippedProtected: false });

    // 3. Appending to line 3 patches ONLY line 3 (reuses lines 1 & 2)
    vt.feedText("!");
    const s3 = vt.patchDom(container, true);
    expect(s3).toEqual({ patched: 1, reused: 2, total: 3, skippedProtected: false });
    expect(container.rows[2].innerHTML.includes("line-3!")).toBe(true);

    // 4. Cursor-only jump to line 1 patches ONLY old cursor row (3) & new cursor row (1)
    vt.feedText("\x1b[1;1H");
    const s4 = vt.patchDom(container, true);
    expect(s4).toEqual({ patched: 2, reused: 1, total: 3, skippedProtected: false });

    // 5. Protected multi-row selection (row 0 -> row 1) skips rows 0 & 1 while patching dirty row 2
    const row0Ref = container.rows[0];
    const row1Ref = container.rows[1];
    const row0HtmlBefore = row0Ref.innerHTML;
    const row1HtmlBefore = row1Ref.innerHTML;
    vt.feedText("EDIT0\x1b[2;1HEDIT1\x1b[3;1HSTREAM2");
    const s5 = vt.patchDom(container, true, { anchorNode: row0Ref, focusNode: row1Ref });
    expect(s5).toEqual({ patched: 1, reused: 2, total: 3, skippedProtected: true });
    expect(row0Ref.innerHTML).toBe(row0HtmlBefore);
    expect(row1Ref.innerHTML).toBe(row1HtmlBefore);
    expect(container.rows[2].innerHTML.includes("STREAM2")).toBe(true);

    // Once selection clears, deferred rows 0 & 1 catch up cleanly
    const s5After = vt.patchDom(container, true, null);
    expect(s5After).toEqual({ patched: 2, reused: 1, total: 3, skippedProtected: false });
    expect(row0Ref.innerHTML.includes("EDIT0")).toBe(true);
    expect(row1Ref.innerHTML.includes("EDIT1")).toBe(true);

    // 6. Partial-outside selection via `containsNode` protects intersecting row even if anchor/focus lie outside
    const row1BeforePartial = container.rows[1].innerHTML;
    vt.feedText("\x1b[2;1HPARTIAL_EDIT");
    const sPartial = vt.patchDom(container, true, {
      containsNode: (node) => node === container.rows[1],
    });
    expect(sPartial.skippedProtected).toBe(true);
    expect(container.rows[1].innerHTML).toBe(row1BeforePartial);
    const sPartialCatchup = vt.patchDom(container, true, null);
    expect(sPartialCatchup.skippedProtected).toBe(false);
    expect(container.rows[1].innerHTML.includes("PARTIAL_EDIT")).toBe(true);

    // 7. Alt-screen shrink trims trailing DOM rows cleanly, and exit restores primary buffer
    vt.feedText("\x1b[?1049h\x1b[2J\x1b[1;1HALT-ONLY");
    const sAlt = vt.patchDom(container, true);
    expect(sAlt).toEqual({ patched: 1, reused: 0, total: 1, skippedProtected: false });
    expect(container.rows.length).toBe(1);
    expect(container.rows[0].innerHTML.includes("ALT-ONLY")).toBe(true);

    vt.feedText("\x1b[?1049l");
    const sRestore = vt.patchDom(container, true);
    expect(sRestore).toEqual({ patched: 3, reused: 0, total: 3, skippedProtected: false });
    expect(container.rows.length).toBe(3);

    // 8. Full-scrollback O(1) eviction stays O(1) instead of rewriting all scrollback rows
    const vtFull = new WebVtTerminal(40, 6, 10);
    const cFull = createMockContainer();
    for (let i = 1; i <= 20; i++) {
      vtFull.feedText(`row-${i}\r\n`);
    }
    vtFull.patchDom(cFull, true);
    vtFull.feedText("row-21\r\n");
    const sEvict = vtFull.patchDom(cFull, true);
    // Only the previous cursor row + newly appended row are patched (<< 16 total rows)
    expect(sEvict.patched).toBeLessThanOrEqual(3);
    expect(sEvict.reused).toBeGreaterThanOrEqual(10);

    // 9. Distinct WebVtTerminal instance on same container never collides on data-vt-key
    const vtOther = new WebVtTerminal(40, 8);
    vtOther.feedText("other-session");
    const sOther = vtOther.patchDom(container, true);
    expect(sOther).toEqual({ patched: 1, reused: 0, total: 1, skippedProtected: false });
    expect(container.rows[0].innerHTML.includes("other-session")).toBe(true);

    // 10. consumeLatestStatus delivers OSC 7501 once & reset() / loadSnapshot() clear pendingStatus
    vtOther.feedText("\x1b]7501;state=needs_you;msg=Approve?\x07");
    expect(vtOther.consumeLatestStatus()?.state).toBe("needs_you");
    expect(vtOther.consumeLatestStatus()).toBeNull();
    vtOther.loadSnapshot("\x1b]7501;state=needs_you;msg=OldSnapshot\x07line1\nline2\n");
    expect(vtOther.consumeLatestStatus()).toBeNull();
    vtOther.reset();
    expect(vtOther.latestStatus).toBeNull();
    expect(vtOther.windowTitle).toBeNull();

    // 11. Selecting rows while scrollback evicts freezes viewport cleanly without row-index skew, then catches up in O(1)
    const vtFreeze = new WebVtTerminal(40, 4, 6);
    const cFreeze = createMockContainer();
    for (let i = 1; i <= 12; i++) {
      vtFreeze.feedText(`line-${i}\r\n`);
    }
    vtFreeze.patchDom(cFreeze, true);
    const selectedEl = cFreeze.rows[3];
    const selectedBefore = selectedEl.innerHTML;
    vtFreeze.feedText("line-13\r\n");
    const sFrozen = vtFreeze.patchDom(cFreeze, true, {
      anchorNode: selectedEl,
      focusNode: selectedEl,
      containsNode: (el) => el === selectedEl,
    });
    expect(sFrozen.patched).toBe(0);
    expect(sFrozen.skippedProtected).toBe(true);
    expect(cFreeze.rows[3].innerHTML).toBe(selectedBefore);
    // Selection cleared -> catches up cleanly
    const sCatchup = vtFreeze.patchDom(cFreeze, true);
    expect(sCatchup.skippedProtected).toBe(false);
    expect(cFreeze.rows.some((r) => r.innerHTML.includes("line-13"))).toBe(true);
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

describe("Web modular engines: terminal_keys, OSC 7501 notifications & Smart-Path hot-switch", () => {
  test("encodeTerminalKeyEvent handles Ctrl/DECCKM/Shift+Tab/F-keys/Alt & all Cmd passthrough", () => {
    expect(encodeTerminalKeyEvent({ key: "c", ctrlKey: true })).toBe("\x03");
    expect(encodeTerminalKeyEvent({ key: "c", metaKey: true })).toBeNull();
    expect(encodeTerminalKeyEvent({ key: "Backspace", metaKey: true })).toBeNull();
    expect(encodeTerminalKeyEvent({ key: "ArrowLeft", metaKey: true })).toBeNull();
    expect(encodeTerminalKeyEvent({ key: "k", metaKey: true })).toBeNull();
    expect(encodeTerminalKeyEvent({ key: "a", isComposing: true })).toBeNull();
    expect(encodeTerminalKeyEvent({ key: "Tab", shiftKey: true })).toBe("\x1b[Z");
    expect(encodeTerminalKeyEvent({ key: "ArrowUp" }, { appCursorKeys: false })).toBe("\x1b[A");
    expect(encodeTerminalKeyEvent({ key: "ArrowUp" }, { appCursorKeys: true })).toBe("\x1bOA");
    expect(encodeTerminalKeyEvent({ key: "F5" })).toBe("\x1b[15~");
    expect(encodeTerminalKeyEvent({ key: "b", altKey: true })).toBe("\x1bb");
    expect(wrapBracketedPaste("hello", true)).toBe("\x1b[200~hello\x1b[201~");
    expect(wrapBracketedPaste("hello", false)).toBe("hello");
    expect(isNeedsYouState("needs_you")).toBe(true);
    expect(isNeedsYouState("permission")).toBe(true);
    expect(isNeedsYouState("waiting")).toBe(true);
    expect(isNeedsYouState("working")).toBe(false);
  });

  test("TerminalNotificationTracker is edge-triggered, suppresses visible active terminal & respects cooldown", () => {
    const tracker = new TerminalNotificationTracker(20_000);
    const baseInput = {
      daemonId: "d-1",
      daemonName: "Mac-mini",
      terminalAddr: "/tmp/h/1",
      terminalName: "claude",
    };

    // Baseline snapshot never fires
    expect(
      tracker.observe(
        { ...baseInput, status: { state: "working", app: "claude" } },
        { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/1", documentHidden: true, locale: "zh", nowMs: 1_000 }
      )
    ).toBeNull();

    // Transition to needs_you while visible on SAME terminal is suppressed
    expect(
      tracker.observe(
        { ...baseInput, status: { state: "needs_you", app: "claude", msg: "Approve rm?" } },
        { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/1", documentHidden: false, locale: "zh", nowMs: 2_000 }
      )
    ).toBeNull();

    // Transition back to working, then needs_you while tab hidden -> fires!
    tracker.observe(
      { ...baseInput, status: { state: "working", app: "claude" } },
      { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/1", documentHidden: true, locale: "zh", nowMs: 3_000 }
    );
    const alert = tracker.observe(
      { ...baseInput, status: { state: "needs_you", app: "claude", msg: "Approve tool?" } },
      { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/1", documentHidden: true, locale: "zh", nowMs: 4_000 }
    );
    expect(alert).not.toBeNull();
    expect(alert?.kind).toBe("needs_you");
    expect(alert?.title).toContain("Mac-mini");
    expect(alert?.body).toBe("Approve tool?");

    // Within 20s cooldown, rapid working -> done is suppressed
    tracker.observe(
      { ...baseInput, status: { state: "working", app: "claude" } },
      { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/2", documentHidden: false, locale: "en", nowMs: 5_000 }
    );
    expect(
      tracker.observe(
        { ...baseInput, status: { state: "done", app: "claude" } },
        { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/2", documentHidden: false, locale: "en", nowMs: 10_000 }
      )
    ).toBeNull();

    // After cooldown expires, working -> done on background terminal fires
    tracker.observe(
      { ...baseInput, status: { state: "working", app: "claude" } },
      { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/2", documentHidden: false, locale: "en", nowMs: 25_000 }
    );
    const doneAlert = tracker.observe(
      { ...baseInput, status: { state: "done", app: "claude", title: "Tests green" } },
      { activeDaemonId: "d-1", activeTerminalAddr: "/tmp/h/2", documentHidden: false, locale: "en", nowMs: 26_000 }
    );
    expect(doneAlert?.kind).toBe("done");
    expect(doneAlert?.body).toBe("Tests green");
  });

  test("SmartPathBackoffPolicy & collectDirectWsCandidates enforce mixed-content rules, backoff & flap guard", () => {
    const peer = {
      id: "d-1",
      name: "Mac-mini",
      role: "host",
      platform: "macos",
      x25519_pubkey: "abc",
      is_online: true,
      endpoints_json: [
        { kind: "lan_ipv4", addr: "192.168.1.10:24444" },
        { kind: "lan_ws", addr: "wss://mac.lan:24444" },
        { kind: "ipv6_ws", addr: "ws://[::1]:24444" },
      ],
    };

    // HTTPS remote origin filters out insecure ws:// endpoints and keeps wss://
    const httpsCands = collectDirectWsCandidates(peer, { protocol: "https:", hostname: "coda.example.com" });
    expect(httpsCands).toEqual(["wss://mac.lan:24444"]);

    // Local HTTP origin prioritizes lan_ws -> ipv6_ws -> lan_ipv4 -> loopback fallback
    const httpCands = collectDirectWsCandidates(peer, { protocol: "http:", hostname: "localhost" });
    expect(httpCands).toEqual([
      "wss://mac.lan:24444",
      "ws://[::1]:24444",
      "ws://192.168.1.10:24444",
      "ws://127.0.0.1:24444",
    ]);

    const policy = new SmartPathBackoffPolicy({ baseIntervalMs: 10_000, maxIntervalMs: 60_000, dropCooldownMs: 5_000 });
    expect(policy.canProbeNow("d-1", "relay_ws", 1000)).toBe(true);
    expect(policy.canProbeNow("d-1", "lan_ws", 1000)).toBe(false);

    expect(policy.recordProbeFailure("d-1").delayMs).toBe(10_000);
    expect(policy.recordProbeFailure("d-1").delayMs).toBe(20_000);
    expect(policy.recordProbeFailure("d-1").delayMs).toBe(40_000);
    expect(policy.recordProbeFailure("d-1").delayMs).toBe(60_000);

    // Network change resets exponential backoff
    policy.resetForNetworkChange("d-1");
    expect(policy.nextDelayMs("d-1")).toBe(10_000);

    // Direct drop enforces flap cooldown
    policy.recordDirectDrop("d-1", 50_000);
    expect(policy.canProbeNow("d-1", "relay_ws", 52_000)).toBe(false);
    expect(policy.canProbeNow("d-1", "relay_ws", 55_000)).toBe(true);
  });

  test("v0.7.0 terminal search, safe URL extraction, Markdown export, health telemetry & file browser", () => {
    const plain = "$ cargo test\nERROR: failed at https://codahub.dev/docs/ota?v=1)，重试\ntest error ok\n   \n";
    const matches = findWebTerminalMatches(plain, "error");
    expect(matches.length).toBe(2);
    expect(matches[0].rowIdx).toBe(1);
    expect(matches[1].rowIdx).toBe(2);

    const urls = extractSafeTerminalUrls("详见 (https://codahub.dev/docs/ota?v=1)，以及 https://example.com/a(b).");
    expect(urls).toEqual(["https://codahub.dev/docs/ota?v=1", "https://example.com/a(b)"]);

    const md = buildWebSessionMarkdown("coda · agy", "/opt/coda", plain, "2026-11-20T00:00:00Z");
    expect(md).toContain("# Coda Session Transcript — coda · agy");
    expect(md).toContain("test error ok\n```");

    expect(MOBILE_QUICK_CHIPS).toContain("/compact");
    expect(MOBILE_QUICK_CHIPS).toContain("git status");

    const hh = formatWebHostHealth({
      daemon_version: "0.7.0",
      os: "linux",
      arch: "x86_64",
      cpu_cores: 8,
      load_avg_1m: 0.42,
      mem_used_mb: 6144,
      mem_total_mb: 16384,
      daemon_uptime_secs: 120,
      ota_supported: true,
    });
    expect(hh).toBe("v0.7.0 · 8c · load 0.42 · 6.0/16.0G");

    expect(parentSubPath("crates/coda-server/src")).toBe("crates/coda-server");
    expect(parentSubPath("crates")).toBe("");
    expect(joinRootAndRel("/opt/coda/", "/src/lib.rs")).toBe("/opt/coda/src/lib.rs");

    const st = createInitialFileBrowserState();
    st.open = true;
    st.rootPath = "/opt/coda";
    st.entries = [{ name: "<img onerror=1>", rel_path: "bad", is_dir: false, size_bytes: 2048 }];
    const html = renderRemoteFileBrowserModal(st, ["/opt/coda"], (en) => en);
    expect(html).toContain("&lt;img onerror=1&gt;");
    expect(html).not.toContain("<img onerror=1>");
    expect(html).toContain("2.0 KB");

    expect(parseWebPreviewPortAndPath("5173", "api/v1")).toEqual({ port: 5173, path: "/api/v1" });
    expect("error" in parseWebPreviewPortAndPath("80", "/")).toBe(true);
    expect("error" in parseWebPreviewPortAndPath("24444", "/")).toBe(true);

    const jsonBody = decodePortPreviewBody(
      toB64(new TextEncoder().encode('{"healthy":true}')),
      "application/json"
    );
    expect(jsonBody).toContain('"healthy": true');

    const pp = createInitialPortPreviewState();
    pp.open = true;
    pp.statusCode = 200;
    pp.contentType = "text/html";
    pp.viewTab = "render";
    pp.bodyText = "<script>alert(1)</script><h1>OK</h1>";
    const ppHtml = renderPortPreviewModal(pp, "dev-box", (en) => en);
    expect(ppHtml).toContain('sandbox=""');
    expect(ppHtml).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
  });
});
