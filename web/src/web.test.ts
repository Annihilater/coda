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
import {
  encodeTerminalKeyEvent,
  isJumpNeedsYouShortcut,
  isToggleSplitViewShortcut,
  sanitizeBracketedPasteText,
  wrapBracketedPaste,
} from "./terminal_keys";
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
  decodePortPreviewPayload,
  detectClientRenderedSpaHtml,
  parseWebPreviewPortAndPath,
  renderPortPreviewModal,
} from "./port_preview";
import {
  APPROVAL_CAPSULE_ITEMS,
  buildClipboardImageFilename,
  canStepUpSpawnDirectory,
  collectNeedsAttentionItems,
  collectPrunableVtKeys,
  computeBootstrapAutoFolder,
  createDefaultNotificationSettings,
  createInitialGitDiffState,
  encodeApprovalActionBytes,
  encodeComposerBracketedPasteSubmit,
  encodeComposerSubmit,
  extractPureClipboardImages,
  filterUnifiedDiffByFile,
  formatUploadedPathForPty,
  isKnownTuiAgentTerminal,
  parentAbsolutePath,
  parseGitStatusShort,
  parseUnifiedDiffLines,
  pickNextNeedsAttentionItem,
  rebuildSpeechRecognitionTranscript,
  renderGitDiffModal,
  renderNeedsAttentionSidebarSection,
  renderNeedsYouCapsuleBar,
  renderPromptComposerPanel,
  resolveClipboardImageExtension,
  resolveEffectiveBracketedPaste,
  resolveGitDiffFileRoot,
  resolveSpawnPickerInitialRoot,
  resolveVoiceCaptureStrategy,
  shellEscapePath,
  stepPaletteSelectionIndex,
  suggestFolderNameFromGitUrl,
  validateGitCloneUrlClient,
  validateNotificationWebhookSettings,
  validateSingleFolderName,
} from "./git_diff_composer";
import { readFileSync } from "node:fs";
import {
  DEFAULT_PROMPT_SNIPPETS,
  IOS_PWA_LOCK_SCREEN_PUSH_TIP_EN,
  MAX_PROMPT_SNIPPETS,
  SNIPPETS_CACHE_STORAGE_KEY,
  arrayBufferToBase64Url,
  buildInviteRegistrationUrl,
  clearAccountScopedSignOutState,
  deriveConsoleConnectionVisualState,
  deriveSnippetTitleFromPrompt,
  extractImagesFromClipboardItems,
  extractPushSubscriptionKeys,
  extractResetTokenFromInput,
  formatConsoleHeaderSubtitle,
  insertSnippetIntoDraft,
  isCustomPromptSnippet,
  isIosNonStandaloneBrowser,
  isPushSubscriptionVapidKeyMismatch,
  normalizePromptSnippetsList,
  normalizeSwNotificationTargetUrl,
  onboardingDismissedStorageKey,
  parseLocationRouteAndParams,
  parseSwPushPayload,
  renderAdminFriendsDirectorySection,
  renderConsoleEmptyStateHtml,
  renderFriendOnboardingCard,
  renderWebPushSettingsCard,
  shouldSwCacheStaticAsset,
  teardownWebPushOnSignOut,
  validateNewPasswordLength,
  validatePromptSnippet,
  validateResetPasswordInputs,
  vapidBase64UrlToUint8Array,
} from "./friend_ux";

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

describe("v0.9.0 Git Diff Review Modal, Clipboard Screenshot Upload, Prompt Composer, Inbox & Webhooks", () => {
  test("parseGitStatusShort parses M/A/D/??/R badges, quoted paths & trimmed lines", () => {
    const raw = [
      " M web/src/app.ts",
      "A  web/src/git_diff_composer.ts",
      "D  legacy/old.ts",
      "?? assets/new screen.png",
      'R  "src/old name.ts" -> "src/new name.ts"',
    ].join("\n");
    const files = parseGitStatusShort(raw);
    expect(files.length).toBe(5);
    expect(files[0]).toEqual({ status: "M", rawStatus: "M", path: "web/src/app.ts", origPath: null });
    expect(files[1]).toEqual({
      status: "A",
      rawStatus: "A",
      path: "web/src/git_diff_composer.ts",
      origPath: null,
    });
    expect(files[2].status).toBe("D");
    expect(files[3]).toEqual({ status: "??", rawStatus: "??", path: "assets/new screen.png", origPath: null });
    expect(files[4]).toEqual({
      status: "R",
      rawStatus: "R",
      path: "src/new name.ts",
      origPath: "src/old name.ts",
    });

    // Trimmed first-line fallback
    expect(parseGitStatusShort("M src/trimmed.ts")[0]?.path).toBe("src/trimmed.ts");
  });

  test("filterUnifiedDiffByFile, parseUnifiedDiffLines & renderGitDiffModal strictly escape HTML/XSS", () => {
    const sampleDiff = [
      "diff --git a/web/src/app.ts b/web/src/app.ts",
      "index 1111111..2222222 100644",
      "--- a/web/src/app.ts",
      "+++ b/web/src/app.ts",
      "@@ -10,3 +10,4 @@ function boot<T>() {",
      " const safe = true;",
      '-const oldHtml = "<img src=x onerror=alert(1)>";',
      '+const newHtml = "<script>alert(document.cookie)</script>";',
      "+const extra = 42;",
      "diff --git a/README.md b/README.md",
      "index 3333333..4444444 100644",
      "--- a/README.md",
      "+++ b/README.md",
      "@@ -1,1 +1,2 @@",
      " # Coda",
      "+v0.9.0",
    ].join("\n");

    const onlyApp = filterUnifiedDiffByFile(sampleDiff, "web/src/app.ts");
    expect(onlyApp).toContain("diff --git a/web/src/app.ts b/web/src/app.ts");
    expect(onlyApp).not.toContain("README.md");

    const parsed = parseUnifiedDiffLines(onlyApp);
    expect(parsed.added).toBe(2);
    expect(parsed.deleted).toBe(1);
    expect(parsed.lines.some((l) => l.kind === "hunk" && l.text.startsWith("@@"))).toBe(true);
    const delLine = parsed.lines.find((l) => l.kind === "del");
    const addLines = parsed.lines.filter((l) => l.kind === "add");
    expect(delLine?.oldLine).toBe(11);
    expect(addLines[0]?.newLine).toBe(11);
    expect(addLines[1]?.newLine).toBe(12);

    const gdState = createInitialGitDiffState();
    gdState.open = true;
    gdState.cwd = '/tmp/"bad"<cwd>';
    gdState.branch = "feat/<xss>";
    gdState.statusShort = ' M web/src/app.ts\n?? "<svg onload=alert(1)>.ts"';
    gdState.files = parseGitStatusShort(gdState.statusShort);
    gdState.diffUnified = sampleDiff;

    const modalHtml = renderGitDiffModal(gdState, [gdState.cwd], (en) => en);
    expect(modalHtml).not.toContain("<script>");
    expect(modalHtml).not.toContain("<img src=x");
    expect(modalHtml).not.toContain("<svg onload=");
    expect(modalHtml).toContain("&lt;script&gt;alert(document.cookie)&lt;/script&gt;");
    expect(modalHtml).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(modalHtml).toContain("feat/&lt;xss&gt;");
    expect(modalHtml).toContain("gd-add");
    expect(modalHtml).toContain("gd-del");
    expect(modalHtml).toContain("gd-hunk");
  });

  test("clipboard filename generator, shell path escaping & E2EE v0.9.0 wire messages", () => {
    expect(buildClipboardImageFilename(1700000000123, 0)).toBe("clipboard-1700000000123.png");
    expect(buildClipboardImageFilename(1700000000123, 1)).toBe("clipboard-1700000000123-1.png");

    expect(shellEscapePath("/tmp/herdr/clipboard-1700000000123.png")).toBe(
      "/tmp/herdr/clipboard-1700000000123.png"
    );
    expect(formatUploadedPathForPty("/tmp/herdr/clipboard-1700000000123.png")).toBe(
      "/tmp/herdr/clipboard-1700000000123.png "
    );

    // Spaces, single quotes, subshells & leading hyphens must be strictly quoted
    expect(formatUploadedPathForPty("/tmp/my folder/it's $(whoami).png")).toBe(
      `'/tmp/my folder/it'\\''s $(whoami).png' `
    );
    expect(shellEscapePath("-rf")).toBe("'-rf'");

    // Verify E2EE session seals/opens git_diff_request and bootstrap_workspace cleanly
    const client = makeTestIdentity("web");
    const daemon = makeTestIdentity("host");
    const auth = createRelaySessionAuth(client, "c-1", "d-1");
    const sClient = BrowserE2EESession.deriveRelayV2({
      myStaticSecretB64: client.x25519SecretB64,
      peerStaticPublicB64: daemon.x25519PublicB64,
      sessionNonceB64: auth.session_nonce_b64,
      clientDeviceId: "c-1",
      daemonDeviceId: "d-1",
      isInitiator: true,
    });
    const sDaemon = BrowserE2EESession.deriveRelayV2({
      myStaticSecretB64: daemon.x25519SecretB64,
      peerStaticPublicB64: client.x25519PublicB64,
      sessionNonceB64: auth.session_nonce_b64,
      clientDeviceId: "c-1",
      daemonDeviceId: "d-1",
      isInitiator: false,
    });

    const diffReq = sDaemon.openMessage(
      sClient.sealMessage({ op: "git_diff_request", req_id: "gd-1", cwd: "/opt/coda" })
    );
    expect(diffReq).toEqual({ op: "git_diff_request", req_id: "gd-1", cwd: "/opt/coda" });

    const bootReq = sDaemon.openMessage(
      sClient.sealMessage({
        op: "bootstrap_workspace",
        req_id: "boot-1",
        parent_dir: "~/code",
        folder_name: "coda",
        git_clone_url: "https://github.com/Annihilater/coda.git",
      })
    );
    expect(bootReq.op).toBe("bootstrap_workspace");
    expect(bootReq.folder_name).toBe("coda");
  });

  test("Needs-You capsule bar, Bracketed Paste composer, Attention Inbox cycling & Speech fallback", () => {
    expect(APPROVAL_CAPSULE_ITEMS.map((i) => i.labelEn)).toEqual([
      "Allow (y)",
      "Reject (Esc)",
      "1",
      "2",
      "3",
      "Enter",
    ]);
    expect(encodeApprovalActionBytes("allow")).toBe("y\r");
    expect(encodeApprovalActionBytes("reject")).toBe("\x1b");
    expect(encodeApprovalActionBytes("1")).toBe("1\r");
    expect(encodeApprovalActionBytes("2")).toBe("2\r");
    expect(encodeApprovalActionBytes("3")).toBe("3\r");
    expect(encodeApprovalActionBytes("enter")).toBe("\r");

    const capsuleHtml = renderNeedsYouCapsuleBar(
      {
        address: "t-1",
        argv: ["claude"],
        cwd: "/repo",
        alive: true,
        status: { state: "needs_you", app: "claude", msg: "Allow <rm>?" },
      },
      (en) => en
    );
    expect(capsuleHtml).toContain("visible");
    expect(capsuleHtml).toContain("Allow (y)");
    expect(capsuleHtml).toContain("Reject (Esc)");
    expect(capsuleHtml).toContain("&lt;rm&gt;");
    expect(capsuleHtml).not.toContain("<rm>");

    // Multi-line Bracketed Paste strips embedded \x1b[201~ breakout attempts
    expect(encodeComposerBracketedPasteSubmit("step 1\r\nstep 2")).toBe(
      "\x1b[200~step 1\nstep 2\x1b[201~\r"
    );
    expect(encodeComposerBracketedPasteSubmit("evil\x1b[201~;rm -rf /")).toBe(
      "\x1b[200~evil;rm -rf /\x1b[201~\r"
    );

    const compHtml = renderPromptComposerPanel({
      open: true,
      draft: "line 1\n<tag>",
      isRecordingVoice: false,
      isTranscribingVoice: false,
      hasSpeechFallback: true,
      hasWhisperKey: false,
      tr: (en) => en,
    });
    expect(compHtml).toContain("Voice (Live)");
    expect(compHtml).toContain("line 1\n&lt;tag&gt;");

    // Zero-config speech strategy selection
    expect(
      resolveVoiceCaptureStrategy({
        apiKey: "",
        isSafeBaseUrl: true,
        hasMediaRecorder: true,
        hasSpeechRecognition: true,
      })
    ).toBe("browser_speech");
    expect(
      resolveVoiceCaptureStrategy({
        apiKey: "••••••••",
        isSafeBaseUrl: true,
        hasMediaRecorder: true,
        hasSpeechRecognition: true,
        hasOnlineDaemon: true,
      })
    ).toBe("daemon_whisper");
    expect(
      resolveVoiceCaptureStrategy({
        apiKey: "••••••••",
        isSafeBaseUrl: true,
        hasMediaRecorder: true,
        hasSpeechRecognition: true,
        hasOnlineDaemon: false,
      })
    ).toBe("browser_speech");
    expect(
      resolveVoiceCaptureStrategy({
        apiKey: "sk-live-123",
        isSafeBaseUrl: true,
        hasMediaRecorder: true,
        hasSpeechRecognition: true,
      })
    ).toBe("whisper");

    // Pinned Needs-Attention inbox & ⌘J / Alt+J round-robin jump
    const devices = [
      { id: "d-mac", name: "MacBook", role: "daemon", platform: "macos", x25519_pubkey: "k1", is_online: true },
      { id: "d-lin", name: "Ubuntu", role: "daemon", platform: "linux", x25519_pubkey: "k2", is_online: true },
    ];
    const macTerms = [
      { address: "m-1", name: "claude", argv: [], cwd: "/a", alive: true, status: { state: "needs_you", msg: "Approve edit?" } },
      { address: "m-2", name: "shell", argv: [], cwd: "/a", alive: true, status: { state: "working" } },
    ];
    const byPeer = new Map([
      ["d-mac", macTerms],
      [
        "d-lin",
        [
          { address: "l-1", name: "codex", argv: [], cwd: "/b", alive: true, status: { state: "permission", msg: "Run cargo test?" } },
        ],
      ],
    ]);
    const inbox = collectNeedsAttentionItems(devices, byPeer, "d-mac", macTerms);
    expect(inbox.length).toBe(2);
    expect(inbox[0].terminalAddr).toBe("m-1");
    expect(inbox[1].terminalAddr).toBe("l-1");

    expect(pickNextNeedsAttentionItem(inbox, "d-mac", "m-2")?.terminalAddr).toBe("m-1");
    expect(pickNextNeedsAttentionItem(inbox, "d-mac", "m-1")?.terminalAddr).toBe("l-1");
    expect(pickNextNeedsAttentionItem(inbox, "d-lin", "l-1")?.terminalAddr).toBe("m-1");

    const inboxHtml = renderNeedsAttentionSidebarSection(inbox, "d-mac", "m-1", (en) => en);
    expect(inboxHtml).toContain("Needs Attention");
    expect(inboxHtml).toContain("⌘J");
    expect(inboxHtml).toContain("Run cargo test?");
  });

  test("Workspace bootstrapper RCE guards & offline webhook validation", () => {
    expect(validateSingleFolderName("coda-service")).toBeNull();
    expect(validateSingleFolderName("")).not.toBeNull();
    expect(validateSingleFolderName("../escape")).not.toBeNull();
    expect(validateSingleFolderName("sub/dir")).not.toBeNull();
    expect(validateSingleFolderName("-upload-pack")).not.toBeNull();

    expect(validateGitCloneUrlClient("")).toBeNull();
    expect(validateGitCloneUrlClient("https://github.com/Annihilater/coda.git")).toBeNull();
    expect(validateGitCloneUrlClient("ssh://git@github.com/Annihilater/coda.git")).toBeNull();
    expect(validateGitCloneUrlClient("git@github.com:Annihilater/coda.git")).toBeNull();
    expect(validateGitCloneUrlClient("ext::sh -c touch% /tmp/pwn")).not.toBeNull();
    expect(validateGitCloneUrlClient("file:///etc/passwd")).not.toBeNull();
    expect(validateGitCloneUrlClient("--upload-pack=evil")).not.toBeNull();

    expect(suggestFolderNameFromGitUrl("https://github.com/Annihilater/coda.git")).toBe("coda");
    expect(suggestFolderNameFromGitUrl("git@github.com:Annihilater/coda-web.git")).toBe("coda-web");
    expect(parentAbsolutePath("/Users/alice/code/coda")).toBe("/Users/alice/code");
    expect(parentAbsolutePath("/Users")).toBe("/");

    const cfg = createDefaultNotificationSettings();
    expect(validateNotificationWebhookSettings(cfg)).toBeNull();
    expect(
      validateNotificationWebhookSettings({ ...cfg, bark_url: "https://api.day.app/abc123" })
    ).toBeNull();
    expect(
      validateNotificationWebhookSettings({ ...cfg, bark_url: "http://192.168.1.5/hook" })
    ).not.toBeNull();
    expect(
      validateNotificationWebhookSettings({ ...cfg, telegram_bot_token: "123:ABC", telegram_chat_id: "" })
    ).not.toBeNull();
    expect(
      validateNotificationWebhookSettings({
        ...cfg,
        telegram_bot_token: "123:ABC",
        telegram_chat_id: "998877",
      })
    ).toBeNull();
  });
});

describe("v1.0.0 Web Console Bug Fixes & UX Hardening", () => {
  test("P0 Bug #1: extractPureClipboardImages ignores rich text+bitmap (Excel/Numbers/Word) and derives true image extensions", () => {
    const fakePngBlob = new Blob([new Uint8Array([137, 80, 78, 71])], { type: "image/png" });
    const fakeJpegBlob = new Blob([new Uint8Array([0xff, 0xd8, 0xff])], { type: "image/jpeg" });

    // Rich copy from Excel / Numbers / Word / Notes contains BOTH text/plain AND image/png
    const richClipboard = {
      getData: (fmt: string) => (fmt === "text/plain" ? "cellA\tcellB\n100\t200" : ""),
      items: [
        { kind: "string", type: "text/plain" },
        { kind: "file", type: "image/png", getAsFile: () => fakePngBlob },
      ],
      files: [fakePngBlob],
    };
    expect(extractPureClipboardImages(richClipboard)).toEqual([]);

    // Pure screenshot paste has empty text/plain and returns the image blob(s)
    const screenshotClipboard = {
      getData: () => "",
      items: [{ kind: "file", type: "image/jpeg", getAsFile: () => fakeJpegBlob }],
      files: [],
    };
    expect(extractPureClipboardImages(screenshotClipboard)).toEqual([fakeJpegBlob]);

    // Files fallback when items is empty
    const filesOnlyClipboard = {
      getData: () => "",
      items: [],
      files: [fakePngBlob],
    };
    expect(extractPureClipboardImages(filesOnlyClipboard)).toEqual([fakePngBlob]);

    // True MIME / filename extension derivation instead of hardcoded .png
    expect(resolveClipboardImageExtension("image/jpeg")).toBe("jpg");
    expect(resolveClipboardImageExtension("image/webp; charset=binary")).toBe("webp");
    expect(resolveClipboardImageExtension("image/gif")).toBe("gif");
    expect(resolveClipboardImageExtension("image/png")).toBe("png");
    expect(resolveClipboardImageExtension("", "photo.JPEG")).toBe("jpg");
    expect(resolveClipboardImageExtension("", "anim.webp")).toBe("webp");
    expect(buildClipboardImageFilename(1700000000999, 0, "image/jpeg")).toBe(
      "clipboard-1700000000999.jpg"
    );
    expect(buildClipboardImageFilename(1700000000999, 2, "image/webp")).toBe(
      "clipboard-1700000000999-2.webp"
    );
  });

  test("P0 Bug #2: WebVtTerminal.loadSnapshot preserves ?2004h, iterative escape stripping blocks nested breakout & smart composer fallback", () => {
    const vt = new WebVtTerminal(80, 24);
    expect(vt.bracketedPaste).toBe(false);
    expect(vt.bracketedPasteExplicit).toBeNull();

    vt.feedText("\x1b[?2004h\x1b[?1h");
    expect(vt.bracketedPaste).toBe(true);
    expect(vt.bracketedPasteExplicit).toBe(true);
    expect(vt.appCursorKeys).toBe(true);

    // Loading a plain screen snapshot MUST preserve existing ?2004h & ?1h modes
    vt.loadSnapshot("claude > ready for prompt\n");
    expect(vt.bracketedPaste).toBe(true);
    expect(vt.bracketedPasteExplicit).toBe(true);
    expect(vt.appCursorKeys).toBe(true);

    // Explicit \x1b[?2004l disables bracketed paste and records explicit=false
    vt.feedText("\x1b[?2004l");
    expect(vt.bracketedPaste).toBe(false);
    expect(vt.bracketedPasteExplicit).toBe(false);
    vt.loadSnapshot("$ ");
    expect(vt.bracketedPaste).toBe(false);
    expect(vt.bracketedPasteExplicit).toBe(false);

    // Nested \x1b[\x1b[201~201~ and 8-bit CSI \x9b201~ cannot reconstruct a breakout sequence
    const nestedEvil = "safe\x1b[\x1b[201~201~;touch /tmp/pwn\x9b\x9b200~200~end";
    const sanitized = sanitizeBracketedPasteText(nestedEvil);
    expect(sanitized).not.toContain("\x1b[201~");
    expect(sanitized).not.toContain("\x1b[200~");
    expect(sanitized).not.toContain("\x9b201~");
    expect(sanitized).not.toContain("\x9b200~");
    expect(wrapBracketedPaste(nestedEvil, true)).toBe(`\x1b[200~${sanitized}\x1b[201~`);
    expect(encodeComposerBracketedPasteSubmit(nestedEvil)).toBe(`\x1b[200~${sanitized}\x1b[201~\r`);

    // Smart composer mode: TUI agents default to bracketed paste unless ?2004l was explicitly sent; plain /bin/sh uses clean stdin
    const claudeTerm = {
      address: "t-claude",
      name: "claude",
      argv: ["claude"],
      cwd: "/repo",
      alive: true,
      status: { state: "idle", app: "claude" },
    };
    const shTerm = {
      address: "t-sh",
      name: "shell",
      argv: ["/bin/sh"],
      cwd: "/repo",
      alive: true,
      status: { state: "idle" },
    };
    expect(isKnownTuiAgentTerminal(claudeTerm)).toBe(true);
    expect(isKnownTuiAgentTerminal(shTerm)).toBe(false);
    expect(
      resolveEffectiveBracketedPaste({
        bracketedPaste: false,
        bracketedPasteExplicit: null,
        term: claudeTerm,
      })
    ).toBe(true);
    expect(
      resolveEffectiveBracketedPaste({
        bracketedPaste: false,
        bracketedPasteExplicit: false,
        term: claudeTerm,
      })
    ).toBe(false);
    expect(
      resolveEffectiveBracketedPaste({
        bracketedPaste: false,
        bracketedPasteExplicit: null,
        term: shTerm,
      })
    ).toBe(false);

    // Plain shell fallback never emits raw ^[[200~ garbage
    expect(encodeComposerSubmit("echo 1\r\necho 2\n", false)).toBe("echo 1\necho 2\r");
    expect(encodeComposerSubmit("echo 1\r\necho 2", true)).toBe("\x1b[200~echo 1\necho 2\x1b[201~\r");
  });

  test("P1 Bug #3 & UX Gap #3: Safari SpeechRecognition deduplication & E2EE daemon-proxied transcribe_audio wire contract", () => {
    // Simulate Safari webkitSpeechRecognition firing multiple events with resultIndex=0
    const tick1 = [{ isFinal: false, 0: { transcript: "hello" }, length: 1 }];
    const tick2 = [{ isFinal: true, 0: { transcript: "hello world" }, length: 1 }];
    const tick3 = [
      { isFinal: true, 0: { transcript: "hello world" }, length: 1 },
      { isFinal: false, 0: { transcript: "run cargo test" }, length: 1 },
    ];
    const tick4 = [
      { isFinal: true, 0: { transcript: "hello world" }, length: 1 },
      { isFinal: true, 0: { transcript: "run cargo test" }, length: 1 },
    ];

    const baseDraft = "Please ";
    expect(`${baseDraft}${rebuildSpeechRecognitionTranscript(tick1).combined}`).toBe("Please hello");
    expect(`${baseDraft}${rebuildSpeechRecognitionTranscript(tick2).combined}`).toBe(
      "Please hello world"
    );
    expect(`${baseDraft}${rebuildSpeechRecognitionTranscript(tick3).combined}`).toBe(
      "Please hello world run cargo test"
    );
    expect(`${baseDraft}${rebuildSpeechRecognitionTranscript(tick4).combined}`).toBe(
      "Please hello world run cargo test"
    );

    // E2EE round-trip for transcribe_audio / transcribe_audio_result
    const client = makeTestIdentity("web");
    const daemon = makeTestIdentity("host");
    const auth = createRelaySessionAuth(client, "c-v1", "d-v1");
    const sClient = BrowserE2EESession.deriveRelayV2({
      myStaticSecretB64: client.x25519SecretB64,
      peerStaticPublicB64: daemon.x25519PublicB64,
      sessionNonceB64: auth.session_nonce_b64,
      clientDeviceId: "c-v1",
      daemonDeviceId: "d-v1",
      isInitiator: true,
    });
    const sDaemon = BrowserE2EESession.deriveRelayV2({
      myStaticSecretB64: daemon.x25519SecretB64,
      peerStaticPublicB64: client.x25519PublicB64,
      sessionNonceB64: auth.session_nonce_b64,
      clientDeviceId: "c-v1",
      daemonDeviceId: "d-v1",
      isInitiator: false,
    });

    const req = sDaemon.openMessage(
      sClient.sealMessage({
        op: "transcribe_audio",
        req_id: "tr-42",
        audio_b64: toB64(new Uint8Array([1, 2, 3, 4])),
        mime_type: "audio/webm",
        filename: "voice.webm",
        language: "zh",
      })
    );
    expect(req.op).toBe("transcribe_audio");
    expect(req.req_id).toBe("tr-42");
    expect(req.filename).toBe("voice.webm");

    const res = sClient.openMessage(
      sDaemon.sealMessage({
        op: "transcribe_audio_result",
        req_id: "tr-42",
        ok: true,
        text: "查看当前分支状态",
        error: null,
      })
    );
    expect(res).toEqual({
      op: "transcribe_audio_result",
      req_id: "tr-42",
      ok: true,
      text: "查看当前分支状态",
      error: null,
    });
  });

  test("P1 Bug #4: Workspace Bootstrapper keystroke-by-keystroke auto-fill & manual-edit lock", () => {
    const url = "https://github.com/org/repo.git";
    let folder = "";
    const manual = false;

    for (let i = 1; i <= url.length; i += 1) {
      const typed = url.slice(0, i);
      folder = computeBootstrapAutoFolder(typed, folder, manual);
    }
    expect(folder).toBe("repo");

    // Partial scheme / host alone does not freeze folder on "h" or "github.com"
    expect(suggestFolderNameFromGitUrl("h")).toBe("");
    expect(suggestFolderNameFromGitUrl("https://")).toBe("");
    expect(suggestFolderNameFromGitUrl("https://github.com")).toBe("");
    expect(suggestFolderNameFromGitUrl("https://github.com/org/my-service.git")).toBe("my-service");

    // Manual edit locks custom folder name until cleared
    expect(
      computeBootstrapAutoFolder("https://github.com/org/other-repo.git", "custom-dir", true)
    ).toBe("custom-dir");
    expect(computeBootstrapAutoFolder("https://github.com/org/other-repo.git", "", false)).toBe(
      "other-repo"
    );
  });

  test("P1 Bug #5 & UX Gap #4: Jump-Needs-You shortcut avoids Alt+J hijack & Command Palette arrow cycling", () => {
    // ⌘J and Ctrl+Shift+J trigger Jump-Needs-You
    expect(
      isJumpNeedsYouShortcut({
        key: "j",
        code: "KeyJ",
        metaKey: true,
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
      })
    ).toBe(true);
    expect(
      isJumpNeedsYouShortcut({
        key: "J",
        code: "KeyJ",
        metaKey: false,
        ctrlKey: true,
        altKey: false,
        shiftKey: true,
      })
    ).toBe(true);

    // Alt+J / Option+J (macOS '∆' / Emacs / tmux M-j) and Ctrl+J (LF) MUST NOT be hijacked!
    expect(
      isJumpNeedsYouShortcut({
        key: "∆",
        code: "KeyJ",
        metaKey: false,
        ctrlKey: false,
        altKey: true,
        shiftKey: false,
      })
    ).toBe(false);
    expect(
      isJumpNeedsYouShortcut({
        key: "j",
        code: "KeyJ",
        metaKey: false,
        ctrlKey: false,
        altKey: true,
        shiftKey: false,
      })
    ).toBe(false);
    expect(
      isJumpNeedsYouShortcut({
        key: "j",
        code: "KeyJ",
        metaKey: false,
        ctrlKey: true,
        altKey: false,
        shiftKey: false,
      })
    ).toBe(false);

    // Alt+J encodes cleanly as ESC + j for terminal apps (Emacs / bash / zsh / tmux)
    expect(
      encodeTerminalKeyEvent(
        {
          key: "j",
          code: "KeyJ",
          altKey: true,
          ctrlKey: false,
          metaKey: false,
          shiftKey: false,
        },
        { appCursorKeys: false, imeComposing: false }
      )
    ).toBe("\x1bj");

    // ⌘K palette selection wraps cleanly on ArrowUp / ArrowDown
    expect(stepPaletteSelectionIndex(0, 5, 1)).toBe(1);
    expect(stepPaletteSelectionIndex(4, 5, 1)).toBe(0);
    expect(stepPaletteSelectionIndex(0, 5, -1)).toBe(4);
    expect(stepPaletteSelectionIndex(0, 0, 1)).toBe(0);
  });

  test("P1 Bug #8 & #9: ~ Spawn Picker root, Up navigation, dead vtTerminals pruning & Git Diff error/T-status/Open-File", () => {
    expect(resolveSpawnPickerInitialRoot({ spawnCwd: "~" })).toBe("~");
    expect(resolveSpawnPickerInitialRoot({ spawnCwd: "~/projects/coda" })).toBe("~/projects/coda");
    expect(resolveSpawnPickerInitialRoot({ spawnCwd: "", activeTermCwd: "" })).toBe("~");

    expect(canStepUpSpawnDirectory("~", "")).toBe(false);
    expect(canStepUpSpawnDirectory("~", "projects")).toBe(true);
    expect(canStepUpSpawnDirectory("~/projects", "")).toBe(true);
    expect(canStepUpSpawnDirectory("/", "")).toBe(false);
    expect(canStepUpSpawnDirectory("/Users/alice", "")).toBe(true);
    expect(parentAbsolutePath("~/projects/coda")).toBe("~/projects");
    expect(parentAbsolutePath("~/projects")).toBe("~");

    // Dead VT session pruning keeps only live/open terminals for the target peer
    const allKeys = ["peerA:t1", "peerA:t2-dead", "peerA:t3-sec", "peerB:other"];
    const stale = collectPrunableVtKeys(allKeys, "peerA", new Set(["t1", "t3-sec"]));
    expect(stale).toEqual(["peerA:t2-dead"]);

    // Git status T (typechange) and safe ' -> ' splitting only on R/C
    const statusParsed = parseGitStatusShort(
      [
        " T scripts/link-tool",
        ' M "docs/a -> b.md"',
        '?? "notes/x -> y.txt"',
        'C  "src/orig.ts" -> "src/copy.ts"',
      ].join("\n")
    );
    expect(statusParsed).toEqual([
      { status: "T", rawStatus: "T", path: "scripts/link-tool", origPath: null },
      { status: "M", rawStatus: "M", path: "docs/a -> b.md", origPath: null },
      { status: "??", rawStatus: "??", path: "notes/x -> y.txt", origPath: null },
      { status: "C", rawStatus: "C", path: "src/copy.ts", origPath: "src/orig.ts" },
    ]);

    expect(resolveGitDiffFileRoot("/repo/root", "/repo/root/web")).toBe("/repo/root");
    expect(resolveGitDiffFileRoot(null, "/repo/root/web")).toBe("/repo/root/web");

    // Non-git directory MUST NOT falsely claim "Working tree clean"
    const nonGit = createInitialGitDiffState();
    nonGit.open = true;
    nonGit.cwd = "/tmp/plain-folder";
    nonGit.isGitRepo = false;
    const nonGitHtml = renderGitDiffModal(nonGit, ["/tmp/plain-folder"], (en) => en);
    expect(nonGitHtml).toContain("Not a Git Repo");
    expect(nonGitHtml).not.toContain("Working tree clean");

    //Untracked file has one-click "Open File" button and does not say "Working tree clean"
    const untrackedSt = createInitialGitDiffState();
    untrackedSt.open = true;
    untrackedSt.cwd = "/repo/web";
    untrackedSt.repoRoot = "/repo";
    untrackedSt.branch = "main";
    untrackedSt.files = [{ status: "??", rawStatus: "??", path: "web/new.ts", origPath: null }];
    untrackedSt.selectedFile = "web/new.ts";
    const untrackedHtml = renderGitDiffModal(untrackedSt, ["/repo/web"], (en) => en);
    expect(untrackedHtml).toContain('data-gd-open-file="web/new.ts"');
    expect(untrackedHtml).not.toContain("Working tree clean");
  });

  test("P2 Bug #10 & UX Gap #2: Port Preview allows :// in query string, keeps rawHtml clean on truncation & surfaces redirects/SPA hint", () => {
    // Query parameter containing http:// must be allowed while full-URL path injection stays blocked
    expect(
      parseWebPreviewPortAndPath("3000", "/_next/image?url=http://127.0.0.1:3000/hero.png&w=640")
    ).toEqual({
      port: 3000,
      path: "/_next/image?url=http://127.0.0.1:3000/hero.png&w=640",
    });
    expect("error" in parseWebPreviewPortAndPath("3000", "http://evil.example.com/x")).toBe(true);

    // Truncated HTML keeps rawHtml clean (no appended marker inside srcdoc!) while setting truncated=true
    const sampleHtml = '<!doctype html><html><body><div id="root"></div><script type="module" src="/@vite/client"></script></body></html>';
    const decoded = decodePortPreviewPayload(
      toB64(new TextEncoder().encode(sampleHtml)),
      "text/html; charset=utf-8",
      true
    );
    expect(decoded.truncated).toBe(true);
    expect(decoded.rawHtml).toBe(sampleHtml);
    expect(decoded.rawHtml).not.toContain("[truncated");
    expect(detectClientRenderedSpaHtml(sampleHtml)).toBe(true);

    const pp = createInitialPortPreviewState();
    pp.open = true;
    pp.portInput = "5173";
    pp.pathInput = "/old";
    pp.statusCode = 302;
    pp.contentType = "text/html";
    pp.headers = [["Location", "http://127.0.0.1:5173/dashboard?tab=1"]];
    pp.rawHtml = decoded.rawHtml;
    pp.bodyText = decoded.bodyText;
    pp.truncated = true;
    pp.viewTab = "render";

    const modalHtml = renderPortPreviewModal(pp, "mac-studio", (en) => en);
    expect(modalHtml).toContain("Truncated");
    expect(modalHtml).toContain('data-pp-follow-path="/dashboard?tab=1"');
    expect(modalHtml).toContain("Client-rendered SPA detected");
    expect(modalHtml).not.toContain("[truncated at 128 KB]");
  });
});

describe("v1.1.0 Service Worker Web Push, Invite/Reset Deep-Links, Snippets & Onboarding", () => {
  test("base64url VAPID public key conversion, PushSubscription key extraction & iOS PWA Web Push UI", () => {
    // 65-byte uncompressed P-256 public key containing bytes that produce '-' (0xfb) and '_' (0xff)
    const rawVapidPub = new Uint8Array(65);
    rawVapidPub[0] = 0x04;
    for (let i = 1; i < 65; i += 1) {
      rawVapidPub[i] = i % 3 === 0 ? 0xfb : i % 5 === 0 ? 0xff : (i * 17) & 0xff;
    }

    const b64url = arrayBufferToBase64Url(rawVapidPub);
    expect(b64url).not.toContain("+");
    expect(b64url).not.toContain("/");
    expect(b64url).not.toContain("=");

    const decoded = vapidBase64UrlToUint8Array(b64url);
    expect(Array.from(decoded)).toEqual(Array.from(rawVapidPub));

    // Standard padded base64 is also accepted cleanly
    const stdB64 = toB64(rawVapidPub);
    expect(Array.from(vapidBase64UrlToUint8Array(stdB64))).toEqual(Array.from(rawVapidPub));

    // Reject empty, malformed length, and invalid characters
    expect(() => vapidBase64UrlToUint8Array("")).toThrow();
    expect(() => vapidBase64UrlToUint8Array("abcde")).toThrow();
    expect(() => vapidBase64UrlToUint8Array("bad$key!")).toThrow();

    // Extract PushSubscription p256dh + auth keys via getKey() and toJSON() fallback
    const rawAuth = new Uint8Array(16).map((_, i) => (0xf0 + i) & 0xff);
    const extracted = extractPushSubscriptionKeys({
      endpoint: "https://fcm.googleapis.com/fcm/send/sub-123",
      getKey: (name) => (name === "p256dh" ? rawVapidPub.buffer : rawAuth.buffer),
    });
    expect(extracted.endpoint).toBe("https://fcm.googleapis.com/fcm/send/sub-123");
    expect(extracted.p256dh).toBe(b64url);
    expect(extracted.auth).toBe(arrayBufferToBase64Url(rawAuth));

    const extractedFallback = extractPushSubscriptionKeys({
      toJSON: () => ({
        endpoint: "https://web.push.apple.com/Q123",
        keys: {
          p256dh: toB64(rawVapidPub),
          auth: toB64(rawAuth),
        },
      }),
    });
    expect(extractedFallback.endpoint).toBe("https://web.push.apple.com/Q123");
    expect(extractedFallback.p256dh).toBe(b64url);
    expect(extractedFallback.auth).toBe(arrayBufferToBase64Url(rawAuth));

    // iOS Safari non-standalone detection & 1-line Home Screen tip rendering
    expect(
      isIosNonStandaloneBrowser(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15",
        false
      )
    ).toBe(true);
    expect(
      isIosNonStandaloneBrowser(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15",
        true
      )
    ).toBe(false);
    expect(
      isIosNonStandaloneBrowser("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", false, 5)
    ).toBe(true);
    expect(
      isIosNonStandaloneBrowser("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", false, 0)
    ).toBe(false);

    const cardUnsubHtml = renderWebPushSettingsCard({
      vapid: { enabled: true, public_key: b64url },
      vapidLoaded: true,
      webPushCount: 0,
      thisDeviceSubscribed: false,
      busy: false,
      pushSupported: true,
      iosNeedsHomeScreenTip: true,
      tr: (en) => en,
    });
    expect(cardUnsubHtml).toContain("Enable Lock-Screen Push (Browser / iOS PWA)");
    expect(cardUnsubHtml).toContain(IOS_PWA_LOCK_SCREEN_PUSH_TIP_EN);

    const cardSubHtml = renderWebPushSettingsCard({
      vapid: { enabled: true, public_key: b64url },
      vapidLoaded: true,
      webPushCount: 2,
      thisDeviceSubscribed: true,
      busy: false,
      pushSupported: true,
      iosNeedsHomeScreenTip: false,
      tr: (en) => en,
    });
    expect(cardSubHtml).toContain("Disable on This Device");
    expect(cardSubHtml).toContain("Subscribed endpoints: 2");
  });

  test("invite deep-links, /reset-password token query parsing, validation & Admin Friends Directory", () => {
    expect(parseLocationRouteAndParams("/register", "?invite=CODA-7F29AB")).toEqual({
      route: "register",
      inviteCode: "CODA-7F29AB",
      resetToken: "",
      activationCode: "",
    });
    expect(parseLocationRouteAndParams("/", "?invite=CODA-FRIEND")).toEqual({
      route: "register",
      inviteCode: "CODA-FRIEND",
      resetToken: "",
      activationCode: "",
    });
    expect(
      parseLocationRouteAndParams("/reset-password", "?token=rst_AbCdEfGh1234567890")
    ).toEqual({
      route: "reset-password",
      inviteCode: "",
      resetToken: "rst_AbCdEfGh1234567890",
      activationCode: "",
    });
    expect(parseLocationRouteAndParams("/", "?code=abcd-1234")).toEqual({
      route: "activate",
      inviteCode: "",
      resetToken: "",
      activationCode: "ABCD-1234",
    });

    // extractResetTokenFromInput handles raw token, full URL, query fragment, and WeChat pasted message
    expect(extractResetTokenFromInput("  rst_rawTokenValue123456  ")).toBe("rst_rawTokenValue123456");
    expect(
      extractResetTokenFromInput(
        "https://codahub.dev/reset-password?token=rst_fromFullUrl987654321"
      )
    ).toBe("rst_fromFullUrl987654321");
    expect(
      extractResetTokenFromInput(
        "Here is your Coda reset link: <https://codahub.dev/reset-password?foo=1&token=rst_wechatMsgToken12345&utm=wechat#top>"
      )
    ).toBe("rst_wechatMsgToken12345");
    expect(extractResetTokenFromInput("https://codahub.dev/reset-password")).toBe("");

    expect(buildInviteRegistrationUrl("https://codahub.dev/", "CODA-999999")).toBe(
      "https://codahub.dev/register?invite=CODA-999999"
    );

    const trEn = (en: string) => en;
    expect(validateResetPasswordInputs("", "validpass123", "validpass123", trEn).ok).toBe(false);
    expect(validateResetPasswordInputs("rst_123456789", "short", "short", trEn).ok).toBe(false);
    expect(
      validateResetPasswordInputs("rst_123456789", "passwordA1", "passwordB2", trEn).ok
    ).toBe(false);
    const validReset = validateResetPasswordInputs(
      "https://codahub.dev/reset-password?token=rst_goodToken12345",
      "strongPass2026!",
      "strongPass2026!",
      trEn
    );
    expect(validReset).toEqual({
      ok: true,
      token: "rst_goodToken12345",
      newPassword: "strongPass2026!",
    });

    // Admin Friends Directory escapes HTML/XSS and renders Copy Reset Link button
    const friendsHtml = renderAdminFriendsDirectorySection({
      users: [
        {
          id: "u-1",
          handle: "bob",
          email: "bob@example.com",
          display_name: 'Bob <img src=x onerror="alert(1)">',
          is_admin: false,
          online_devices: 1,
          total_devices: 3,
          last_login_at: "2026-11-21T05:30:00Z",
          created_at: "2026-11-01T00:00:00Z",
        },
      ],
      busyUserId: null,
      latestResetLink: null,
      tr: trEn,
    });
    expect(friendsHtml).toContain("@bob");
    expect(friendsHtml).toContain("1/3 devices");
    expect(friendsHtml).toContain('data-admin-reset-user="u-1"');
    expect(friendsHtml).toContain("Copy Reset Link");
    expect(friendsHtml).not.toContain("<img src=x");
  });

  test("cross-device saved prompt snippets validation/rendering & 3-step onboarding card", () => {
    expect(DEFAULT_PROMPT_SNIPPETS.length).toBe(4);
    expect(isCustomPromptSnippet(DEFAULT_PROMPT_SNIPPETS[0])).toBe(false);
    expect(isCustomPromptSnippet({ id: "custom-1" })).toBe(true);

    expect(deriveSnippetTitleFromPrompt("  Check git diff and run cargo test --workspace\nMore")).toBe(
      "Check git diff and run cargo test -…"
    );

    expect(validatePromptSnippet({ title: "", prompt: "hello" }).ok).toBe(false);
    expect(validatePromptSnippet({ title: "x".repeat(61), prompt: "hello" }).ok).toBe(false);
    expect(validatePromptSnippet({ title: "ok", prompt: "" }).ok).toBe(false);
    expect(validatePromptSnippet({ title: "ok", prompt: "p".repeat(4001) }).ok).toBe(false);

    const validSnip = validatePromptSnippet({
      id: "c-1",
      title: 'Fix <script> & "Verify"',
      prompt: "Run bun test && cargo test",
    });
    expect(validSnip.ok).toBe(true);

    // normalizePromptSnippetsList enforces 24-item cap and deduplicates IDs
    const rawMany = Array.from({ length: 30 }, (_, idx) => ({
      id: idx === 1 ? "dup-0" : `dup-${idx}`,
      title: `Title ${idx}`,
      prompt: `Prompt ${idx}`,
    }));
    const normalized = normalizePromptSnippetsList(rawMany);
    expect(normalized.length).toBe(MAX_PROMPT_SNIPPETS);

    expect(insertSnippetIntoDraft("", "Run tests")).toBe("Run tests");
    expect(insertSnippetIntoDraft("First line", "Second line")).toBe("First line\nSecond line");

    const composerHtml = renderPromptComposerPanel({
      open: true,
      draft: "",
      isRecordingVoice: false,
      isTranscribingVoice: false,
      hasSpeechFallback: true,
      hasWhisperKey: false,
      snippets: [
        DEFAULT_PROMPT_SNIPPETS[0],
        { id: "custom-xss", title: "<b>Audit</b>", prompt: 'Check "security"' },
      ],
      savingSnippetOpen: true,
      savingSnippetTitle: "My Snippet",
      tr: (en) => en,
    });
    expect(composerHtml).toContain('data-snippet-insert="s1"');
    expect(composerHtml).not.toContain('data-snippet-delete="s1"');
    expect(composerHtml).toContain('data-snippet-insert="custom-xss"');
    expect(composerHtml).toContain('data-snippet-delete="custom-xss"');
    expect(composerHtml).toContain("&lt;b&gt;Audit&lt;/b&gt;");
    expect(composerHtml).toContain('id="btn-composer-save-snippet-toggle"');
    expect(composerHtml).toContain('id="composer-snippet-title-input"');

    const onboardingHtml = renderFriendOnboardingCard({
      installCmd: "curl -fsSL https://codahub.dev/install.sh | sh",
      copiedInstall: false,
      thisDevicePushSubscribed: false,
      webPushCount: 0,
      webPushBusy: false,
      iosNeedsHomeScreenTip: true,
      userHandle: "alice",
      tr: (en) => en,
    });
    expect(onboardingHtml).toContain("Step 1");
    expect(onboardingHtml).toContain("Step 2");
    expect(onboardingHtml).toContain("Step 3");
    expect(onboardingHtml).toContain("Claude Code / Codex / Grok / AGY");
    expect(onboardingHtml).toContain("Enable Lock-Screen Push");
    expect(onboardingHtml).toContain(IOS_PWA_LOCK_SCREEN_PUSH_TIP_EN);
  });

  test("Service Worker (/sw.js) CSP, fetch bypass rules, push/notificationclick handlers & build output", async () => {
    const origin = "https://codahub.dev";
    // NEVER intercept /api/, /v1/, /download/, /install.sh, /sw.js, non-GET, or cross-origin
    expect(shouldSwCacheStaticAsset(`${origin}/api/v1/auth/me`, origin, "GET")).toBe(false);
    expect(shouldSwCacheStaticAsset(`${origin}/v1/tunnel/ws`, origin, "GET")).toBe(false);
    expect(shouldSwCacheStaticAsset(`${origin}/download/desktop-macos.dmg`, origin, "GET")).toBe(
      false
    );
    expect(shouldSwCacheStaticAsset(`${origin}/install.sh`, origin, "GET")).toBe(false);
    expect(shouldSwCacheStaticAsset(`${origin}/sw.js`, origin, "GET")).toBe(false);
    expect(shouldSwCacheStaticAsset(`${origin}/console`, origin, "GET")).toBe(false);
    expect(shouldSwCacheStaticAsset(`${origin}/app.78e23166ea.js`, origin, "POST")).toBe(false);
    expect(shouldSwCacheStaticAsset("https://cdn.other.dev/app.78e23166ea.js", origin, "GET")).toBe(
      false
    );

    // Cache hashed bundle and icons
    expect(shouldSwCacheStaticAsset(`${origin}/app.78e23166ea.js`, origin, "GET")).toBe(true);
    expect(shouldSwCacheStaticAsset(`${origin}/logo.svg?v=78e23166ea`, origin, "GET")).toBe(true);
    expect(
      shouldSwCacheStaticAsset(`${origin}/apple-touch-icon.png?v=78e23166ea`, origin, "GET")
    ).toBe(true);

    // Push JSON parsing, plain-text fallback & safe target URL normalization
    expect(
      parseSwPushPayload(
        JSON.stringify({
          title: "Needs Approval · claude",
          body: "Waiting on Mac-Studio",
          url: "/console",
          tag: "coda-needs-you",
        }),
        origin
      )
    ).toEqual({
      title: "Needs Approval · claude",
      body: "Waiting on Mac-Studio",
      url: "/console",
      tag: "coda-needs-you",
    });
    expect(parseSwPushPayload("Plain alert message", origin)).toEqual({
      title: "Coda Agent Alert",
      body: "Plain alert message",
      url: "/console",
      tag: "coda-alert",
    });
    expect(normalizeSwNotificationTargetUrl("javascript:alert(1)", origin)).toBe("/console");
    expect(normalizeSwNotificationTargetUrl("https://evil.example/phish", origin)).toBe("/console");
    expect(normalizeSwNotificationTargetUrl("https://codahub.dev/console?tab=1", origin)).toBe(
      "/console?tab=1"
    );

    // Verify web/index.html CSP allows worker-src 'self'
    const indexHtml = readFileSync(new URL("../index.html", import.meta.url), "utf8");
    expect(indexHtml).toContain("worker-src 'self'");

    // Compile src/sw.ts via Bun.build IIFE (matching web/build.ts) and execute in a mock SW scope
    const swEntry = new URL("./sw.ts", import.meta.url).pathname;
    const buildRes = await Bun.build({
      entrypoints: [swEntry],
      minify: true,
      target: "browser",
      format: "iife",
    });
    expect(buildRes.success).toBe(true);
    const rawSwJs = (await buildRes.outputs[0].text()).replaceAll(
      "__CODA_SW_CACHE_VERSION__",
      "testhash01"
    );
    expect(rawSwJs).toContain("coda-static-testhash01");
    expect(rawSwJs).not.toContain("__CODA_SW_CACHE_VERSION__");
    expect(rawSwJs).not.toMatch(/\bexport\s*\{/);

    const listeners = new Map<string, (ev: any) => void>();
    const shownNotifications: Array<{ title: string; opts: any }> = [];
    let skippedWaiting = false;
    let claimedClients = false;
    let focusedConsoleClient = false;
    const deletedCaches: string[] = [];

    const mockSelf = {
      location: { origin },
      skipWaiting: async () => {
        skippedWaiting = true;
      },
      clients: {
        claim: async () => {
          claimedClients = true;
        },
        matchAll: async () => [
          {
            url: `${origin}/console`,
            focus: async () => {
              focusedConsoleClient = true;
              return {};
            },
          },
        ],
        openWindow: async () => null,
      },
      registration: {
        showNotification: async (title: string, opts: any) => {
          shownNotifications.push({ title, opts });
        },
      },
      addEventListener: (type: string, fn: (ev: any) => void) => {
        listeners.set(type, fn);
      },
    };
    const mockCaches = {
      keys: async () => ["coda-static-old000", "coda-static-testhash01"],
      delete: async (k: string) => {
        deletedCaches.push(k);
        return true;
      },
      open: async () => ({
        match: async () => undefined,
        put: async () => {},
      }),
    };

    const runSw = new Function("self", "caches", rawSwJs);
    runSw(mockSelf, mockCaches);

    expect(Array.from(listeners.keys()).sort()).toEqual([
      "activate",
      "fetch",
      "install",
      "notificationclick",
      "push",
    ]);

    let pendingPromise: Promise<unknown> = Promise.resolve();
    listeners.get("install")!({
      waitUntil: (p: Promise<unknown>) => {
        pendingPromise = p;
      },
    });
    await pendingPromise;
    expect(skippedWaiting).toBe(true);

    listeners.get("activate")!({
      waitUntil: (p: Promise<unknown>) => {
        pendingPromise = p;
      },
    });
    await pendingPromise;
    expect(deletedCaches).toEqual(["coda-static-old000"]);
    expect(claimedClients).toBe(true);

    listeners.get("push")!({
      data: {
        text: () =>
          JSON.stringify({
            title: "Action Needed · Codex",
            body: "Approval requested",
            url: "/console",
            tag: "coda-t1",
          }),
      },
      waitUntil: (p: Promise<unknown>) => {
        pendingPromise = p;
      },
    });
    await pendingPromise;
    expect(shownNotifications).toEqual([
      {
        title: "Action Needed · Codex",
        opts: {
          body: "Approval requested",
          icon: "/apple-touch-icon.png",
          badge: "/logo.svg",
          tag: "coda-t1",
          data: { url: "/console" },
        },
      },
    ]);

    let closedNotif = false;
    listeners.get("notificationclick")!({
      notification: {
        close: () => {
          closedNotif = true;
        },
        data: { url: "/console" },
      },
      waitUntil: (p: Promise<unknown>) => {
        pendingPromise = p;
      },
    });
    await pendingPromise;
    expect(closedNotif).toBe(true);
    expect(focusedConsoleClient).toBe(true);
  });

  test("VAPID key rotation mismatch detection, sign-out Web Push & local state cleanup helpers", async () => {
    const serverKeyA = new Uint8Array(65).map((_, idx) => (idx === 0 ? 0x04 : (idx * 7 + 3) & 0xff));
    const serverKeyB = new Uint8Array(serverKeyA);
    serverKeyB[64] ^= 0xff;

    // Exact match via ArrayBuffer and sliced Uint8Array view
    expect(isPushSubscriptionVapidKeyMismatch(serverKeyA.buffer.slice(0), serverKeyA)).toBe(false);
    const padded = new Uint8Array(80);
    padded.set(serverKeyA, 8);
    const subView = padded.subarray(8, 8 + 65);
    expect(isPushSubscriptionVapidKeyMismatch(subView, serverKeyA)).toBe(false);

    // Omitted options.applicationServerKey (undefined) does not force unsubscribe, while null or mismatched key does
    expect(isPushSubscriptionVapidKeyMismatch(undefined, serverKeyA)).toBe(false);
    expect(isPushSubscriptionVapidKeyMismatch(null, serverKeyA)).toBe(true);
    expect(isPushSubscriptionVapidKeyMismatch(serverKeyB.buffer, serverKeyA)).toBe(true);
    expect(isPushSubscriptionVapidKeyMismatch(new Uint8Array(32).buffer, serverKeyA)).toBe(true);

    // Sign-out unsubscribes server FIRST (while bearer token is valid), then browser PushSubscription
    const callOrder: string[] = [];
    const signOutRes = await teardownWebPushOnSignOut({
      getSubscription: async () => ({
        endpoint: "https://fcm.googleapis.com/fcm/send/tenant-a-endpoint",
        unsubscribe: async () => {
          callOrder.push("browser:unsubscribe");
          return true;
        },
      }),
      knownEndpoint: "https://fallback.example/push",
      hasAuthToken: true,
      unsubscribeServer: async (ep) => {
        callOrder.push(`server:unsubscribe:${ep}`);
      },
    });
    expect(callOrder).toEqual([
      "server:unsubscribe:https://fcm.googleapis.com/fcm/send/tenant-a-endpoint",
      "browser:unsubscribe",
    ]);
    expect(signOutRes).toEqual({
      endpoint: "https://fcm.googleapis.com/fcm/send/tenant-a-endpoint",
      unsubscribedServer: true,
      unsubscribedBrowser: true,
      webPushThisDeviceSubscribed: false,
      webPushEndpoint: "",
      webPushCount: 0,
    });

    // Even if server unsubscribe rejects (network offline / revoked session), browser subscription still unsubscribes cleanly
    let browserUnsubAfterServerErr = false;
    const offlineRes = await teardownWebPushOnSignOut({
      getSubscription: async () => ({
        endpoint: "https://web.push.apple.com/tenant-a",
        unsubscribe: async () => {
          browserUnsubAfterServerErr = true;
          return true;
        },
      }),
      hasAuthToken: true,
      unsubscribeServer: async () => {
        throw new Error("Network unreachable");
      },
    });
    expect(browserUnsubAfterServerErr).toBe(true);
    expect(offlineRes.unsubscribedServer).toBe(false);
    expect(offlineRes.unsubscribedBrowser).toBe(true);
    expect(offlineRes.webPushThisDeviceSubscribed).toBe(false);
    expect(offlineRes.webPushEndpoint).toBe("");
    expect(offlineRes.webPushCount).toBe(0);

    // Account-scoped storage cleanup clears coda_snippets_cache, resets snippets & namespaces onboarding per user.id
    const removedKeys: string[] = [];
    const cleaned = clearAccountScopedSignOutState({
      removeItem: (k) => removedKeys.push(k),
    });
    expect(removedKeys).toEqual([SNIPPETS_CACHE_STORAGE_KEY]);
    expect(cleaned.promptSnippets).toEqual([...DEFAULT_PROMPT_SNIPPETS]);
    expect(cleaned.webPushThisDeviceSubscribed).toBe(false);
    expect(cleaned.webPushEndpoint).toBe("");
    expect(cleaned.webPushCount).toBe(0);
    expect(cleaned.onboardingDismissed).toBe(false);

    expect(onboardingDismissedStorageKey("user-uuid-1")).toBe(
      "coda_onboarding_dismissed:user-uuid-1"
    );
    expect(onboardingDismissedStorageKey("user-uuid-2")).toBe(
      "coda_onboarding_dismissed:user-uuid-2"
    );
    expect(onboardingDismissedStorageKey("")).toBe("coda_onboarding_dismissed");
  });

  test("truthful /console empty-state rendering, connection indicators, ⌘D shortcut & clipboard image extraction", async () => {
    const trEn = (en: string) => en;

    // 1. 0 paired hosts: prompts to bind first host, zero misleading PostgreSQL/Local Herdr Core/Relay Ready text
    const zeroHostsHtml = renderConsoleEmptyStateHtml({
      devicesCount: 0,
      selectedDaemon: null,
      activeTerminalName: null,
      relayWsOpen: false,
      directWsOpen: false,
      transportMode: "relay_ws",
      rttMs: null,
      installCmd: 'curl -fsSL "https://codahub.dev/install.sh" | sh',
      tr: trEn,
    });
    expect(zeroHostsHtml).toContain("No remote hosts paired to this account yet.");
    expect(zeroHostsHtml).toContain("bind your first host");
    expect(zeroHostsHtml).toContain("&quot;https://codahub.dev/install.sh&quot;");
    expect(zeroHostsHtml).not.toContain("PostgreSQL 18");
    expect(zeroHostsHtml).not.toContain("Local Herdr Core online");
    expect(zeroHostsHtml).not.toContain("Relay WSS :443 Ready");

    const zeroVis = deriveConsoleConnectionVisualState({
      devicesCount: 0,
      selectedDaemon: null,
      relayWsOpen: true,
      directWsOpen: false,
      transportMode: "relay_ws",
      rttMs: null,
      tr: trEn,
    });
    expect(zeroVis.tone).toBe("idle");
    expect(zeroVis.sidebarColorCss).not.toBe("var(--emerald)");
    expect(zeroVis.railDotBgCss).not.toBe("var(--emerald)");
    expect(
      formatConsoleHeaderSubtitle({
        devicesCount: 0,
        selectedDaemon: null,
        activeCwd: null,
        activeBranch: null,
        tr: trEn,
      })
    ).toBe("No hosts paired");

    // 2. Offline host: shows "Host <name> is currently offline" (HTML-escaped), non-green status dot, no fake "main" branch
    const offlineHtml = renderConsoleEmptyStateHtml({
      devicesCount: 1,
      selectedDaemon: { name: "prod-box<1>", is_online: false },
      activeTerminalName: null,
      relayWsOpen: true,
      directWsOpen: false,
      transportMode: "relay_ws",
      rttMs: null,
      installCmd: "curl -fsSL https://codahub.dev/install.sh | sh",
      tr: trEn,
    });
    expect(offlineHtml).toContain("Host prod-box&lt;1&gt; is currently offline.");
    expect(offlineHtml).not.toContain("Local Herdr Core online");

    const offlineVis = deriveConsoleConnectionVisualState({
      devicesCount: 1,
      selectedDaemon: { name: "prod-box", is_online: false },
      relayWsOpen: true,
      directWsOpen: false,
      transportMode: "relay_ws",
      rttMs: null,
      tr: trEn,
    });
    expect(offlineVis.tone).toBe("offline");
    expect(offlineVis.railDotBgCss).toBe("var(--text-quaternary)");
    expect(offlineVis.railTitle).toContain("Host prod-box is currently offline");
    expect(
      formatConsoleHeaderSubtitle({
        devicesCount: 1,
        selectedDaemon: { name: "prod-box", is_online: false },
        activeCwd: undefined,
        activeBranch: undefined,
        tr: trEn,
      })
    ).toBe("prod-box (offline)");

    // 3. Online host with 0 terminals: prompts "Press + to launch Claude Code, Codex, Grok, AGY, or Shell"
    const onlineEmptyHtml = renderConsoleEmptyStateHtml({
      devicesCount: 1,
      selectedDaemon: { name: "Mac-Studio", is_online: true },
      activeTerminalName: null,
      relayWsOpen: true,
      directWsOpen: false,
      transportMode: "relay_ws",
      rttMs: 18,
      installCmd: "curl -fsSL https://codahub.dev/install.sh | sh",
      tr: trEn,
    });
    expect(onlineEmptyHtml).toContain(
      "Press + to launch Claude Code, Codex, Grok, AGY, or Shell."
    );
    expect(onlineEmptyHtml).toContain("RTT 18ms");

    const connectingVis = deriveConsoleConnectionVisualState({
      devicesCount: 1,
      selectedDaemon: { name: "Mac-Studio", is_online: true },
      relayWsOpen: false,
      directWsOpen: false,
      transportMode: "relay_ws",
      rttMs: null,
      tr: trEn,
    });
    expect(connectingVis.tone).toBe("connecting");
    expect(connectingVis.railDotBgCss).toBe("var(--amber)");

    const onlineVis = deriveConsoleConnectionVisualState({
      devicesCount: 1,
      selectedDaemon: { name: "Mac-Studio", is_online: true },
      relayWsOpen: true,
      directWsOpen: false,
      transportMode: "relay_ws",
      rttMs: 14,
      tr: trEn,
    });
    expect(onlineVis.tone).toBe("online");
    expect(onlineVis.railDotBgCss).toBe("var(--emerald)");

    // Header subtitle includes git branch ONLY when present, never defaulting to "main"
    expect(
      formatConsoleHeaderSubtitle({
        devicesCount: 1,
        selectedDaemon: { name: "Mac-Studio", is_online: true },
        activeCwd: "/tmp/scratch",
        activeBranch: null,
        tr: trEn,
      })
    ).toBe("/tmp/scratch &middot; Mac-Studio");
    expect(
      formatConsoleHeaderSubtitle({
        devicesCount: 1,
        selectedDaemon: { name: "Mac-Studio", is_online: true },
        activeCwd: "/opt/coda",
        activeBranch: "feat/web-polish",
        tr: trEn,
      })
    ).toBe("/opt/coda &middot; feat/web-polish &middot; Mac-Studio");

    // 4. ⌘D / Ctrl+Shift+D Split View shortcut preserves bare Ctrl+D (\x04 EOF) and Alt+D (\x1bd)
    expect(isToggleSplitViewShortcut({ key: "d", metaKey: true })).toBe(true);
    expect(isToggleSplitViewShortcut({ key: "D", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(isToggleSplitViewShortcut({ key: "d", ctrlKey: true })).toBe(false);
    expect(isToggleSplitViewShortcut({ key: "d", altKey: true })).toBe(false);
    expect(encodeTerminalKeyEvent({ key: "d", ctrlKey: true })).toBe("\x04");

    // 5. Password min-length helper & clipboard image item extraction & landing /activate link check
    expect(validateNewPasswordLength("1234567", trEn).ok).toBe(false);
    expect(validateNewPasswordLength("12345678", trEn).ok).toBe(true);

    const pngBlob = new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: "image/png" });
    const extractedImages = await extractImagesFromClipboardItems([
      { types: ["text/plain"], getType: async () => new Blob(["hi"], { type: "text/plain" }) },
      { types: ["image/png"], getType: async () => pngBlob },
    ]);
    expect(extractedImages.length).toBe(1);
    expect(extractedImages[0].type).toBe("image/png");

    const appTsSrc = readFileSync(new URL("./app.ts", import.meta.url), "utf8");
    expect(appTsSrc).not.toContain("/activate?code=CODA-7F29");
    expect(appTsSrc).not.toContain("Local Herdr Core online");
  });
});
