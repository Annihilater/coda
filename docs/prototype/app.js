(() => {
  "use strict";

  // ===================== Shared Live State (v1.1.0) =====================
  const ROUTES = [
    { id: "lan", labelZh: "⚡ 局域网直连 · 1.4ms", labelEn: "⚡ LAN Direct · 1.4ms", short: "LAN P2P", isRelay: false },
    { id: "ipv6", labelZh: "🚀 公网 IPv6 直连 · 9.2ms", labelEn: "🚀 CN IPv6 P2P · 9.2ms", short: "IPv6 P2P", isRelay: false },
    { id: "rawtcp", labelZh: "🔒 国内免备案 TCP :24443 · 24ms", labelEn: "🔒 CN Raw TCP :24443 · 24ms", short: "CN TCP", isRelay: true },
    { id: "wss", labelZh: "☁️ 海外 CDN WSS · 48ms", labelEn: "☁️ CDN WSS · 48ms", short: "CDN WSS", isRelay: true },
  ];

  const AGENTS = [
    { id: "claude", name: "Claude Code", badge: "Claude Code", descZh: "Anthropic CLI · OSC 7501 权限感知", descEn: "Anthropic CLI · OSC 7501 hooks" },
    { id: "codex", name: "Codex", badge: "Codex", descZh: "OpenAI Coding Agent · 全双工 PTY", descEn: "OpenAI Coding Agent · Full-duplex PTY" },
    { id: "grok", name: "Grok CLI", badge: "Grok CLI", descZh: "xAI Coding Agent · 极速推理流", descEn: "xAI Coding Agent · Fast streaming" },
    { id: "agy", name: "Antigravity (AGY)", badge: "Antigravity (AGY)", descZh: "Google DeepMind Agentic CLI", descEn: "Google DeepMind Agentic CLI" },
    { id: "gemini", name: "Gemini CLI", badge: "Gemini CLI", descZh: "Google Gemini 2.5 Pro CLI", descEn: "Google Gemini 2.5 Pro CLI" },
    { id: "shell", name: "Shell (Zsh/Bash)", badge: "Shell", descZh: "herdr-core 托管或原生 PTY 兜底", descEn: "herdr-core managed or native PTY" },
  ];

  const state = {
    lang: "zh",
    platform: "mac_gpui", // web_desktop | web_mobile | mac_gpui | ipad | iphone | iphone_duo
    routeIdx: 0,
    showNotes: true,
    showOnboarding: false,
    webSubView: "workbench", // workbench | landing | activate | invite | reset
    webSplit: false,
    webMobileDrawer: false,
    webMobileKeypad: true,
    macSplit: false,
    macRightDrawer: "changes", // null | changes | files | ports | mesh
    macAppUpdated: false,
    allowRemoteControl: true,
    ipadSidebar: true,
    iphoneTab: "inbox", // inbox | spaces
    iphoneScreen: "list", // list | terminal
    iphoneKeypad: false,
    duoPosture: "inner_landscape", // cover | inner_landscape | tabletop
    duoSplitMode: "split", // split | diff | dual | canvas
    inspectorTab: "changes", // changes | files | ports
    selectedFilePath: "web/src/sw.ts",
    selectedPort: 5173,
    previewMethod: "GET",
    termSearchOpen: false,
    termSearchQuery: "",
    activeSessionId: "s1",
    secondarySessionId: "s2",
    voiceRecording: false,
    voiceMode: "native_e2ee", // native_e2ee | https_whisper
    voiceDraft: "",
    attachedShot: null,
    launcherHostId: "h-macmini",
    launcherBootMode: "folder", // folder | clone
    launcherFolder: "~/workspace/coda",
    launcherCloneUrl: "https://github.com/Annihilater/coda.git",
    launcherCloneDir: "~/workspace/coda-fork",
    settingsTab: "alerts", // alerts | friends | hosts | security | voice
    webPushEnabled: true,
    webPushCount: 2,
    webhooks: {
      bark: true,
      telegram: true,
      feishu: false,
      discord: false,
    },
    inviteCode: "CODA-8F3K9A",
    lastResetToken: "rst_9xK2mP7vQ4nL8wR5tY3jH6bN1cF4zW8q",
    lastResetHandle: "alice",
    friends: [
      { id: "u-vampire", handle: "vampire", displayName: "Vampire (Admin)", isAdmin: true, onlineDevices: 3, totalDevices: 4, lastLogin: "2m ago" },
      { id: "u-alice", handle: "alice", displayName: "Alice · iOS + Mac", isAdmin: false, onlineDevices: 2, totalDevices: 2, lastLogin: "14m ago" },
      { id: "u-bob", handle: "bob", displayName: "Bob · GPU Lab", isAdmin: false, onlineDevices: 1, totalDevices: 2, lastLogin: "3h ago" },
    ],
    snippets: [
      { id: "sn1", titleZh: "⚡ 跑全量测试并修复", titleEn: "⚡ Run Tests & Fix", prompt: "Run the full test suite, fix any failures or compiler warnings, and summarize what changed.", builtin: true },
      { id: "sn2", titleZh: "🔍 审查 Git Diff", titleEn: "🔍 Review Git Diff", prompt: "Review the current git diff carefully for bugs, edge cases, or security regressions before committing.", builtin: true },
      { id: "sn3", titleZh: "🚀 继续推进到底", titleEn: "🚀 Continue", prompt: "Continue with the remaining tasks until everything is verified and clean.", builtin: true },
      { id: "sn4", titleZh: "🧹 规范化提交", titleEn: "🧹 Clean Commit", prompt: "Stage and commit the current changes with a concise conventional commit message.", builtin: true },
    ],
    hosts: [
      {
        id: "h-macmini",
        name: "Mac mini · 家里主力",
        platform: "macos-aarch64",
        role: "hybrid",
        online: true,
        cpu: "18%",
        ram: "11.4/32G",
        daemonVer: "v1.1.0",
        needsOta: false,
        branches: ["main", "feat/vapid-push", "feat/sw-push"],
      },
      {
        id: "h-gpu01",
        name: "prod-gpu-01 · 阿里云",
        platform: "linux-x86_64",
        role: "daemon",
        online: true,
        cpu: "42%",
        ram: "26.8/64G",
        daemonVer: "v1.0.0",
        needsOta: true,
        branches: ["main", "feat/snippets-sync"],
      },
      {
        id: "h-euler",
        name: "openeuler-arm64",
        platform: "linux-aarch64",
        role: "daemon",
        online: false,
        cpu: "—",
        ram: "—",
        daemonVer: "v1.1.0",
        needsOta: false,
        branches: ["main"],
      },
    ],
    sessions: [
      {
        id: "s1",
        hostId: "h-macmini",
        hostName: "Mac mini · 家里主力",
        workspace: "coda",
        color: "#2563eb",
        branch: "feat/vapid-push",
        agent: "Claude Code",
        title: "实现 W3C VAPID 锁屏推送与好友重置深链",
        state: "needs_you",
        statusMsg: "Claude Code 请求执行 `cargo test -p coda-server` 并校验 aes128gcm 载荷",
        diffAdded: 214,
        diffDeleted: 28,
        lines: [
          { cls: "t-cyan", text: "╭─ Claude Code · coda (feat/vapid-push) · herdr-core t1" },
          { cls: "", text: "│ 已完成 RFC 8292 ES256 VAPID 签名与 RFC 8291 aes128gcm 锁屏加密模块：" },
          { cls: "hunk", text: "@@ -88,6 +88,19 @@ pub async fn dispatch_web_push_alert(" },
          { cls: "add", text: "+  let sealed = encrypt_aes128gcm_payload(&sub.p256dh, &sub.auth, plaintext)?;" },
          { cls: "add", text: "+  // DNS-pinned SSRF guard + auto-prune 404/410 stale endpoints" },
          { cls: "add", text: "+  ?? web/src/sw.ts (untracked · green synthetic diff ready)" },
          { cls: "", text: "" },
          { cls: "t-amber", text: "⚠ Permission required: Run `cargo test -p coda-server && cd web && bun test`?" },
          { cls: "prompt", text: "❯ 1. Yes, allow once" },
          { cls: "", text: "  2. Always allow for this workspace" },
          { cls: "", text: "  3. Cancel (Esc)" },
        ],
      },
      {
        id: "s2",
        hostId: "h-macmini",
        hostName: "Mac mini · 家里主力",
        workspace: "coda-web",
        color: "#0891b2",
        branch: "feat/sw-push",
        agent: "Antigravity (AGY)",
        title: "联调 Service Worker 锁屏通知与 :5173 预览",
        state: "working",
        statusMsg: "AGY serving live preview on 127.0.0.1:5173 & testing sw.js",
        diffAdded: 126,
        diffDeleted: 14,
        lines: [
          { cls: "t-cyan", text: "╭─ Antigravity (AGY) · coda-web (feat/sw-push) · herdr-core t2" },
          { cls: "t-dim", text: "$ bun run build && bun test" },
          { cls: "t-green", text: "✓ Emitted dist/manifest.json + dist/sw.js (CSP worker-src 'self' verified)" },
          { cls: "", text: "  Local dev server listening on http://127.0.0.1:5173 (tunneled via E2EE)" },
          { cls: "t-dim", text: "  Synced 4 prompt snippets from GET /api/v1/me/snippets" },
        ],
      },
      {
        id: "s3",
        hostId: "h-macmini",
        hostName: "Mac mini · 家里主力",
        workspace: "coda-desktop",
        color: "#d97706",
        branch: "feat/self-update",
        agent: "Grok CLI",
        title: "验证 Coda.app SHA256SUMS 应用内自更新器",
        state: "needs_you",
        statusMsg: "Grok CLI 请求执行 `cargo run -p coda-desktop -- --self-test`",
        diffAdded: 168,
        diffDeleted: 19,
        lines: [
          { cls: "t-cyan", text: "╭─ Grok CLI · coda-desktop (feat/self-update) · herdr-core t3" },
          { cls: "", text: "│ 已实现 self_update.rs：校验 SHA256SUMS + 剥离 quarantine + 原子替换重启" },
          { cls: "t-amber", text: "⚠ OSC 7501 [permission]: Run `cargo run -p coda-desktop -- --self-test`?" },
          { cls: "prompt", text: "❯ 1. Yes, allow once   2. Always   Esc. Cancel" },
        ],
      },
      {
        id: "s4",
        hostId: "h-gpu01",
        hostName: "prod-gpu-01 · 阿里云",
        workspace: "coda-server",
        color: "#059669",
        branch: "feat/snippets-sync",
        agent: "Codex",
        title: "压测双栈 IPv6 直连与跨设备 Snippets 同步",
        state: "working",
        statusMsg: "Running cargo test -p coda-daemon -- --nocapture",
        diffAdded: 64,
        diffDeleted: 11,
        lines: [
          { cls: "t-cyan", text: "codex@prod-gpu-01 ~/coda-server (feat/snippets-sync)" },
          { cls: "t-dim", text: "$ coda-daemon status" },
          { cls: "t-green", text: "● daemon online · pid=41822 · herdr-core v0.1.24 · E2EE Whisper ready" },
          { cls: "", text: "  primary relay : wss://codahub.dev/v1/tunnel/ws (connected, 44ms)" },
          { cls: "", text: "  cn raw tcp    : cn-relay.codahub.dev:24443 (connected, 19ms)" },
          { cls: "", text: "  direct p2p    : [::]:24444 (dual-stack IPv4+IPv6 ready)" },
        ],
      },
      {
        id: "s5",
        hostId: "h-macmini",
        hostName: "Mac mini · 家里主力",
        workspace: "coda-apple",
        color: "#7c3aed",
        branch: "main",
        agent: "Gemini CLI",
        title: "验证 Swift 6 原生语音、相册直传与折叠屏重排",
        state: "done",
        unread: true,
        statusMsg: "All Apple Swift 6 CodaSelfTest assertions passed",
        diffAdded: 94,
        diffDeleted: 12,
        lines: [
          { cls: "t-dim", text: "$ cd apple && swift build && swift run CodaSelfTest" },
          { cls: "t-green", text: "[OK] ALL CODA APPLE NATIVE SELF-TESTS PASSED!" },
          { cls: "t-dim", text: "✓ CodaPromptSnippet roundtrip + extractResetTokenFromInput verified" },
          { cls: "t-dim", text: "✓ SFSpeechRecognizer zero-config fallback + PhotosPicker upload verified" },
        ],
      },
    ],
    files: [
      {
        path: "web/src/sw.ts",
        status: "??",
        added: 78,
        del: 0,
        snippet: [
          "// Coda v1.1.0 Zero-Config W3C Web Push Service Worker",
          "self.addEventListener('push', (event) => {",
          "  const data = event.data ? event.data.json() : { title: 'Coda Alert' };",
          "  event.waitUntil(self.registration.showNotification(data.title, {",
          "    body: data.body, icon: '/apple-touch-icon.png', data: { url: data.url || '/console' }",
          "  }));",
          "});",
        ],
      },
      {
        path: "crates/coda-server/src/web_push.rs",
        status: "A",
        added: 210,
        del: 0,
        snippet: [
          "/// RFC 8292 VAPID ES256 + RFC 8291 aes128gcm payload encryption (Zero-Knowledge metadata)",
          "pub fn encrypt_web_push_aes128gcm(p256dh: &[u8], auth: &[u8], payload: &[u8]) -> Result<Vec<u8>> {",
          "  // Ephemeral P-256 ECDH + HKDF-SHA256 + AES-128-GCM (rs=4096)",
          "  seal_rfc8291_record(p256dh, auth, payload)",
          "}",
        ],
      },
      {
        path: "crates/coda-desktop/src/self_update.rs",
        status: "M",
        added: 142,
        del: 16,
        snippet: [
          "pub fn verify_sha256sums_and_stage(archive: &[u8], sums_txt: &str, asset: &str) -> Result<()> {",
          "  let expected = parse_expected_sha256(sums_txt, asset)?;",
          "  ensure_ConstantTimeEq(sha256_hex(archive), expected)?;",
          "  strip_macos_quarantine_and_atomic_rename()",
          "}",
        ],
      },
      {
        path: "apple/Sources/CodaKit/CodaCoreModels.swift",
        status: "M",
        added: 88,
        del: 8,
        snippet: [
          "public struct CodaPromptSnippet: Codable, Identifiable, Equatable, Sendable {",
          "  public let id: String",
          "  public var title: String",
          "  public var prompt: String",
          "}",
        ],
      },
    ],
    ports: [
      { port: 5173, localProxy: 54173, name: "Vite · Coda Web Preview", proto: "HTTP/1.1 Full-Method", status: "200 OK" },
      { port: 8080, localProxy: 58080, name: "coda-server · Local API", proto: "GET/POST/PUT/DELETE", status: "200 OK" },
    ],
  };

  // ===================== Safe Escaping & Core Helpers =====================
  const t = (zh, en) => (state.lang === "zh" ? zh : en);
  const esc = (s) =>
    String(s ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");

  const activeSession = () => state.sessions.find((s) => s.id === state.activeSessionId) || state.sessions[0];
  const secondarySession = () =>
    state.sessions.find((s) => s.id === state.secondarySessionId && s.id !== state.activeSessionId) ||
    state.sessions.find((s) => s.id !== state.activeSessionId) ||
    state.sessions[0];
  const needsYouSessions = () => state.sessions.filter((s) => s.state === "needs_you");
  const attentionCount = () => state.sessions.filter((s) => s.state === "needs_you" || (s.state === "done" && s.unread)).length;

  let toastTimer = null;
  function showToast(msg) {
    const el = document.getElementById("proto-toast");
    el.textContent = msg;
    el.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.classList.add("hidden"), 2900);
  }

  function badgeHtml(s) {
    const map = {
      needs_you: t("待确认", "Needs you"),
      working: t("运行中", "Working"),
      done: s.unread ? t("已完成 · 新", "Done · New") : t("已完成", "Done"),
      idle: t("空闲", "Idle"),
    };
    const st = esc(s.state);
    return `<span class="badge ${st}">${esc(map[s.state] || s.state)}</span>`;
  }

  function avatarHtml(s, size = 28) {
    const ch = esc((s.workspace || "C")[0].toUpperCase());
    const col = esc(s.color || "#2563eb");
    const sz = Number(size) || 28;
    const fsz = Math.round(sz * 0.44);
    return `<span class="avatar" style="width:${sz}px;height:${sz}px;background:${col}28;border:1px solid ${col}66;color:${col};font-size:${fsz}px;">${ch}</span>`;
  }

  function selectSession(id, enterMobileTerm = false) {
    state.activeSessionId = id;
    const s = activeSession();
    if (s && s.unread) s.unread = false;
    if (enterMobileTerm) {
      state.iphoneScreen = "terminal";
      state.webMobileDrawer = false;
    }
    render();
  }

  function jumpNextNeedsYou() {
    const list = needsYouSessions();
    if (list.length === 0) {
      showToast(t("🎉 当前没有待确认阻塞的会话 (Needs-You = 0)", "🎉 Zero blocked Needs-You sessions right now"));
      return;
    }
    const curIdx = list.findIndex((s) => s.id === state.activeSessionId);
    const next = list[(curIdx + 1) % list.length];
    state.activeSessionId = next.id;
    if (state.platform === "iphone") state.iphoneScreen = "terminal";
    showToast(t(`⌘J 已跳转至待确认会话：${next.workspace} (${next.agent})`, `⌘J jumped to Needs-You: ${next.workspace} (${next.agent})`));
    render();
  }

  function approveSession(id, choice) {
    const s = state.sessions.find((x) => x.id === id) || activeSession();
    if (!s) return;
    if (choice === "esc") {
      s.lines.push({ cls: "t-amber", text: "^C · Interrupted by user" });
      s.state = "idle";
      s.statusMsg = "Interrupted";
      showToast(t(`已向 ${s.workspace} 发送中断信号 (Esc / ^C)`, `Interrupted ${s.workspace}`));
    } else {
      s.lines.push({ cls: "t-green", text: `> ${choice} · Approved over E2EE (${ROUTES[state.routeIdx].short})` });
      s.lines.push({ cls: "t-dim", text: "$ cargo test -p coda-server && cd web && bun test" });
      s.lines.push({ cls: "t-green", text: "✓ All tests passed (0 warnings) · VAPID + sw.js verified" });
      s.state = "done";
      s.unread = false;
      s.statusMsg = "Completed · All tests green";
      showToast(t(`已批准 ${s.workspace}（全端状态与角标已实时同步）`, `Approved ${s.workspace} (synced across all 6 views)`));
    }
    render();
  }

  function sendCommand(text, targetId = state.activeSessionId) {
    const clean = (text || "").trim();
    const s = state.sessions.find((x) => x.id === targetId) || activeSession();
    if (!s) return;
    if (!clean && !state.attachedShot) {
      s.lines.push({ cls: "t-dim", text: "↵ (bare Enter sent via pty_input)" });
    } else {
      const payload = clean || state.attachedShot;
      s.lines.push({ cls: "t-cyan", text: `❯ ${payload}` });
      s.lines.push({
        cls: "t-dim",
        text: `[?2004h Bracketed Paste · E2EE ${ROUTES[state.routeIdx].short}] delivered to ${s.agent} (${s.hostName})`,
      });
      if (s.state === "idle" || s.state === "done") s.state = "working";
    }
    state.voiceDraft = "";
    state.attachedShot = null;
    render();
  }

  function toggleVoice() {
    if (state.voiceRecording) {
      state.voiceRecording = false;
      const sample = t(
        "检查未跟踪文件的 Git Diff 补丁并验证本地端口隧道",
        "Review untracked file Git diffs and verify localhost port tunnel"
      );
      state.voiceDraft = state.voiceDraft ? `${state.voiceDraft} ${sample}` : sample;
      const engineLabel =
        state.voiceMode === "native_e2ee"
          ? t("免配置原生语音 / E2EE Daemon Whisper", "Zero-Config Speech / E2EE Daemon Whisper")
          : "HTTPS Whisper API";
      showToast(`🎙 ${engineLabel}: ${t("转写已填入输入框", "Transcript inserted")}`);
    } else {
      state.voiceRecording = true;
      showToast(
        t(
          "🎙 正在听写（支持 SpeechRecognition / SFSpeechRecognizer 免配置与 E2EE TranscribeAudio）...",
          "🎙 Listening (Zero-config native speech + E2EE Daemon TranscribeAudio)..."
        )
      );
    }
    render();
  }

  function simulateScreenshotPaste() {
    const shotPath = "/tmp/coda-uploads/clipboard-shot-v11.png";
    state.attachedShot = shotPath;
    if (!state.voiceDraft.includes(shotPath)) {
      state.voiceDraft = state.voiceDraft ? `${state.voiceDraft} ${shotPath}` : `${t("请对照这张截图修复 UI 细节：", "Fix the UI based on this screenshot:")} ${shotPath}`;
    }
    showToast(
      t(
        "🖼 已通过 E2EE upload_terminal_file 加密上传剪贴板截图并插入路径",
        "🖼 Screenshot uploaded via E2EE upload_terminal_file & path inserted"
      )
    );
    render();
  }

  function applySnippet(snipId) {
    const snip = state.snippets.find((x) => x.id === snipId);
    if (!snip) return;
    state.voiceDraft = snip.prompt;
    const title = state.lang === "zh" ? snip.titleZh : snip.titleEn;
    showToast(t(`⚡ 已填入云同步快捷指令「${title}」`, `⚡ Inserted synced snippet "${title}"`));
    render();
  }

  function saveCurrentDraftAsSnippet() {
    const text = (state.voiceDraft || "").trim();
    const prompt = text || "Run cargo check --workspace && cd web && bun test && summarize status.";
    const shortTitle = prompt.slice(0, 18) + (prompt.length > 18 ? "…" : "");
    const id = "sn-custom-" + Date.now();
    state.snippets.push({
      id,
      titleZh: `📌 ${shortTitle}`,
      titleEn: `📌 ${shortTitle}`,
      prompt,
      builtin: false,
    });
    showToast(t("☁️ 已保存至 PUT /api/v1/me/snippets 并同步全端", "☁️ Saved via PUT /api/v1/me/snippets & synced across devices"));
    render();
  }

  function deleteSnippet(snipId) {
    state.snippets = state.snippets.filter((x) => x.id !== snipId);
    showToast(t("已删除自定义快捷指令并同步云端", "Deleted custom snippet & synced"));
    render();
  }

  function exportSessionMarkdown(s) {
    const target = s || activeSession();
    showToast(
      t(
        `📄 已导出纯净 Markdown 记录：coda-${target.workspace}-${target.id}.md（自动剥离 ANSI 控制符）`,
        `📄 Exported clean Markdown transcript: coda-${target.workspace}-${target.id}.md`
      )
    );
  }

  function upgradeHostOta(hostId) {
    const h = state.hosts.find((x) => x.id === hostId);
    if (!h) return;
    h.daemonVer = "v1.1.0";
    h.needsOta = false;
    showToast(
      t(
        `🚀 ${h.name} 已完成 SHA256SUMS 校验 + SIGUSR1 零中断热升级至 v1.1.0（活跃 PTY 零断连）`,
        `🚀 ${h.name} upgraded to v1.1.0 via SHA256SUMS + SIGUSR1 zero-downtime OTA`
      )
    );
    render();
  }

  function triggerMacSelfUpdate() {
    if (state.macAppUpdated) {
      showToast(t("✓ Coda.app 当前已是最新 v1.1.0 版本", "✓ Coda.app is already up-to-date on v1.1.0"));
      return;
    }
    state.macAppUpdated = true;
    showToast(
      t(
        "✨ Coda.app 已校验 SHA256SUMS、清理 quarantine 属性并完成 v1.1.0 原子热更新！",
        "✨ Coda.app verified SHA256SUMS, stripped quarantine & self-updated to v1.1.0!"
      )
    );
    render();
  }

  function launchNewSession(agentId) {
    const agentObj = AGENTS.find((a) => a.id === agentId) || AGENTS[0];
    const host = state.hosts.find((h) => h.id === state.launcherHostId) || state.hosts[0];
    const isClone = state.launcherBootMode === "clone";
    const rawDir = isClone ? state.launcherCloneDir : state.launcherFolder;
    const wsName = (rawDir.split("/").filter(Boolean).pop() || "coda").replace(/[^a-zA-Z0-9._-]/g, "");
    const id = "s" + (state.sessions.length + 1);
    const colors = ["#0891b2", "#db2777", "#4f46e5", "#0d9488", "#ea580c"];
    const bootLines = isClone
      ? [
          { cls: "t-green", text: `✓ [BootstrapWorkspace] Cloned ${state.launcherCloneUrl} -> ${rawDir}` },
          { cls: "t-cyan", text: `● Launched ${agentObj.name} on ${host.name} (${rawDir})` },
        ]
      : [{ cls: "t-cyan", text: `● Launched ${agentObj.name} on ${host.name} (${rawDir})` }];
    const next = {
      id,
      hostId: host.id,
      hostName: host.name,
      workspace: wsName || "coda",
      color: colors[state.sessions.length % colors.length],
      branch: host.branches[0] || "main",
      agent: agentObj.name,
      title: `${agentObj.name} · ${isClone ? t("克隆并启动", "Cloned Workspace") : t("新建会话", "New Session")}`,
      state: "working",
      statusMsg: `Running ${agentObj.name} in ${rawDir}`,
      diffAdded: 0,
      diffDeleted: 0,
      lines: [
        ...bootLines,
        { cls: "t-dim", text: `  E2EE ChaCha20-Poly1305 + zstd active via ${ROUTES[state.routeIdx].short}` },
        { cls: "", text: `${agentObj.id}> Ready.` },
      ],
    };
    state.sessions.unshift(next);
    state.activeSessionId = id;
    closeModal();
    showToast(
      isClone
        ? t(`已在 ${host.name} 克隆仓库并启动 ${agentObj.name}`, `Cloned repo & launched ${agentObj.name} on ${host.name}`)
        : t(`已在 ${host.name} (${rawDir}) 启动 ${agentObj.name}`, `Launched ${agentObj.name} on ${host.name}`)
    );
    render();
  }

  // ===================== Shared UI Fragments =====================
  function renderHighlightedText(raw, query) {
    const str = String(raw ?? "");
    if (!str) return "&nbsp;";
    const q = String(query ?? "").trim();
    if (!q) return esc(str);
    const lowerStr = str.toLowerCase();
    const lowerQ = q.toLowerCase();
    let out = "";
    let idx = 0;
    while (idx < str.length) {
      const hit = lowerStr.indexOf(lowerQ, idx);
      if (hit === -1) {
        out += esc(str.slice(idx));
        break;
      }
      if (hit > idx) out += esc(str.slice(idx, hit));
      out += `<mark class="t-match">${esc(str.slice(hit, hit + q.length))}</mark>`;
      idx = hit + q.length;
    }
    return out;
  }

  function countSearchMatches(s, query) {
    const q = String(query ?? "").trim().toLowerCase();
    if (!q || !s) return 0;
    let total = 0;
    for (const l of s.lines) {
      const txt = String(l.text ?? "").toLowerCase();
      let pos = 0;
      while (pos < txt.length) {
        const hit = txt.indexOf(q, pos);
        if (hit === -1) break;
        total += 1;
        pos = hit + q.length;
      }
    }
    return total;
  }

  function renderTerminalSearchBar(s) {
    if (!state.termSearchOpen) return "";
    const hits = countSearchMatches(s, state.termSearchQuery);
    return `
      <div class="term-search-bar">
        <span class="mono" style="font-size:11px;color:var(--sky);">🔍 ⌘F</span>
        <input id="proto-term-search-input" class="term-search-input" placeholder="${esc(t("搜索终端历史回滚 (如 VAPID / sw.js / SHA256)...", "Search terminal scrollback (e.g. VAPID / sw.js / SHA256)..."))}" value="${esc(state.termSearchQuery)}" />
        <span class="mono" style="font-size:10.5px;color:var(--muted);">${ state.termSearchQuery.trim() ? `${hits} ${esc(t("处匹配", "matches"))}` : esc(t("实时高亮", "Live match")) }</span>
        <button type="button" class="icon-btn" style="width:22px;height:22px;" data-close-term-search title="Esc">✕</button>
      </div>
    `;
  }

  function renderTerminalLines(s) {
    const q = state.termSearchOpen ? state.termSearchQuery : "";
    return (
      s.lines
        .map((l) => `<div class="t-line ${esc(l.cls || "")}">${renderHighlightedText(l.text, q)}</div>`)
        .join("") + `<div class="t-line"><span class="t-cursor"></span></div>`
    );
  }

  function renderAttentionSidebarBox() {
    const list = needsYouSessions();
    if (list.length === 0) return "";
    return `
      <div class="attn-box">
        <div class="attn-head">
          <span>▲ ${esc(t("待确认收件箱", "NEEDS ATTENTION"))} (${list.length})</span>
          <button type="button" class="pill-btn amber" style="padding:1px 6px;font-size:9.5px;" data-jump-needs>⌘J ${esc(t("速跳", "Next"))}</button>
        </div>
        ${list
          .map(
            (item) => `
            <div class="attn-item">
              <button type="button" class="attn-item-btn" data-select-sid="${esc(item.id)}" data-enter-mobile="1">
                ${esc(item.workspace)} · <span style="color:var(--amber);">${esc(item.agent)}</span>
              </button>
              <button type="button" class="pill-btn amber" style="padding:1px 6px;font-size:9.5px;" data-approve="1" data-sid="${esc(item.id)}">Yes</button>
            </div>`
          )
          .join("")}
      </div>
    `;
  }

  function renderHostRowCompact(h) {
    return `
      <div style="display:flex;flex-direction:column;gap:4px;padding:6px 8px;border-radius:8px;background:rgba(255,255,255,0.025);border:1px solid rgba(255,255,255,0.04);">
        <div style="display:flex;align-items:center;justify-content:space-between;font-size:11.5px;">
          <span style="display:flex;align-items:center;gap:6px;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:600;">
            <span class="dot ${h.online ? "online" : "offline"}"></span>${esc(h.name)}
          </span>
          <span class="mono" style="font-size:9.5px;color:var(--dim);">${esc(h.role)}</span>
        </div>
        <div class="health-row">
          <span class="health-pill">CPU ${esc(h.cpu)}</span>
          <span class="health-pill">RAM ${esc(h.ram)}</span>
          ${
            h.needsOta
              ? `<button type="button" class="ota-pill" data-upgrade-host="${esc(h.id)}">⚡ OTA v1.1.0</button>`
              : `<span class="health-pill" style="color:var(--green);border-color:rgba(34,197,94,0.3);">${esc(h.daemonVer)}</span>`
          }
        </div>
      </div>
    `;
  }

  function renderOnboardingCard() {
    if (!state.showOnboarding) return "";
    return `
      <div class="onboard-card">
        <div style="display:flex;align-items:center;justify-content:space-between;">
          <strong style="font-size:12px;color:var(--sky);">👋 ${esc(t("Coda v1.1.0 好友三步极速上手卡片（空状态引导预览）", "Coda v1.1.0 3-Step Friend Quickstart Card"))}</strong>
          <button type="button" class="icon-btn" style="width:22px;height:22px;" data-toggle-onboarding>✕</button>
        </div>
        <div class="onboard-steps">
          <div class="onboard-step">
            <strong style="color:var(--text);">1 · ${esc(t("一行命令绑定主机", "1-Line Bind Host"))}</strong>
            <span style="color:var(--muted);font-size:10.5px;">curl -fsSL https://codahub.dev/install.sh | sh</span>
            <button type="button" class="pill-btn" style="margin-top:auto;font-size:10px;" data-toast="${esc(t("📋 已复制一键安装命令", "📋 Copied 1-line install command"))}">${esc(t("复制安装命令", "Copy Command"))}</button>
          </div>
          <div class="onboard-step">
            <strong style="color:var(--text);">2 · ${esc(t("启动 5 大 AI Agent", "Launch 5 AI Agents"))}</strong>
            <span style="color:var(--muted);font-size:10.5px;">Claude · Codex · Grok · AGY · Gemini</span>
            <button type="button" class="pill-btn sky" style="margin-top:auto;font-size:10px;" data-open-modal="launcher">+ ${esc(t("打开启动器", "Open Launcher"))}</button>
          </div>
          <div class="onboard-step">
            <strong style="color:var(--text);">3 · ${esc(t("开启锁屏推送通知", "1-Click Lock-Screen Push"))}</strong>
            <span style="color:var(--muted);font-size:10.5px;">W3C VAPID (sw.js) + Bark / Telegram</span>
            <button type="button" class="pill-btn amber" style="margin-top:auto;font-size:10px;" data-open-settings-tab="alerts">🔔 ${esc(t("一键配置推送", "Enable Push"))}</button>
          </div>
        </div>
      </div>
    `;
  }

  function renderNeedsBanner(s, compact = false) {
    if (s.state !== "needs_you") return "";
    return `
      <div class="needs-banner">
        <div class="needs-banner-top">
          <span style="color:var(--amber);">▲</span>
          <span>${esc(s.statusMsg)}</span>
        </div>
        <div class="needs-banner-actions">
          <button type="button" class="pill-btn amber" data-approve="1" data-sid="${esc(s.id)}">1 · Yes</button>
          <button type="button" class="pill-btn amber" data-approve="2" data-sid="${esc(s.id)}">2 · Always</button>
          <button type="button" class="pill-btn" data-approve="esc" data-sid="${esc(s.id)}">Esc</button>
          ${compact ? "" : `<span class="mono" style="margin-left:auto;font-size:10.5px;color:var(--muted);">OSC 7501 · Zero-Knowledge Alert</span>`}
        </div>
      </div>
    `;
  }

  function renderSnippetStrip(compact = false) {
    return `
      <div class="snippet-bar">
        ${state.snippets
          .map((sn) => {
            const label = state.lang === "zh" ? sn.titleZh : sn.titleEn;
            return `<button type="button" class="snippet-chip" data-apply-snippet="${esc(sn.id)}" title="${esc(sn.prompt)}">${esc(label)}${
              !sn.builtin && !compact ? `<span data-del-snippet="${esc(sn.id)}" style="margin-left:3px;opacity:0.75;">×</span>` : ""
            }</button>`;
          })
          .join("")}
        <button type="button" class="snippet-chip save" data-save-snippet>+ ${esc(t("存为快捷指令", "Save Snippet"))}</button>
      </div>
    `;
  }

  function renderComposerBar(s, compact = false) {
    return `
      <div class="composer-stack">
        ${renderSnippetStrip(compact)}
        ${
          state.attachedShot
            ? `<div class="attach-chip">🖼 ${esc(state.attachedShot)} <button type="button" style="border:none;background:transparent;color:inherit;font-weight:700;" data-clear-shot>✕</button></div>`
            : ""
        }
        <form style="display:flex;align-items:center;gap:6px;" data-cmd-form="${esc(s.id)}">
          <input class="term-input" placeholder="${esc(
            compact
              ? t("一步直发 / ?2004h 粘贴...", "Send or paste...")
              : t("直接发指令或点击上方云同步 Snippet（支持 ⌘V 截图直传 & 免配置语音）...", "Type prompt, click a synced Snippet, paste ⌘V screenshot, or dictate...")
          )}" value="${esc(state.voiceDraft)}" />
          <button type="button" class="pill-btn" data-paste-shot title="${esc(t("模拟 ⌘V 剪贴板截图 / 相册上传", "Simulate ⌘V screenshot / PhotosPicker upload"))}">📎${compact ? "" : ` ${esc(t("⌘V图", "⌘V Img"))}`}</button>
          <button type="button" class="mic-btn ${state.voiceRecording ? "recording" : ""}" data-toggle-voice title="SpeechRecognition / SFSpeechRecognizer / E2EE Whisper">
            🎙${compact ? "" : ` ${esc(state.voiceRecording ? t("停止", "Stop") : t("语音", "Voice"))}`}
          </button>
          <button type="submit" class="pill-btn primary">${esc(compact ? "↵" : t("发送", "Send"))}</button>
        </form>
      </div>
    `;
  }

  function renderKeypad(sid) {
    const id = esc(sid);
    return `
      <div class="keypad">
        <div class="keypad-grid">
          <div class="keypad-row">
            <button type="button" class="k-btn" data-key="esc" data-sid="${id}">esc</button>
            <button type="button" class="k-btn" data-key="ctrl" data-sid="${id}">ctrl</button>
            <button type="button" class="k-btn" data-key="tab" data-sid="${id}">tab</button>
            <button type="button" class="k-btn" data-key="⇧tab" data-sid="${id}">⇧tab</button>
            <button type="button" class="k-btn" data-key="↑" data-sid="${id}">↑</button>
            <button type="button" class="k-btn" data-key="⌫" data-sid="${id}">⌫</button>
          </div>
          <div class="keypad-row">
            <button type="button" class="k-btn danger" data-key="^C" data-sid="${id}">^C</button>
            <button type="button" class="k-btn" data-key="^D" data-sid="${id}">^D</button>
            <button type="button" class="k-btn" data-key="/" data-sid="${id}">/</button>
            <button type="button" class="k-btn" data-key="←" data-sid="${id}">←</button>
            <button type="button" class="k-btn" data-key="↓" data-sid="${id}">↓</button>
            <button type="button" class="k-btn" data-key="→" data-sid="${id}">→</button>
          </div>
        </div>
        <button type="button" class="k-enter" data-key="enter" data-sid="${id}">↵</button>
      </div>
    `;
  }

  // ===================== Unified E2EE Workspace Inspector =====================
  function renderUnifiedInspector(s, isGpui = false) {
    const tab = state.inspectorTab;
    const selectedFile = state.files.find((f) => f.path === state.selectedFilePath) || state.files[0];
    const activePort = state.ports.find((p) => p.port === state.selectedPort) || state.ports[0];

    let bodyHtml = "";
    if (tab === "changes") {
      bodyHtml = `
        <div style="padding:8px 10px;border-bottom:1px solid #1e222b;display:flex;align-items:center;justify-content:space-between;gap:6px;">
          <div>
            <div style="font-size:12px;font-weight:600;">${esc(s.workspace)} · ${esc(s.branch)}</div>
            <div class="mono" style="font-size:10px;color:var(--muted);">repo_root: ~/workspace/${esc(s.workspace)} · <span style="color:var(--green);">+${Number(s.diffAdded)}</span> <span style="color:var(--red);">-${Number(s.diffDeleted)}</span></div>
          </div>
          <span class="badge working">E2EE Diff</span>
        </div>
        <div class="term-screen" style="font-size:11px;padding:10px;">
          <div class="t-line" style="display:flex;align-items:center;justify-content:space-between;margin-bottom:3px;">
            <span class="t-green">?? web/src/sw.ts</span>
            <span class="untracked-pill">${esc(t("?? 未跟踪新文件", "?? Untracked +"))}</span>
          </div>
          <div class="t-line t-cyan">A  crates/coda-server/src/web_push.rs</div>
          <div class="t-line t-cyan">M  crates/coda-desktop/src/self_update.rs</div>
          <div class="t-line">&nbsp;</div>
          <div class="t-line hunk">@@ -0,0 +1,12 @@ web/src/sw.ts (synthetic untracked diff)</div>
          <div class="t-line add">+self.addEventListener("install", () =&gt; self.skipWaiting());</div>
          <div class="t-line add">+self.addEventListener("push", (ev) =&gt; {</div>
          <div class="t-line add">+  const msg = ev.data?.json() ?? { title: "Coda Agent Alert" };</div>
          <div class="t-line add">+  ev.waitUntil(self.registration.showNotification(msg.title, {</div>
          <div class="t-line add">+    body: msg.body, tag: msg.tag || "coda-alert"</div>
          <div class="t-line add">+  }));</div>
          <div class="t-line add">+});</div>
          <div class="t-line">&nbsp;</div>
          <div class="t-line hunk">@@ -44,6 +44,9 @@ crates/coda-desktop/src/self_update.rs</div>
          <div class="t-line del">-  let skip_verify = false;</div>
          <div class="t-line add">+  verify_sha256sums_manifest(&amp;tarball_bytes, &amp;sums_text, &amp;asset_name)?;</div>
          <div class="t-line add">+  strip_quarantine_and_atomic_swap(&amp;staged_bin, &amp;target_exe)?;</div>
        </div>
      `;
    } else if (tab === "files") {
      bodyHtml = `
        <div class="right-drawer-body">
          <div class="sec-label"><span>${esc(t("工作区文件树 (点击预览)", "E2EE FILE TREE"))}</span><span>${state.files.length}</span></div>
          ${state.files
            .map((f) => {
              const isSel = f.path === selectedFile.path;
              const pill =
                f.status === "??"
                  ? `<span class="untracked-pill">??</span>`
                  : `<span class="mono" style="font-size:10px;color:${f.status === "A" ? "var(--green)" : "var(--amber)"};">${esc(f.status)}</span>`;
              return `
                <button type="button" class="host-card" style="width:100%;text-align:left;border-color:${isSel ? "rgba(56,189,248,0.55)" : "var(--border)"};" data-select-file="${esc(f.path)}">
                  <span class="mono" style="font-size:10.5px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${esc(f.path)}</span>
                  ${pill}
                </button>`;
            })
            .join("")}
          <div class="sec-label" style="margin-top:4px;"><span>${esc(selectedFile.path)}</span><span style="color:var(--sky);">E2EE</span></div>
          <div class="preview-sandbox">
            <div class="term-screen" style="font-size:10.5px;padding:8px 10px;max-height:210px;">
              ${selectedFile.snippet.map((line) => `<div class="t-line">${esc(line)}</div>`).join("")}
            </div>
          </div>
        </div>
      `;
    } else {
      bodyHtml = `
        <div class="right-drawer-body">
          <div class="sec-label"><span>${esc(t("远端本地端口探测", "REMOTE LOCALHOST PORTS"))}</span><span>${state.ports.length}</span></div>
          <div style="display:flex;gap:5px;">
            ${state.ports
              .map(
                (p) => `
              <button type="button" class="pill-btn ${p.port === activePort.port ? "sky" : ""}" style="flex:1;justify-content:center;" data-select-port="${Number(p.port)}">:${Number(p.port)}</button>`
              )
              .join("")}
          </div>
          ${
            isGpui
              ? `
            <div class="tunnel-pill">
              <span>⚡ http://127.0.0.1:${Number(activePort.localProxy)} → :${Number(activePort.port)}</span>
              <button type="button" class="pill-btn" style="padding:1px 6px;font-size:9.5px;" data-toast="${esc(
                t(
                  `已复制本地全方法回环隧道 http://127.0.0.1:${activePort.localProxy} (支持 GET/POST/PUT/PATCH/DELETE + Cookie)`,
                  `Copied full-method loopback tunnel http://127.0.0.1:${activePort.localProxy}`
                )
              )}">${esc(t("复制", "Copy"))}</button>
            </div>`
              : `
            <div class="mono" style="font-size:10px;color:var(--green);padding:2px 2px;">
              🔒 ${esc(t("沙箱化 E2EE HttpLocalPreview (免公网暴露端口)", "Sandboxed E2EE HttpLocalPreview (Zero public port exposure)"))}
            </div>`
          }
          <div class="preview-sandbox">
            <div class="preview-sandbox-bar">
              <div style="display:flex;gap:4px;align-items:center;">
                <button type="button" class="pill-btn ${state.previewMethod === "GET" ? "green" : ""}" style="padding:1px 5px;font-size:9.5px;" data-preview-method="GET">GET</button>
                <button type="button" class="pill-btn ${state.previewMethod === "POST" ? "amber" : ""}" style="padding:1px 5px;font-size:9.5px;" data-preview-method="POST">POST</button>
                <span>:${Number(activePort.port)}${state.previewMethod === "POST" ? "/api/test" : "/"}</span>
              </div>
              <span style="color:var(--green);">${esc(activePort.status)}</span>
            </div>
            <div class="preview-sandbox-body">
              <div style="font-weight:600;color:var(--text);">${esc(activePort.name)}</div>
              <div class="mono" style="font-size:10px;color:var(--muted);">
                ${
                  state.previewMethod === "POST"
                    ? esc('{"ok":true,"method":"POST","set_cookie":"coda_dev=1","e2ee":true}')
                    : esc(t("实时渲染远端 127.0.0.1 页面 · 支持表单提交与热更新预览", "Live remote 127.0.0.1 render · Full-method headers & body passthrough"))
                }
              </div>
              <div style="display:flex;gap:5px;margin-top:2px;">
                <span class="badge working">Location / Set-Cookie</span>
                <span class="badge idle">Zero-Port-Leak</span>
              </div>
            </div>
          </div>
        </div>
      `;
    }

    return `
      <div class="insp-tabs">
        <button type="button" class="insp-tab ${tab === "changes" ? "active" : ""}" data-insp-tab="changes">Changes (??)</button>
        <button type="button" class="insp-tab ${tab === "files" ? "active" : ""}" data-insp-tab="files">Files</button>
        <button type="button" class="insp-tab ${tab === "ports" ? "active" : ""}" data-insp-tab="ports">${esc(isGpui ? "127.0.0.1 Tunnel" : "Port Preview")}</button>
      </div>
      ${bodyHtml}
    `;
  }

  // ===================== 1. Web Desktop Console =====================
  function renderWebDesktop() {
    const s = activeSession();
    const s2 = secondarySession();
    const route = ROUTES[state.routeIdx];
    const urlPath =
      state.webSubView === "landing"
        ? "https://codahub.dev/"
        : state.webSubView === "activate"
        ? "https://codahub.dev/activate?code=K7M9-4X2Q"
        : state.webSubView === "invite"
        ? `https://codahub.dev/register?invite=${state.inviteCode}`
        : state.webSubView === "reset"
        ? `https://codahub.dev/reset-password?token=${state.lastResetToken.slice(0, 18)}...`
        : "https://codahub.dev/console";

    let inner = "";
    if (state.webSubView === "landing") {
      inner = `
        <div class="landing-wrap">
          <div style="display:flex;align-items:center;justify-content:space-between;">
            <div style="display:flex;align-items:center;gap:8px;font-weight:700;font-size:15px;">
              <span class="dot online"></span> Coda <span class="mono" style="font-size:11px;color:var(--green);">v1.1.0</span>
              <span class="badge working">5 AI Agents · W3C VAPID Push · Zero-Downtime OTA</span>
            </div>
            <div style="display:flex;gap:8px;">
              <button type="button" class="pill-btn" data-web-sub="invite">${esc(t("好友邀请 /register", "Invite /register"))}</button>
              <button type="button" class="pill-btn" data-web-sub="activate">${esc(t("设备授权 /activate", "Device /activate"))}</button>
              <button type="button" class="pill-btn primary" data-web-sub="workbench">${esc(t("进入 Web 工作台 →", "Open Console →"))}</button>
            </div>
          </div>
          <div class="landing-hero">
            <div>
              <h1>${esc(t("把家用 Mac、Linux 服务器与手机连成一张零知识加密 AI 开发网", "One zero-knowledge encrypted AI agent mesh for Mac, Linux & Mobile"))}</h1>
              <p>${esc(
                t(
                  "原生编排 Claude Code · Codex · Grok CLI · Antigravity (AGY) · Gemini CLI；内置 E2EE Git Diff (含 ?? 新文件)、本地端口预览、免配置语音 + ⌘V 截图直传、W3C VAPID 锁屏推送与零中断 OTA。",
                  "Orchestrate Claude Code, Codex, Grok CLI, Antigravity (AGY) & Gemini CLI with E2EE Git Diff, Port Preview, Voice + ⌘V Screenshot Paste, 1-Click Lock-Screen Web Push & OTA."
                )
              )}</p>
              <div style="display:flex;gap:8px;flex-wrap:wrap;">
                <button type="button" class="pill-btn primary" data-web-sub="workbench">${esc(t("立即体验 v1.1.0 工作台", "Launch v1.1.0 Console"))}</button>
                <button type="button" class="pill-btn" data-open-settings-tab="alerts">🔔 ${esc(t("锁屏推送与告警", "Lock-Screen Push"))}</button>
                <button type="button" class="pill-btn" data-open-settings-tab="friends">🤝 ${esc(t("好友邀请与重置链接", "Invite & Reset Links"))}</button>
              </div>
            </div>
            <div class="dl-card" style="background:#090b10;">
              <div class="mono" style="font-size:11px;color:var(--green);">$ curl -fsSL https://codahub.dev/install.sh | sh</div>
              <div class="mono" style="font-size:11px;color:var(--muted);margin-top:6px;">
                ==&gt; [1/4] herdr-core verified (Claude / Codex / Grok / AGY / Gemini)<br/>
                ==&gt; [2/4] coda-daemon v1.1.0 SHA256SUMS OK (SIGUSR1 OTA ready)<br/>
                ==&gt; [3/4] Browser approval: K7M9-4X2Q<br/>
                ==&gt; [4/4] LAN IPv6 P2P + CN Raw TCP :24443 + CDN WSS active
              </div>
            </div>
          </div>
          <div class="dl-grid">
            <div class="dl-card">
              <h4>macOS Desktop (Coda.app v1.1.0)</h4>
              <p>Rust GPUI Metal 120Hz · 应用内一键自更新 (SHA256SUMS) · 127.0.0.1 全方法端口隧道 + ⌘V 截图粘贴</p>
            </div>
            <div class="dl-card">
              <h4>Web PWA &amp; W3C Lock-Screen Push</h4>
              <p>sw.js Service Worker · RFC 8292 VAPID + aes128gcm 原生锁屏通知 · 跨设备云同步 Saved Snippets</p>
            </div>
            <div class="dl-card">
              <h4>Apple Swift 6 (iPhone / iPad / Duo)</h4>
              <p>GhosttyReflow 折叠屏三态 · 免配置 SFSpeechRecognizer + PhotosPicker 直传 · 好友邀请与重置</p>
            </div>
          </div>
        </div>
      `;
    } else if (state.webSubView === "activate") {
      inner = `
        <div class="landing-wrap" style="align-items:center;justify-content:center;">
          <div class="dl-card" style="max-width:430px;width:100%;padding:22px;gap:12px;">
            <div style="font-size:16px;font-weight:700;">${esc(t("批准主机入网 (RFC 8628)", "Approve Host Login (RFC 8628)"))}</div>
            <p>${esc(t("正在将新设备绑定到当前账号 @vampire（Ed25519/X25519 公钥指纹已校验）", "Bind this device to @vampire with verified Ed25519/X25519 keys"))}</p>
            <div class="mono" style="font-size:26px;font-weight:700;color:var(--amber);text-align:center;padding:12px;background:#08090c;border-radius:10px;border:1px solid var(--border);">K7M9-4X2Q</div>
            <div style="font-size:12px;color:var(--muted);">
              ${esc(t("主机名称", "Host"))}: <strong style="color:var(--text);">Mac mini · 家里主力</strong> · <span class="mono">macos-aarch64 / v1.1.0</span>
            </div>
            <div style="display:flex;gap:8px;margin-top:4px;">
              <button type="button" class="pill-btn primary" style="flex:1;justify-content:center;padding:8px;" data-web-sub="workbench" data-toast="${esc(t("已批准主机 K7M9-4X2Q 入网", "Approved K7M9-4X2Q"))}">${esc(t("确认批准入网", "Approve Device"))}</button>
              <button type="button" class="pill-btn" data-web-sub="workbench">${esc(t("返回工作台", "Back"))}</button>
            </div>
          </div>
        </div>
      `;
    } else if (state.webSubView === "invite") {
      inner = `
        <div class="landing-wrap" style="align-items:center;justify-content:center;">
          <div class="dl-card" style="max-width:440px;width:100%;padding:22px;gap:10px;">
            <div style="display:flex;align-items:center;justify-content:space-between;">
              <div style="font-size:16px;font-weight:700;">🤝 ${esc(t("好友受邀注册", "Friend Invite Registration"))}</div>
              <span class="badge working">/register?invite=</span>
            </div>
            <p>${esc(t("已从微信深链自动填入好友邀请码，注册后即刻拥有独立零知识加密空间：", "Invite code pre-populated from deep-link — isolated zero-knowledge tenant:"))}</p>
            <input class="term-input" value="${esc(state.inviteCode)}" readonly style="color:var(--green);font-weight:700;" />
            <input class="term-input" placeholder="${esc(t("用户名 (@handle，如 @charlie)", "Handle (e.g. @charlie)"))}" value="charlie" />
            <input class="term-input" type="password" placeholder="${esc(t("设置登录密码（至少 8 位）", "Password (min 8 chars)"))}" value="••••••••••••" />
            <div style="display:flex;gap:8px;margin-top:4px;">
              <button type="button" class="pill-btn primary" style="flex:1;justify-content:center;padding:8px;" data-complete-invite>${esc(t("完成注册并查看三步引导 →", "Create Account & Start →"))}</button>
              <button type="button" class="pill-btn" data-web-sub="reset">${esc(t("忘记密码？", "Forgot Password?"))}</button>
            </div>
          </div>
        </div>
      `;
    } else if (state.webSubView === "reset") {
      inner = `
        <div class="landing-wrap" style="align-items:center;justify-content:center;">
          <div class="dl-card" style="max-width:450px;width:100%;padding:22px;gap:10px;">
            <div style="display:flex;align-items:center;justify-content:space-between;">
              <div style="font-size:16px;font-weight:700;">🔑 ${esc(t("免邮件单次安全重置密码", "One-Time Token Password Reset"))}</div>
              <span class="badge needs_you">SHA-256 · 24h</span>
            </div>
            <p>${esc(
              t(
                "粘贴管理员通过微信发送的重置链接或 rst_ 令牌；重置后自动吊销旧登录会话，但保留已配对主机的 daemon 隧道票据不断连。",
                "Paste the admin's one-time reset link or rst_ token; revokes old full sessions while keeping paired daemon relay tickets online."
              )
            )}</p>
            <input class="term-input" value="${esc(state.lastResetToken)}" />
            <input class="term-input" type="password" placeholder="${esc(t("输入新密码（至少 8 位）", "New password (min 8 chars)"))}" value="••••••••••••" />
            <div style="display:flex;gap:8px;margin-top:4px;">
              <button type="button" class="pill-btn primary" style="flex:1;justify-content:center;padding:8px;" data-web-sub="workbench" data-toast="${esc(
                t(`已为 @${state.lastResetHandle} 重置密码并轮换全端登录态（主机守护进程保持在线）`, `Password reset for @${state.lastResetHandle}; old sessions revoked`)
              )}">${esc(t("确认重置密码", "Reset Password"))}</button>
              <button type="button" class="pill-btn" data-web-sub="workbench">${esc(t("返回", "Cancel"))}</button>
            </div>
          </div>
        </div>
      `;
    } else {
      inner = `
        <div class="wb-body">
          <aside class="wb-sidebar">
            <div style="padding:10px 10px 6px;display:flex;align-items:center;justify-content:space-between;">
              <div style="display:flex;align-items:center;gap:6px;font-weight:700;font-size:13px;">
                <span class="dot online"></span> Coda Web <span class="mono" style="font-size:10px;color:var(--green);">v1.1.0</span>
              </div>
              <div style="display:flex;gap:4px;">
                <button type="button" class="icon-btn" title="⌘K" data-open-modal="palette">⌘K</button>
                <button type="button" class="icon-btn" title="${esc(t("新建 Agent/克隆仓库", "Launch Agent / Clone"))}" data-open-modal="launcher">+</button>
                <button type="button" class="icon-btn" title="${esc(t("设置中心", "Settings"))}" data-open-modal="settings">⚙</button>
              </div>
            </div>
            <div class="wb-sidebar-scroll">
              ${renderAttentionSidebarBox()}
              <div class="sec-label"><span>${esc(t("在线主机与健康", "HOSTS & HEALTH"))} (${state.hosts.filter((h) => h.online).length}/${state.hosts.length})</span></div>
              ${state.hosts.map((h) => renderHostRowCompact(h)).join("")}
              <div class="sec-label" style="margin-top:4px;"><span>${esc(t("5 大智能体活跃会话", "ACTIVE SESSIONS"))}</span><span>${state.sessions.length}</span></div>
              ${state.sessions
                .map(
                  (sess) => `
                  <button type="button" class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${esc(sess.id)}">
                    ${avatarHtml(sess, 26)}
                    <div class="sess-meta">
                      <div class="sess-title">${esc(sess.workspace)} · ${esc(sess.agent)}</div>
                      <div class="sess-sub">${esc(sess.title)}</div>
                    </div>
                    ${badgeHtml(sess)}
                  </button>`
                )
                .join("")}
            </div>
            <div class="wb-sidebar-foot">
              <span class="mono" style="font-size:10px;color:${state.webPushEnabled ? "var(--green)" : "var(--muted)"};">
                ${state.webPushEnabled ? `🔔 Push (${Number(state.webPushCount)}) · sw.js` : "🔕 Push OFF"}
              </span>
              <button type="button" class="pill-btn" data-open-settings-tab="friends">🤝 ${esc(t("邀请/重置", "Friends"))}</button>
            </div>
          </aside>

          <section class="term-col">
            <div class="term-head">
              <div style="min-width:0;">
                <div style="font-size:12.5px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(s.title)}</div>
                <div class="mono" style="font-size:10px;color:var(--muted);">${esc(s.hostName)} · ${esc(s.workspace)} (${esc(s.branch)}) · <span style="color:var(--green);">${esc(s.agent)}</span></div>
              </div>
              <div style="display:flex;gap:5px;align-items:center;flex-shrink:0;">
                <button type="button" class="pill-btn ${state.webSplit ? "sky" : ""}" data-toggle-web-split>⌘D ${esc(t("分屏", "Split"))}</button>
                <button type="button" class="pill-btn ${state.termSearchOpen ? "sky" : ""}" data-toggle-term-search>🔍 ⌘F</button>
                <button type="button" class="pill-btn" data-export-md="${esc(s.id)}">⬇ .md</button>
                <button type="button" class="pill-btn danger" data-approve="esc" data-sid="${esc(s.id)}">^C</button>
              </div>
            </div>
            ${renderTerminalSearchBar(s)}
            ${renderOnboardingCard()}
            ${renderNeedsBanner(s)}
            <div style="flex:1;min-height:0;display:flex;gap:1px;background:#1e222b;">
              <div class="term-screen" style="background:#08090c;">${renderTerminalLines(s)}</div>
              ${
                state.webSplit
                  ? `<div class="term-screen" style="background:#08090c;border-left:1px solid #1e222b;">
                      <div class="mono" style="font-size:10px;color:var(--sky);margin-bottom:6px;">Split · ${esc(s2.workspace)} (${esc(s2.agent)})</div>
                      ${renderTerminalLines(s2)}
                    </div>`
                  : ""
              }
            </div>
            ${renderComposerBar(s)}
          </section>

          <aside class="right-drawer">
            ${renderUnifiedInspector(s, false)}
          </aside>
        </div>
      `;
    }

    return `
      <div class="frame-browser">
        <div class="chrome-bar">
          <div class="traffic-lights"><span class="tl red"></span><span class="tl yellow"></span><span class="tl green"></span></div>
          <div class="url-bar">🔒 ${esc(urlPath)}</div>
          <span class="mono" style="font-size:10.5px;color:${route.isRelay ? "var(--sky)" : "var(--green)"};">${esc(route.short)} · sw.js PWA</span>
        </div>
        ${inner}
      </div>
    `;
  }

  // ===================== 2. Web Mobile / Safari PWA =====================
  function renderWebMobile() {
    const s = activeSession();
    const needsCount = needsYouSessions().length;
    return `
      <div class="frame-phone">
        <div class="ios-status">
          <span>9:41</span>
          <div class="dynamic-island"></div>
          <span>5G · 🔔PWA</span>
        </div>
        <div class="term-head" style="padding:7px 9px;">
          <button type="button" class="icon-btn" data-toggle-web-drawer>☰</button>
          <div style="min-width:0;flex:1;text-align:center;">
            <div style="font-size:12px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(s.workspace)} · ${esc(s.agent)}</div>
            <div class="mono" style="font-size:9.5px;color:var(--muted);">v1.1.0 PWA · ${esc(s.branch)}</div>
          </div>
          <div style="display:flex;gap:4px;">
            ${needsCount > 0 ? `<button type="button" class="icon-btn" style="border-color:rgba(245,158,11,0.5);color:var(--amber);" data-jump-needs title="⌘J">▲${needsCount}</button>` : ""}
            <button type="button" class="icon-btn" data-open-modal="inspector" title="Files / Diff / Ports">±</button>
            <button type="button" class="icon-btn ${state.webMobileKeypad ? "active" : ""}" data-toggle-web-keypad title="Keypad">⌨</button>
          </div>
        </div>
        ${renderTerminalSearchBar(s)}
        ${renderNeedsBanner(s, true)}
        <div class="term-screen" style="font-size:11px;padding:10px;">${renderTerminalLines(s)}</div>
        ${state.webMobileKeypad ? renderKeypad(s.id) : ""}
        ${renderComposerBar(s, true)}
        <div class="home-indicator"></div>
        ${
          state.webMobileDrawer
            ? `
          <div class="mobile-drawer-backdrop" data-close-web-drawer>
            <div class="mobile-drawer-panel" data-stop-prop>
              <div style="padding:12px;border-bottom:1px solid var(--border);display:flex;justify-content:space-between;align-items:center;">
                <strong>Coda PWA <span class="mono" style="font-size:10.5px;color:var(--green);">v1.1.0</span></strong>
                <button type="button" class="icon-btn" data-close-web-drawer>✕</button>
              </div>
              <div class="wb-sidebar-scroll">
                ${renderAttentionSidebarBox()}
                ${state.sessions
                  .map(
                    (sess) => `
                  <button type="button" class="sess-item ${sess.id === s.id ? "active" : ""}" data-select-sid="${esc(sess.id)}" data-enter-mobile="1">
                    ${avatarHtml(sess, 26)}
                    <div class="sess-meta">
                      <div class="sess-title">${esc(sess.workspace)} · ${esc(sess.agent)}</div>
                      <div class="sess-sub">${esc(sess.title)}</div>
                    </div>
                    ${badgeHtml(sess)}
                  </button>`
                  )
                  .join("")}
              </div>
              <div class="wb-sidebar-foot">
                <button type="button" class="pill-btn" data-open-modal="launcher">+ ${esc(t("启动/克隆", "Launch"))}</button>
                <button type="button" class="pill-btn amber" data-open-settings-tab="alerts">🔔 Push</button>
                <button type="button" class="pill-btn" data-open-modal="settings">⚙</button>
              </div>
            </div>
          </div>`
            : ""
        }
      </div>
    `;
  }

  // ===================== 3. macOS GPUI Desktop (Coda.app) =====================
  function renderMacGPUI() {
    const s = activeSession();
    const s2 = secondarySession();
    const route = ROUTES[state.routeIdx];

    let rightDrawerHtml = "";
    if (state.macRightDrawer === "mesh") {
      rightDrawerHtml = `
        <aside class="right-drawer">
          <div class="term-head">
            <strong style="font-size:12px;">${esc(t("云网、被控与主机 OTA", "Mesh, Host Mode & OTA"))}</strong>
            <button type="button" class="icon-btn" data-mac-drawer="none">✕</button>
          </div>
          <div class="right-drawer-body">
            <div class="host-card">
              <div>
                <div class="host-card-title">${esc(t("允许远端控制本机", "Allow Remote Control"))}</div>
                <div class="host-card-sub">${esc(
                  state.allowRemoteControl
                    ? t("已开启 · 本机可被手机/公司 Mac 遥控", "ON · Controllable from phone/other Mac")
                    : t("已关闭 · 仅作为纯控制端运行", "OFF · Client-only mode")
                )}</div>
              </div>
              <button type="button" class="pill-btn ${state.allowRemoteControl ? "green" : ""}" data-toggle-host-mode>
                ${esc(state.allowRemoteControl ? t("已开启", "ON") : t("已关闭", "OFF"))}
              </button>
            </div>
            <div class="sec-label" style="margin-top:4px;"><span>${esc(t("已配对主机与健康遥测", "PAIRED HOSTS & OTA"))}</span><span>${state.hosts.length}</span></div>
            ${state.hosts
              .map(
                (h) => `
                <div class="host-card" style="flex-direction:column;align-items:stretch;gap:6px;">
                  <div style="display:flex;align-items:center;justify-content:space-between;">
                    <span style="display:flex;align-items:center;gap:6px;font-weight:600;"><span class="dot ${h.online ? "online" : "offline"}"></span>${esc(h.name)}</span>
                    <span class="mono" style="font-size:10px;color:var(--muted);">${esc(h.role)}</span>
                  </div>
                  <div class="health-row">
                    <span class="health-pill">CPU ${esc(h.cpu)}</span>
                    <span class="health-pill">RAM ${esc(h.ram)}</span>
                    ${
                      h.needsOta
                        ? `<button type="button" class="ota-pill" data-upgrade-host="${esc(h.id)}">⚡ OTA v1.1.0</button>`
                        : `<span class="health-pill" style="color:var(--green);">${esc(h.daemonVer)}</span>`
                    }
                  </div>
                </div>`
              )
              .join("")}
            <div style="font-size:10.5px;color:var(--dim);padding:4px;">
              🔐 ${esc(t("Token 存入 macOS Keychain · 磁盘零明文", "Session token cached in macOS Keychain"))}
            </div>
          </div>
        </aside>
      `;
    } else if (state.macRightDrawer === "changes" || state.macRightDrawer === "files" || state.macRightDrawer === "ports") {
      rightDrawerHtml = `<aside class="right-drawer">${renderUnifiedInspector(s, true)}</aside>`;
    }

    return `
      <div class="frame-mac">
        <div class="chrome-bar">
          <div style="display:flex;align-items:center;gap:10px;">
            <div class="traffic-lights"><span class="tl red"></span><span class="tl yellow"></span><span class="tl green"></span></div>
            <span style="font-size:12px;font-weight:700;">Coda.app</span>
            <span class="mono" style="font-size:10px;color:var(--muted);">Metal 120Hz · ${esc(state.allowRemoteControl ? "hybrid" : "client")}</span>
            <button type="button" class="update-pill ${state.macAppUpdated ? "done" : ""}" data-self-update>
              ${esc(state.macAppUpdated ? t("✓ v1.1.0 最新", "✓ v1.1.0 Latest") : t("✨ 升级 v1.1.0", "✨ Update v1.1.0"))}
            </button>
          </div>
          <div style="display:flex;align-items:center;gap:5px;">
            <button type="button" class="pill-btn ${!state.macSplit ? "primary" : ""}" data-mac-split="0">${esc(t("单窗", "Single"))}</button>
            <button type="button" class="pill-btn ${state.macSplit ? "primary" : ""}" data-mac-split="1">${esc(t("分屏 ⌘D", "Split ⌘D"))}</button>
            <button type="button" class="pill-btn" data-jump-needs>▲ ⌘J</button>
            <button type="button" class="pill-btn ${state.termSearchOpen ? "sky" : ""}" data-toggle-term-search>🔍 ⌘F</button>
            <button type="button" class="pill-btn" data-open-modal="palette">⌘K</button>
          </div>
          <div style="display:flex;align-items:center;gap:4px;">
            <button type="button" class="pill-btn ${state.macRightDrawer === "changes" ? "sky" : ""}" data-mac-insp="changes">Changes</button>
            <button type="button" class="pill-btn ${state.macRightDrawer === "files" ? "sky" : ""}" data-mac-insp="files">Files</button>
            <button type="button" class="pill-btn ${state.macRightDrawer === "ports" ? "sky" : ""}" data-mac-insp="ports">Ports</button>
            <button type="button" class="pill-btn ${state.macRightDrawer === "mesh" ? "sky" : ""}" data-mac-drawer="mesh">Mesh</button>
            <button type="button" class="icon-btn" data-open-modal="settings" title="Settings ⌘,">⚙</button>
          </div>
        </div>
        <div class="wb-body">
          <aside class="wb-sidebar" style="width:246px;">
            <div class="wb-sidebar-scroll">
              ${renderAttentionSidebarBox()}
              <div class="sec-label">
                <span>${esc(t("工作区 & 5 大 Agent", "SPACES & 5 AGENTS"))}</span>
                <button type="button" class="icon-btn" style="width:22px;height:22px;" data-open-modal="launcher" title="New Agent / Git Clone">+</button>
              </div>
              ${state.sessions
                .map(
                  (sess) => `
                  <button type="button" class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${esc(sess.id)}">
                    ${avatarHtml(sess, 24)}
                    <div class="sess-meta">
                      <div class="sess-title">${esc(sess.workspace)} · ${esc(sess.agent)}</div>
                      <div class="sess-sub">${esc(sess.hostName)}</div>
                    </div>
                    ${badgeHtml(sess)}
                  </button>`
                )
                .join("")}
              <div class="sec-label" style="margin-top:4px;"><span>${esc(t("主机健康与 OTA", "HOST HEALTH & OTA"))}</span></div>
              ${state.hosts.filter((h) => h.online).map((h) => renderHostRowCompact(h)).join("")}
            </div>
            <div class="wb-sidebar-foot">
              <span class="mono" style="font-size:10px;color:${route.isRelay ? "var(--sky)" : "var(--green)"};">${esc(route.short)}</span>
              <button type="button" class="pill-btn ${state.allowRemoteControl ? "green" : ""}" style="font-size:10.5px;padding:3px 7px;" data-toggle-host-mode>
                ${esc(state.allowRemoteControl ? t("被控: 开", "Host: ON") : t("被控: 关", "Host: OFF"))}
              </button>
            </div>
          </aside>

          <section class="term-col">
            ${renderTerminalSearchBar(s)}
            ${renderOnboardingCard()}
            <div style="flex:1;min-height:0;display:flex;gap:1px;background:#1e222b;">
              <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#0a0b0e;">
                <div class="term-head">
                  <span style="font-size:12px;font-weight:600;">${esc(s.workspace)} · ${esc(s.title)}</span>
                  <div style="display:flex;align-items:center;gap:6px;">
                    <span class="mono" style="font-size:10px;color:var(--sky);">127.0.0.1:54173 Tunnel</span>
                    <button type="button" class="pill-btn" style="padding:2px 7px;font-size:10.5px;" data-export-md="${esc(s.id)}">⬇ .md</button>
                  </div>
                </div>
                ${renderNeedsBanner(s)}
                <div class="term-screen">${renderTerminalLines(s)}</div>
              </div>
              ${
                state.macSplit
                  ? `
                <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#0a0b0e;">
                  <div class="term-head">
                    <span style="font-size:12px;font-weight:600;">${esc(s2.workspace)} · ${esc(s2.agent)}</span>
                    ${badgeHtml(s2)}
                  </div>
                  <div class="term-screen">${renderTerminalLines(s2)}</div>
                </div>`
                  : ""
              }
            </div>
            ${renderComposerBar(s)}
          </section>

          ${rightDrawerHtml}
        </div>
      </div>
    `;
  }

  // ===================== 4. iPadOS Split Workbench =====================
  function renderIPad() {
    const s = activeSession();
    return `
      <div class="frame-ipad">
        <div class="ios-status" style="background:#101217;border-bottom:1px solid #1e222b;">
          <span>9:41 · Coda v1.1.0</span>
          <span class="mono" style="font-size:11px;color:var(--muted);">iPadOS Split View · GhosttyReflow</span>
          <span>Wi-Fi · 96%</span>
        </div>
        <div class="wb-body">
          ${
            state.ipadSidebar
              ? `
            <aside class="wb-sidebar" style="width:276px;background:#13151a;">
              <div style="padding:10px 12px;display:flex;align-items:center;justify-content:space-between;">
                <span style="font-weight:700;font-size:15px;"><span class="dot online" style="margin-right:6px;"></span>Coda <span class="mono" style="font-size:10.5px;color:var(--green);">v1.1.0</span></span>
                <div style="display:flex;gap:5px;align-items:center;">
                  ${attentionCount() > 0 ? `<button type="button" class="red-badge" style="border:none;background:var(--amber);color:#000;" data-jump-needs>▲ ${attentionCount()}</button>` : ""}
                  <button type="button" class="icon-btn" data-open-modal="launcher">+</button>
                </div>
              </div>
              <div class="wb-sidebar-scroll">
                ${renderAttentionSidebarBox()}
                <div class="sec-label"><span>${esc(t("主机健康与多分支", "HOSTS & WORKTREES"))}</span></div>
                ${state.hosts.filter((h) => h.online).map((h) => renderHostRowCompact(h)).join("")}
                <div class="sec-label" style="margin-top:4px;"><span>${esc(t("5 大 Agent 优先级队列", "INBOX PRIORITY"))}</span></div>
                ${state.sessions
                  .map(
                    (sess) => `
                  <button type="button" class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${esc(sess.id)}">
                    ${avatarHtml(sess, 26)}
                    <div class="sess-meta">
                      <div class="sess-title">${esc(sess.title)}</div>
                      <div class="sess-sub">${esc(sess.agent)} · ${esc(sess.branch)}</div>
                    </div>
                    ${badgeHtml(sess)}
                  </button>`
                  )
                  .join("")}
              </div>
              <div class="wb-sidebar-foot">
                <button type="button" class="pill-btn" data-open-modal="settings">⚙ ${esc(t("设置/推送/好友", "Settings"))}</button>
                <button type="button" class="pill-btn sky" data-open-modal="launcher">+ ${esc(t("新建", "Launch"))}</button>
              </div>
            </aside>`
              : ""
          }

          <section class="term-col">
            <div class="term-head">
              <div style="display:flex;align-items:center;gap:8px;min-width:0;">
                <button type="button" class="icon-btn" data-toggle-ipad-sidebar>◧</button>
                <div style="min-width:0;">
                  <div style="font-size:13px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(s.title)}</div>
                  <div class="mono" style="font-size:10px;color:var(--muted);">${esc(s.workspace)} · ${esc(s.agent)} · ${esc(s.hostName)}</div>
                </div>
              </div>
              <div style="display:flex;gap:5px;flex-shrink:0;">
                <button type="button" class="pill-btn ${state.termSearchOpen ? "sky" : ""}" data-toggle-term-search>🔍 ⌘F</button>
                <button type="button" class="pill-btn" data-export-md="${esc(s.id)}">⬇ .md</button>
                <button type="button" class="pill-btn" data-open-modal="inspector">± ${esc(t("检视器 (Files/Diff/Port)", "Inspector"))}</button>
                <button type="button" class="pill-btn sky" data-open-modal="compose">✎ ${esc(t("Composer / 🎙语音", "Composer"))}</button>
              </div>
            </div>
            ${renderTerminalSearchBar(s)}
            ${renderOnboardingCard()}
            ${renderNeedsBanner(s)}
            <div class="term-screen">${renderTerminalLines(s)}</div>
            <div class="composer-stack" style="padding:6px 12px 10px;">
              ${renderSnippetStrip(true)}
              <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;flex-wrap:wrap;">
                <div style="background:rgba(26,29,36,0.96);border:1px solid var(--border);border-radius:999px;padding:3px 7px;display:flex;gap:4px;">
                  <button type="button" class="pill-btn" data-key="esc" data-sid="${esc(s.id)}">esc</button>
                  <button type="button" class="pill-btn" data-key="tab" data-sid="${esc(s.id)}">tab</button>
                  <button type="button" class="pill-btn" data-key="⇧tab" data-sid="${esc(s.id)}">⇧tab</button>
                  <button type="button" class="pill-btn" data-paste-shot>🖼 PhotosPicker</button>
                  <button type="button" class="pill-btn danger" data-key="^C" data-sid="${esc(s.id)}">^C</button>
                </div>
                <button type="button" class="mic-btn ${state.voiceRecording ? "recording" : ""}" style="border-radius:999px;" data-open-modal="compose">🎙 ${esc(t("SFSpeech / 多行撰写", "Voice & Snippets"))}</button>
              </div>
            </div>
          </section>
        </div>
        <div class="home-indicator"></div>
      </div>
    `;
  }

  // ===================== 5. iPhone Native SwiftUI =====================
  function renderIPhone() {
    const s = activeSession();
    const att = attentionCount();

    let screen = "";
    if (state.iphoneScreen === "terminal") {
      screen = `
        <div class="term-head" style="padding:7px 10px;">
          <button type="button" class="pill-btn" data-iphone-back>‹ ${esc(t("收件箱", "Inbox"))}${att > 0 ? ` (${att})` : ""}</button>
          <div style="min-width:0;flex:1;text-align:center;">
            <div style="font-size:12px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(s.workspace)} · ${esc(s.agent)}</div>
            <div class="mono" style="font-size:9.5px;color:var(--muted);">${esc(s.branch)} · ${esc(ROUTES[state.routeIdx].short)}</div>
          </div>
          <div style="display:flex;gap:4px;">
            <button type="button" class="icon-btn ${state.termSearchOpen ? "active" : ""}" data-toggle-term-search title="Search">🔍</button>
            <button type="button" class="icon-btn" data-open-modal="inspector" title="Files / Diff / Port">±</button>
          </div>
        </div>
        ${renderTerminalSearchBar(s)}
        ${renderNeedsBanner(s, true)}
        <div class="term-screen" style="font-size:11px;padding:10px;">${renderTerminalLines(s)}</div>
        ${state.iphoneKeypad ? renderKeypad(s.id) : ""}
        <div style="padding:4px 10px 0;">${renderSnippetStrip(true)}</div>
        <div style="padding:6px 10px 8px;display:flex;align-items:center;gap:6px;">
          <div style="flex:1;background:#1a1d24;border:1px solid var(--border);border-radius:999px;padding:3px 6px;display:flex;gap:3px;overflow-x:auto;">
            <button type="button" class="pill-btn" style="padding:3px 7px;font-size:10.5px;" data-key="esc" data-sid="${esc(s.id)}">esc</button>
            <button type="button" class="pill-btn" style="padding:3px 7px;font-size:10.5px;" data-key="tab" data-sid="${esc(s.id)}">tab</button>
            <button type="button" class="pill-btn" style="padding:3px 7px;font-size:10.5px;" data-paste-shot>🖼</button>
            <button type="button" class="pill-btn ${state.iphoneKeypad ? "sky" : ""}" style="padding:3px 7px;font-size:10.5px;" data-toggle-iphone-keypad>⌨</button>
            <button type="button" class="pill-btn danger" style="padding:3px 7px;font-size:10.5px;" data-key="^C" data-sid="${esc(s.id)}">^C</button>
          </div>
          <button type="button" class="icon-btn" style="border-radius:50%;width:34px;height:34px;" data-open-modal="compose" title="Compose, Snippets & Voice">✎</button>
        </div>
      `;
    } else if (state.iphoneTab === "inbox") {
      screen = `
        <div style="padding:8px 16px 6px;display:flex;align-items:center;justify-content:space-between;">
          <h2 style="font-size:22px;font-weight:700;">${esc(t("收件箱", "Inbox"))} <span class="mono" style="font-size:11px;color:var(--green);">v1.1</span></h2>
          <div style="display:flex;gap:5px;align-items:center;">
            <button type="button" class="pill-btn amber" style="font-size:10px;padding:3px 7px;" data-jump-needs>▲ ⌘J</button>
            <button type="button" class="icon-btn" style="border-radius:50%;" data-open-modal="launcher">+</button>
            <button type="button" class="icon-btn" style="border-radius:50%;" data-open-modal="settings">⚙</button>
          </div>
        </div>
        ${renderOnboardingCard()}
        <div style="flex:1;overflow-y:auto;padding:8px 14px 72px;display:flex;flex-direction:column;gap:9px;">
          ${state.sessions
            .map(
              (sess) => `
            <button type="button" class="sess-item ${sess.state === "needs_you" ? "needs" : ""}" style="background:#13151a;border-color:${sess.state === "needs_you" ? "rgba(245,158,11,0.45)" : "var(--border)"};padding:10px;" data-select-sid="${esc(sess.id)}" data-enter-mobile="1">
              ${avatarHtml(sess, 34)}
              <div class="sess-meta">
                <div class="sess-title">${esc(sess.workspace)} · ${esc(sess.agent)}</div>
                <div class="sess-sub">${esc(sess.title)}</div>
                ${sess.state === "needs_you" ? `<div style="font-size:10px;color:var(--amber);margin-top:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(sess.statusMsg)}</div>` : ""}
              </div>
              ${badgeHtml(sess)}
            </button>`
            )
            .join("")}
        </div>
        <div class="mobile-bottom-pill">
          <button type="button" class="mobile-tab-btn active" data-iphone-tab="inbox">${esc(t("收件箱", "Inbox"))}${att > 0 ? `<span class="red-badge">${att}</span>` : ""}</button>
          <button type="button" class="mobile-tab-btn" data-iphone-tab="spaces">${esc(t("空间与主机", "Spaces"))}</button>
        </div>
      `;
    } else {
      screen = `
        <div style="padding:8px 16px 6px;display:flex;align-items:center;justify-content:space-between;">
          <h2 style="font-size:22px;font-weight:700;">${esc(t("空间与主机", "Spaces"))}</h2>
          <button type="button" class="icon-btn" style="border-radius:50%;" data-open-modal="launcher">+</button>
        </div>
        <div style="flex:1;overflow-y:auto;padding:8px 14px 72px;display:flex;flex-direction:column;gap:11px;">
          ${state.hosts
            .map(
              (h) => `
            <div style="background:#13151a;border:1px solid var(--border);border-radius:14px;padding:12px;display:flex;flex-direction:column;gap:8px;">
              <div style="display:flex;justify-content:space-between;align-items:center;">
                <span style="font-size:13px;font-weight:600;"><span class="dot ${h.online ? "online" : "offline"}" style="margin-right:6px;"></span>${esc(h.name)}</span>
                ${
                  h.needsOta
                    ? `<button type="button" class="ota-pill" data-upgrade-host="${esc(h.id)}">⚡ OTA v1.1.0</button>`
                    : `<span class="mono" style="font-size:10px;color:var(--green);">${esc(h.daemonVer)}</span>`
                }
              </div>
              <div class="health-row">
                <span class="health-pill">CPU ${esc(h.cpu)}</span>
                <span class="health-pill">RAM ${esc(h.ram)}</span>
                <span class="health-pill">${esc(h.platform)}</span>
              </div>
              <div style="display:grid;grid-template-columns:1fr 1fr;gap:6px;">
                ${state.sessions
                  .filter((x) => x.hostId === h.id)
                  .map(
                    (sess) => `
                  <button type="button" class="sess-item" style="flex-direction:column;align-items:flex-start;gap:4px;background:#0b0c0f;border-color:var(--border);" data-select-sid="${esc(sess.id)}" data-enter-mobile="1">
                    <div style="display:flex;width:100%;justify-content:space-between;align-items:center;gap:4px;">
                      <span class="mono" style="font-size:10px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${esc(sess.agent)}</span>
                      ${badgeHtml(sess)}
                    </div>
                    <div class="sess-sub" style="width:100%;">${esc(sess.workspace)}</div>
                  </button>`
                  )
                  .join("")}
              </div>
            </div>`
            )
            .join("")}
        </div>
        <div class="mobile-bottom-pill">
          <button type="button" class="mobile-tab-btn" data-iphone-tab="inbox">${esc(t("收件箱", "Inbox"))}${att > 0 ? `<span class="red-badge">${att}</span>` : ""}</button>
          <button type="button" class="mobile-tab-btn active" data-iphone-tab="spaces">${esc(t("空间与主机", "Spaces"))}</button>
        </div>
      `;
    }

    return `
      <div class="frame-phone">
        <div class="ios-status">
          <span>9:41</span>
          <div class="dynamic-island"></div>
          <span>5G 100%</span>
        </div>
        <div style="flex:1;min-height:0;display:flex;flex-direction:column;position:relative;">
          ${screen}
        </div>
        <div class="home-indicator"></div>
      </div>
    `;
  }

  // ===================== 6. iPhone Duo Foldable (3 Postures) =====================
  function renderIPhoneDuo() {
    const s = activeSession();
    const s2 = secondarySession();

    if (state.duoPosture === "cover") {
      return `
        <div class="frame-duo-cover">
          <div style="flex:1;min-height:0;display:flex;">
            <div style="flex:1;min-width:0;display:flex;flex-direction:column;">
              <div class="term-head">
                <div>
                  <div style="font-size:12px;font-weight:600;">${esc(s.workspace)} · ${esc(s.agent)}</div>
                  <div class="mono" style="font-size:9.5px;color:var(--muted);">5.4" Cover (466×678) · 58×28</div>
                </div>
                <div style="display:flex;gap:4px;">
                  <button type="button" class="icon-btn" data-open-modal="inspector">±</button>
                  <button type="button" class="icon-btn" data-open-modal="compose">✎</button>
                </div>
              </div>
              ${renderNeedsBanner(s, true)}
              <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
              <div style="padding:0 10px 4px;">${renderSnippetStrip(true)}</div>
              ${renderKeypad(s.id)}
            </div>
            <div class="duo-rail">
              <div style="font-size:11.5px;font-weight:700;"><span class="dot online" style="margin-right:4px;"></span>Duo</div>
              <span class="mono" style="font-size:9px;color:var(--green);">v1.1.0</span>
              <button type="button" class="duo-rail-btn active" data-duo-posture="cover">${esc(t("外屏", "Cover"))}</button>
              <button type="button" class="duo-rail-btn" data-duo-posture="inner_landscape">${esc(t("展开", "Unfold"))}</button>
              <button type="button" class="duo-rail-btn" data-duo-posture="tabletop">${esc(t("笔电", "Laptop"))}</button>
              <button type="button" class="duo-rail-btn" data-jump-needs>▲<span>⌘J</span></button>
              <div style="margin-top:auto;display:flex;flex-direction:column;gap:6px;width:100%;align-items:center;">
                <button type="button" class="icon-btn" data-open-modal="settings">⚙</button>
              </div>
            </div>
          </div>
        </div>
      `;
    }

    if (state.duoPosture === "tabletop") {
      return `
        <div class="frame-duo-tabletop">
          <div style="flex:1;min-height:0;display:flex;">
            <div style="flex:1;min-width:0;display:flex;flex-direction:column;">
              <div class="term-head">
                <span style="font-size:12.5px;font-weight:700;">Coda Duo · ${esc(t("笔电半折态", "Tabletop Laptop Pose"))}</span>
                <div style="display:flex;gap:5px;align-items:center;">
                  <span class="mono" style="font-size:10px;color:var(--green);">626×890</span>
                  <button type="button" class="pill-btn" style="padding:2px 6px;font-size:10px;" data-open-modal="inspector">± Diff/Port</button>
                </div>
              </div>
              ${renderTerminalSearchBar(s)}
              <div style="flex:1;min-height:0;display:flex;flex-direction:column;background:#08090c;">
                <div class="term-screen" style="font-size:11.5px;">${renderTerminalLines(s)}</div>
              </div>
              <div class="fold-crease-h">─── FOLD CREASE · 24PT .division ───</div>
              <div style="flex:1;min-height:0;padding:8px 11px;display:flex;flex-direction:column;gap:6px;background:#0b0c0f;overflow-y:auto;">
                <div style="display:flex;gap:5px;overflow-x:auto;">
                  ${state.sessions
                    .map(
                      (sess) => `
                    <button type="button" class="pill-btn ${sess.id === s.id ? "sky" : ""}" style="white-space:nowrap;font-size:10px;" data-select-sid="${esc(sess.id)}">${esc(sess.workspace)}/${esc(sess.agent)}</button>`
                    )
                    .join("")}
                </div>
                ${renderNeedsBanner(s, true)}
                ${renderSnippetStrip(true)}
                <form style="display:flex;gap:6px;align-items:center;" data-cmd-form="${esc(s.id)}">
                  <input class="term-input" placeholder="${esc(t("向半折上半屏发命令、粘贴图片或语音听写...", "Send command, snippet, photo or voice...") )}" value="${esc(state.voiceDraft)}" />
                  <button type="button" class="pill-btn" data-paste-shot>🖼</button>
                  <button type="button" class="mic-btn ${state.voiceRecording ? "recording" : ""}" data-toggle-voice>🎙</button>
                  <button type="submit" class="pill-btn sky">↑</button>
                </form>
                ${renderKeypad(s.id)}
              </div>
            </div>
            <div class="duo-rail">
              <div style="font-size:11.5px;font-weight:700;">Duo</div>
              <button type="button" class="duo-rail-btn" data-duo-posture="cover">${esc(t("外屏", "Cover"))}</button>
              <button type="button" class="duo-rail-btn" data-duo-posture="inner_landscape">${esc(t("双栏", "Split"))}</button>
              <button type="button" class="duo-rail-btn active" data-duo-posture="tabletop">${esc(t("笔电", "Laptop"))}</button>
              <div style="margin-top:auto;"><button type="button" class="icon-btn" data-open-modal="settings">⚙</button></div>
            </div>
          </div>
        </div>
      `;
    }

    // Inner Unfolded Landscape (890 x 626: Left Leaf + 24pt Fold Crease + Right Leaf + 84pt System Rail)
    let leftLeaf = "";
    let rightLeaf = "";
    if (state.duoSplitMode === "split") {
      leftLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#13151a;">
          <div style="padding:8px 10px;display:flex;justify-content:space-between;align-items:center;border-bottom:1px solid var(--border);">
            <strong style="font-size:12px;">${esc(t("左侧智能体目录 Leaf 1", "Agent Directory Leaf 1"))}</strong>
            <button type="button" class="pill-btn sky" style="font-size:10.5px;padding:3px 8px;" data-open-modal="launcher">+ ${esc(t("启动/克隆", "Launch"))}</button>
          </div>
          <div class="wb-sidebar-scroll">
            ${renderAttentionSidebarBox()}
            ${state.sessions
              .map(
                (sess) => `
              <button type="button" class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${esc(sess.id)}">
                ${avatarHtml(sess, 26)}
                <div class="sess-meta">
                  <div class="sess-title">${esc(sess.workspace)} · ${esc(sess.agent)}</div>
                  <div class="sess-sub">${esc(sess.title)}</div>
                </div>
                ${badgeHtml(sess)}
              </button>`
              )
              .join("")}
          </div>
        </div>
      `;
      rightLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#08090c;">
          <div class="term-head">
            <span style="font-size:12px;font-weight:600;">${esc(s.workspace)} · ${esc(s.agent)}</span>
            <div style="display:flex;gap:4px;">
              <button type="button" class="pill-btn ${state.termSearchOpen ? "sky" : ""}" style="padding:2px 6px;font-size:10px;" data-toggle-term-search>🔍</button>
              <button type="button" class="pill-btn" style="padding:2px 7px;font-size:10px;" data-open-modal="compose">✎ / 🎙</button>
            </div>
          </div>
          ${renderTerminalSearchBar(s)}
          ${renderNeedsBanner(s, true)}
          <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
          <div style="padding:4px 8px;border-top:1px solid #1e222b;">${renderSnippetStrip(true)}</div>
        </div>
      `;
    } else if (state.duoSplitMode === "diff") {
      leftLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#08090c;">
          <div class="term-head"><strong style="font-size:12px;">L1 · ${esc(s.workspace)} (${esc(s.agent)})</strong></div>
          <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
        </div>
      `;
      rightLeaf = `<div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#0a0b0e;">${renderUnifiedInspector(s, false)}</div>`;
    } else {
      leftLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#08090c;">
          <div class="term-head"><strong style="font-size:12px;">L1 · ${esc(s.workspace)} (${esc(s.agent)})</strong>${badgeHtml(s)}</div>
          <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
        </div>
      `;
      rightLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#08090c;">
          <div class="term-head"><strong style="font-size:12px;">L2 · ${esc(s2.workspace)} (${esc(s2.agent)})</strong>${badgeHtml(s2)}</div>
          <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s2)}</div>
        </div>
      `;
    }

    return `
      <div class="frame-duo-inner">
        <div style="flex:1;min-height:0;display:flex;">
          <div style="flex:1;min-width:0;display:flex;flex-direction:column;">
            <div class="term-head" style="height:36px;">
              <div style="display:flex;align-items:center;gap:8px;">
                <span class="dot online"></span>
                <strong style="font-size:12.5px;">Coda Duo 7.6" <span class="mono" style="font-size:10px;color:var(--green);">v1.1.0</span></strong>
                <span class="mono" style="font-size:10px;color:var(--muted);">433 + 24 Crease + 349 + 84 Rail</span>
              </div>
              <div style="display:flex;gap:4px;">
                ${[
                  ["split", t("分栏", "Split")],
                  ["diff", t("三合一检视", "Inspector")],
                  ["dual", t("双窗", "Dual")],
                  ["canvas", t("全宽", "Canvas")],
                ]
                  .map(
                    ([m, label]) =>
                      `<button type="button" class="pill-btn ${state.duoSplitMode === m ? "primary" : ""}" style="padding:3px 8px;font-size:10.5px;" data-duo-mode="${esc(m)}">${esc(label)}</button>`
                  )
                  .join("")}
              </div>
            </div>
            <div style="flex:1;min-height:0;display:flex;">
              ${
                state.duoSplitMode === "canvas"
                  ? `
                <div style="flex:1;display:flex;flex-direction:column;background:#08090c;">
                  ${renderNeedsBanner(s)}
                  <div class="term-screen" style="font-size:11.5px;">${renderTerminalLines(s)}</div>
                </div>`
                  : `${leftLeaf}<div class="fold-crease-v"><span>24PT CREASE</span></div>${rightLeaf}`
              }
            </div>
          </div>
          <div class="duo-rail">
            <div style="font-size:11.5px;font-weight:700;"><span class="dot online" style="margin-right:4px;"></span>Duo</div>
            <span class="mono" style="font-size:9px;color:var(--green);">84pt Rail</span>
            <button type="button" class="duo-rail-btn ${state.duoSplitMode === "split" ? "active" : ""}" data-duo-mode="split">◧<span>${esc(t("分栏", "Split"))}</span></button>
            <button type="button" class="duo-rail-btn ${state.duoSplitMode === "diff" ? "active" : ""}" data-duo-mode="diff">±<span>${esc(t("检视", "Inspect"))}</span></button>
            <button type="button" class="duo-rail-btn ${state.duoSplitMode === "dual" ? "active" : ""}" data-duo-mode="dual">▯▯<span>${esc(t("双窗", "Dual"))}</span></button>
            <button type="button" class="duo-rail-btn ${state.duoSplitMode === "canvas" ? "active" : ""}" data-duo-mode="canvas">⛶<span>${esc(t("全宽", "Canvas"))}</span></button>
            <div style="margin-top:auto;"><button type="button" class="icon-btn" data-open-modal="settings">⚙</button></div>
          </div>
        </div>
      </div>
    `;
  }

  // ===================== Modals =====================
  let currentModalKind = null;
  function openModal(kind) {
    const resolvedKind = kind === "diff" ? "inspector" : kind;
    currentModalKind = resolvedKind;
    const backdrop = document.getElementById("proto-modal-backdrop");
    const modal = document.getElementById("proto-modal");
    const s = activeSession();

    if (resolvedKind === "launcher") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>🚀 ${esc(t("启动远端 AI Agent / 工作区引导 (v1.1.0)", "Launch Remote AI Agent & Workspace Bootstrapper"))}</span>
          <button type="button" class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="display:flex;gap:6px;flex-wrap:wrap;align-items:center;">
          <span style="font-size:11.5px;color:var(--muted);">${esc(t("目标主机：", "Target Host:"))}</span>
          ${state.hosts
            .filter((h) => h.online)
            .map(
              (h) =>
                `<button type="button" class="pill-btn ${state.launcherHostId === h.id ? "sky" : ""}" data-launcher-host="${esc(h.id)}">● ${esc(h.name)}</button>`
            )
            .join("")}
        </div>
        <div style="background:#0d0f14;border:1px solid var(--border);border-radius:10px;padding:10px;display:flex;flex-direction:column;gap:8px;">
          <div style="display:flex;gap:6px;">
            <button type="button" class="pill-btn ${state.launcherBootMode === "folder" ? "primary" : ""}" data-boot-mode="folder">📁 ${esc(t("选择 / 新建远端目录", "Remote Folder Picker"))}</button>
            <button type="button" class="pill-btn ${state.launcherBootMode === "clone" ? "primary" : ""}" data-boot-mode="clone">⎇ ${esc(t("Git Clone 仓库引导", "Git Clone Bootstrapper"))}</button>
          </div>
          ${
            state.launcherBootMode === "folder"
              ? `
            <div style="display:flex;gap:5px;flex-wrap:wrap;">
              ${["~/workspace/coda", "~/workspace/coda-web", "~/workspace/coda-apple", "~/workspace/new-service"]
                .map(
                  (dir) =>
                    `<button type="button" class="snippet-chip" data-pick-folder="${esc(dir)}">${esc(dir)}</button>`
                )
                .join("")}
            </div>
            <input id="launcher-folder-input" class="term-input" value="${esc(state.launcherFolder)}" placeholder="~/workspace/project" />`
              : `
            <input id="launcher-clone-url" class="term-input" value="${esc(state.launcherCloneUrl)}" placeholder="https://github.com/org/repo.git" />
            <input id="launcher-clone-dir" class="term-input" value="${esc(state.launcherCloneDir)}" placeholder="~/workspace/repo-target" />`
          }
        </div>
        <div class="sec-label"><span>${esc(t("选择要启动的 AI Agent（点击即刻创建会话）", "CLICK AN AGENT TO LAUNCH"))}</span><span>6</span></div>
        <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;">
          ${AGENTS.map(
            (ag) => `
            <button type="button" class="host-card" style="flex-direction:column;align-items:flex-start;text-align:left;" data-launch-agent="${esc(ag.id)}">
              <div class="host-card-title" style="color:var(--sky);">${esc(ag.name)}</div>
              <div class="host-card-sub">${esc(state.lang === "zh" ? ag.descZh : ag.descEn)}</div>
            </button>`
          ).join("")}
        </div>
      `;
    } else if (resolvedKind === "compose") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>✎ ${esc(t("多行 Prompt Composer · 云同步 Snippets · 语音与截图", "Multi-Line Prompt Composer · Snippets · Voice & Image"))}</span>
          <button type="button" class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="font-size:11.5px;color:var(--muted);">${esc(s.workspace)} · <strong style="color:var(--sky);">${esc(s.agent)}</strong> · ${esc(s.hostName)} (?2004h Smart Bracketed Paste)</div>
        ${renderSnippetStrip(false)}
        ${
          state.attachedShot
            ? `<div class="attach-chip">🖼 ${esc(state.attachedShot)} <button type="button" style="border:none;background:transparent;color:inherit;font-weight:700;" data-clear-shot>✕</button></div>`
            : ""
        }
        <textarea id="modal-compose-ta" class="term-input" rows="5" placeholder="${esc(
          t(
            "输入多行指令、点选上方云同步快捷指令、粘贴剪贴板截图，或使用免配置原生语音 / E2EE Whisper 听写...",
            "Write a multi-line prompt, tap a synced Snippet, attach a screenshot, or dictate via native speech / E2EE Whisper..."
          )
        )}">${esc(state.voiceDraft)}</textarea>
        <div style="display:flex;align-items:center;gap:8px;justify-content:space-between;flex-wrap:wrap;">
          <div style="display:flex;gap:6px;flex-wrap:wrap;">
            <button type="button" class="pill-btn" data-paste-shot>🖼 ${esc(t("粘贴截图 / 相册", "⌘V Screenshot / Photos"))}</button>
            <button type="button" class="mic-btn ${state.voiceRecording ? "recording" : ""}" data-toggle-voice>
              🎙 ${esc(state.voiceRecording ? t("停止并转写", "Stop & Transcribe") : t("免配置语音 / Whisper", "Zero-Config Voice"))}
            </button>
          </div>
          <button type="button" class="pill-btn primary" data-submit-compose>${esc(t("发送至终端 ↑", "Send to PTY ↑"))}</button>
        </div>
      `;
    } else if (resolvedKind === "inspector") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>± ${esc(s.workspace)} · ${esc(t("三合一加密工作区检视器", "Unified E2EE Workspace Inspector"))}</span>
          <button type="button" class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="max-height:62vh;overflow-y:auto;background:#08090c;border-radius:10px;border:1px solid var(--border);display:flex;flex-direction:column;">
          ${renderUnifiedInspector(s, state.platform === "mac_gpui")}
        </div>
      `;
    } else if (resolvedKind === "palette") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>⌘K ${esc(t("全局命令与快捷指令面板", "Command & Snippet Palette"))}</span>
          <button type="button" class="icon-btn" data-close-modal>✕</button>
        </div>
        <input class="term-input" placeholder="${esc(t("跳转工作区、发送云同步 Snippet、跳至待确认 (⌘J) 或导出 .md...", "Jump to session, run synced Snippet, jump Needs-You (⌘J), or export .md...") )}" />
        <div class="sec-label"><span>${esc(t("快捷操作", "QUICK ACTIONS"))}</span></div>
        <div style="display:flex;gap:6px;flex-wrap:wrap;">
          <button type="button" class="pill-btn amber" data-jump-needs data-close-after="1">▲ ⌘J ${esc(t("跳至下一个待确认", "Jump Next Needs-You"))}</button>
          <button type="button" class="pill-btn" data-toggle-term-search data-close-after="1">🔍 ⌘F ${esc(t("搜索终端回滚", "Search Scrollback"))}</button>
          <button type="button" class="pill-btn" data-export-md="${esc(s.id)}" data-close-after="1">⬇ ${esc(t("导出会话 .md", "Export Session .md"))}</button>
          <button type="button" class="pill-btn sky" data-open-modal="launcher">+ ${esc(t("启动 5 大 Agent", "Launch Agent"))}</button>
        </div>
        <div class="sec-label"><span>${esc(t("云同步快捷指令 (点击直接发送至当前终端)", "SYNCED SNIPPETS"))}</span></div>
        <div style="display:flex;flex-direction:column;gap:5px;">
          ${state.snippets
            .map(
              (sn) => `
            <button type="button" class="host-card" style="text-align:left;" data-run-snippet-now="${esc(sn.id)}">
              <div>
                <div class="host-card-title">${esc(state.lang === "zh" ? sn.titleZh : sn.titleEn)}</div>
                <div class="host-card-sub mono">${esc(sn.prompt)}</div>
              </div>
              <span class="mono" style="font-size:10px;color:var(--sky);">↵ Run</span>
            </button>`
            )
            .join("")}
        </div>
        <div class="sec-label"><span>${esc(t("活跃会话", "ACTIVE SESSIONS"))}</span></div>
        <div style="display:flex;flex-direction:column;gap:5px;">
          ${state.sessions
            .map(
              (sess) => `
            <button type="button" class="sess-item" data-select-sid="${esc(sess.id)}" data-close-after="1">
              ${avatarHtml(sess, 24)}
              <div class="sess-meta"><div class="sess-title">${esc(sess.workspace)} · ${esc(sess.agent)}</div><div class="sess-sub">${esc(sess.title)}</div></div>
              ${badgeHtml(sess)}
            </button>`
            )
            .join("")}
        </div>
      `;
    } else if (resolvedKind === "settings") {
      let tabBody = "";
      if (state.settingsTab === "alerts") {
        tabBody = `
          <div class="host-card" style="flex-direction:column;align-items:stretch;gap:8px;border-color:rgba(56,189,248,0.45);">
            <div style="display:flex;align-items:center;justify-content:space-between;gap:8px;">
              <div>
                <div class="host-card-title">🔔 ${esc(t("一键开启原生锁屏推送 (W3C VAPID Web Push · sw.js)", "1-Click Lock-Screen Web Push (RFC 8292 VAPID + aes128gcm)"))}</div>
                <div class="host-card-sub">${esc(
                  t(
                    "零第三方 App 依赖 · 仅传设备名与目录末级名（零终端明文）· 已订阅设备数：",
                    "Zero 3rd-party app needed · Zero terminal plaintext · Active subscriptions: "
                  )
                )}<strong style="color:var(--green);">${Number(state.webPushCount)}</strong></div>
              </div>
              <button type="button" class="pill-btn ${state.webPushEnabled ? "green" : "primary"}" data-toggle-web-push>
                ${esc(state.webPushEnabled ? t("✓ 已开启锁屏推送", "✓ Subscribed") : t("一键开启推送", "Enable Push"))}
              </button>
            </div>
            <div style="font-size:10.5px;color:var(--amber);background:rgba(245,158,11,0.08);padding:5px 8px;border-radius:6px;">
              💡 ${esc(t("iPhone 提示：在 Safari 点击「分享 → 添加到主屏幕」即可激活 iOS 原生锁屏推送通知。", "iPhone Tip: Tap Share → Add to Home Screen in Safari to enable iOS lock-screen push."))}
            </div>
            <div style="display:flex;gap:6px;">
              <button type="button" class="pill-btn sky" data-toast="${esc(
                t("🔔 已并发投递测试通知：web_push (aes128gcm 201 Created) + Bark + Telegram", "🔔 Dispatched test push: web_push (201) + Bark + Telegram")
              )}">${esc(t("发送测试锁屏通知", "Send Test Push"))}</button>
            </div>
          </div>
          <div class="sec-label"><span>${esc(t("多通道离线 Webhook（DNS 钉扎防 SSRF）", "SSRF-SAFE DNS-PINNED WEBHOOKS"))}</span></div>
          <div style="display:grid;grid-template-columns:1fr 1fr;gap:7px;">
            ${[
              ["bark", "Bark (iOS APNs)", "https://api.day.app/•••"],
              ["telegram", "Telegram Bot", "chat_id: 6819•••"],
              ["feishu", "Feishu / 飞书群机器人", "open.feishu.cn/open-apis/bot/v2/•••"],
              ["discord", "Discord / Generic Webhook", "discord.com/api/webhooks/•••"],
            ]
              .map(
                ([k, name, sub]) => `
              <div class="host-card">
                <div style="min-width:0;">
                  <div class="host-card-title">${esc(name)}</div>
                  <div class="host-card-sub mono" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${esc(sub)}</div>
                </div>
                <button type="button" class="pill-btn ${state.webhooks[k] ? "green" : ""}" data-toggle-webhook="${esc(k)}">${state.webhooks[k] ? "ON" : "OFF"}</button>
              </div>`
              )
              .join("")}
          </div>
        `;
      } else if (state.settingsTab === "friends") {
        tabBody = `
          <div class="host-card" style="flex-direction:column;align-items:stretch;gap:8px;">
            <div style="display:flex;justify-content:space-between;align-items:center;">
              <div>
                <div class="host-card-title">🤝 ${esc(t("好友邀请注册深链 (/register?invite=)", "Friend Invite Deep-Link (/register?invite=)"))}</div>
                <div class="host-card-sub mono">https://codahub.dev/register?invite=${esc(state.inviteCode)}</div>
              </div>
              <button type="button" class="pill-btn" data-new-invite>+ ${esc(t("生成新码", "New Code"))}</button>
            </div>
            <div style="display:flex;gap:6px;flex-wrap:wrap;">
              <button type="button" class="pill-btn sky" data-toast="${esc(
                t(`🔗 已复制邀请深链 https://codahub.dev/register?invite=${state.inviteCode}，可直接发微信好友`, `🔗 Copied https://codahub.dev/register?invite=${state.inviteCode}`)
              )}">🔗 ${esc(t("一键复制邀请链接", "Copy Invite Link"))}</button>
              <button type="button" class="pill-btn" data-preview-web-route="invite">${esc(t("在浏览器预览受邀注册页 →", "Preview /register View →"))}</button>
            </div>
          </div>
          <div class="sec-label"><span>${esc(t("已注册好友列表 · 管理员免邮件一键重置密码 (GET /api/v1/admin/users)", "FRIENDS DIRECTORY & 1-CLICK RESET LINK"))}</span></div>
          ${state.friends
            .map(
              (u) => `
            <div class="host-card">
              <div>
                <div class="host-card-title">@${esc(u.handle)} · <span style="color:var(--muted);font-weight:500;">${esc(u.displayName)}</span></div>
                <div class="host-card-sub mono">${Number(u.onlineDevices)}/${Number(u.totalDevices)} devices online · last login ${esc(u.lastLogin)}</div>
              </div>
              <div style="display:flex;gap:5px;">
                <button type="button" class="pill-btn amber" data-issue-reset="${esc(u.handle)}">🔑 ${esc(t("复制重置链接", "Copy Reset Link"))}</button>
              </div>
            </div>`
            )
            .join("")}
          <div style="display:flex;justify-content:space-between;align-items:center;font-size:11px;color:var(--muted);">
            <span>${esc(t("最新签发单次链接（24h 有效，数据库仅存 SHA-256）", "Latest 24h single-use SHA-256 hashed reset link"))}</span>
            <button type="button" class="pill-btn" data-preview-web-route="reset">${esc(t("预览 /reset-password 页 →", "Preview /reset-password →"))}</button>
          </div>
        `;
      } else if (state.settingsTab === "hosts") {
        tabBody = `
          <div class="host-card" style="border-color:rgba(245,158,11,0.4);">
            <div>
              <div class="host-card-title">✨ ${esc(t("Coda.app 应用内一键自更新 (SHA256SUMS 强校验)", "Coda.app In-App Self-Updater (SHA256SUMS Verified)"))}</div>
              <div class="host-card-sub">${esc(
                state.macAppUpdated
                  ? t("当前已运行最新 v1.1.0 原生二进制（已剥离 quarantine）", "Running latest v1.1.0 binary (quarantine stripped)")
                  : t("检测到新版本 v1.1.0 · 自动下载校验 SHA256SUMS 并原子热替换重启", "New version v1.1.0 ready · Verifies SHA256SUMS & relaunches")
              )}</div>
            </div>
            <button type="button" class="pill-btn ${state.macAppUpdated ? "green" : "amber"}" data-self-update>
              ${esc(state.macAppUpdated ? "✓ v1.1.0" : t("立即自更新", "Update & Restart"))}
            </button>
          </div>
          <div class="host-card">
            <div>
              <div class="host-card-title">${esc(t("允许远端控制本机（Coda.app Host 模式）", "Allow Remote Control (Coda.app Host Mode)"))}</div>
              <div class="host-card-sub">${esc(t("随时在 hybrid 被控与纯 client 控制端间切换", "Switch cleanly between hybrid host and client-only"))}</div>
            </div>
            <button type="button" class="pill-btn ${state.allowRemoteControl ? "green" : ""}" data-toggle-host-mode>${state.allowRemoteControl ? "ON" : "OFF"}</button>
          </div>
          ${state.hosts
            .map(
              (h) => `
            <div class="host-card">
              <div>
                <div class="host-card-title"><span class="dot ${h.online ? "online" : "offline"}" style="margin-right:6px;"></span>${esc(h.name)}</div>
                <div class="host-card-sub mono">${esc(h.platform)} · CPU ${esc(h.cpu)} · RAM ${esc(h.ram)} · ${esc(h.daemonVer)}</div>
              </div>
              <div style="display:flex;gap:5px;">
                ${h.needsOta ? `<button type="button" class="pill-btn sky" data-upgrade-host="${esc(h.id)}">⚡ OTA v1.1.0</button>` : ""}
                <button type="button" class="pill-btn" data-toast="${esc(t(`已重命名主机 ${h.name}`, `Renamed ${h.name}`))}">${esc(t("改名", "Rename"))}</button>
              </div>
            </div>`
            )
            .join("")}
          <div class="host-card">
            <div>
              <div class="host-card-title">${esc(t("清理离线浏览器残留设备（防自删保护）", "Prune Stale Offline Browser Devices"))}</div>
              <div class="host-card-sub">${esc(t("自动排除当前活跃 auth.device_id 会话", "Excludes current active session device_id safely"))}</div>
            </div>
            <button type="button" class="pill-btn" data-toast="${esc(t("🧹 已安全清理 2 台过期离线浏览器虚拟设备", "🧹 Pruned 2 stale offline browser devices"))}">${esc(t("一键清理", "Prune Stale"))}</button>
          </div>
        `;
      } else if (state.settingsTab === "security") {
        tabBody = `
          <div class="host-card" style="flex-direction:column;align-items:stretch;gap:7px;">
            <div class="host-card-title">🔑 ${esc(t("使用管理员重置令牌改密 (POST /api/v1/auth/reset-password)", "Reset Password with Admin Token (rst_...)"))}</div>
            <input class="term-input" value="${esc(state.lastResetToken)}" placeholder="rst_... or https://codahub.dev/reset-password?token=rst_..." />
            <input class="term-input" type="password" placeholder="${esc(t("输入新密码（至少 8 位）", "New password (min 8 chars)"))}" value="••••••••••••" />
            <button type="button" class="pill-btn amber" data-toast="${esc(
              t("密码已通过单次令牌重置，旧登录会话已吊销（主机隧道不断连）", "Password reset via token; full sessions rotated while keeping daemons online")
            )}">${esc(t("使用令牌重置密码", "Reset with Token"))}</button>
          </div>
          <div class="host-card">
            <div>
              <div class="host-card-title">${esc(t("活跃登录会话 (3)", "Active Full Sessions (3)"))}</div>
              <div class="host-card-sub">MacBook Pro · iPhone 16 Pro · Web Safari PWA</div>
            </div>
            <button type="button" class="pill-btn danger" data-toast="${esc(t("已注销其他全部登录设备会话", "Signed out all other sessions"))}">${esc(t("全部踢下线", "Logout All"))}</button>
          </div>
        `;
      } else {
        tabBody = `
          <div class="host-card" style="flex-direction:column;align-items:stretch;gap:8px;">
            <div class="host-card-title">🎙 ${esc(t("三级语音听写引擎（免配置原生 + E2EE 守护进程 Whisper）", "3-Tier Voice Dictation (Zero-Config Native + E2EE Daemon Whisper)"))}</div>
            <div style="display:flex;gap:6px;flex-wrap:wrap;">
              <button type="button" class="pill-btn ${state.voiceMode === "native_e2ee" ? "sky" : ""}" data-voice-mode="native_e2ee">${esc(
                t("✓ 免配置原生识别 + E2EE TranscribeAudio", "✓ Zero-Config Native + E2EE Daemon")
              )}</button>
              <button type="button" class="pill-btn ${state.voiceMode === "https_whisper" ? "sky" : ""}" data-voice-mode="https_whisper">HTTPS Whisper API</button>
            </div>
            <input class="term-input" value="https://api.openai.com/v1" />
            <input class="term-input" value="gpt-4o-mini-transcribe" />
            <button type="button" class="pill-btn primary" data-toast="${esc(t("语音与云端快捷指令配置已同步", "Voice & synced snippets settings saved"))}">${esc(t("保存语音配置", "Save Voice Config"))}</button>
          </div>
        `;
      }

      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>⚙ ${esc(t("Coda v1.1.0 全端设置、推送与好友中心", "Coda v1.1.0 Settings, Push & Friends Center"))}</span>
          <button type="button" class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="display:flex;gap:5px;flex-wrap:wrap;">
          ${[
            ["alerts", t("🔔 锁屏推送 & Webhook", "🔔 Web Push & Alerts")],
            ["friends", t("🤝 好友邀请 & 重置", "🤝 Friends & Reset")],
            ["hosts", t("🖥 主机健康 & OTA", "🖥 Hosts & Self-Update")],
            ["security", t("🔐 令牌改密 & 会话", "🔐 Security & Reset")],
            ["voice", t("🎙 零配置语音", "🎙 Voice & Whisper")],
          ]
            .map(
              ([id, lbl]) =>
                `<button type="button" class="pill-btn ${state.settingsTab === id ? "primary" : ""}" data-settings-tab="${esc(id)}">${esc(lbl)}</button>`
            )
            .join("")}
        </div>
        ${tabBody}
      `;
    }

    backdrop.classList.remove("hidden");
  }

  function closeModal() {
    currentModalKind = null;
    document.getElementById("proto-modal-backdrop").classList.add("hidden");
  }

  // ===================== Spec Annotations (v1.1.0) =====================
  const PLATFORM_SPECS = {
    web_desktop: {
      titleZh: "Web 桌面端控制台 (v1.1.0)",
      titleEn: "Web Desktop Console (v1.1.0)",
      res: "≥ 900px · OLED #000000 · sw.js VAPID",
      pointsZh: [
        "<strong>好友深链与重置路由</strong>：支持 `/register?invite=CODA-XXXXXX` 自动填码注册、`/reset-password?token=rst_...` 免邮件单次改密与三步新手引导卡片。",
        "<strong>W3C VAPID 锁屏推送</strong>：`sw.js` 配合 `RFC 8292 ES256 + RFC 8291 aes128gcm` 实现浏览器/桌面原生锁屏通知（零终端明文）。",
        "<strong>三合一 E2EE 检视器</strong>：右侧内置 `Changes（含 ?? 未跟踪新文件绿色 + 补丁） | Files | 沙箱 Port Preview`。",
        "<strong>多模态输入增强</strong>：云同步 Saved Snippets 胶囊、富文本安全的 `⌘V` 截图自动上传、`⌘F` 搜索、`⌘J` 速跳与 `.md` 导出。",
      ],
      pointsEn: [
        "<strong>Invite & Reset Routes</strong>: `/register?invite=CODA-XXXXXX` auto-fill, `/reset-password?token=rst_...`, and 3-step onboarding card.",
        "<strong>W3C VAPID Web Push</strong>: `sw.js` with `RFC 8292 ES256 + RFC 8291 aes128gcm` zero-knowledge lock-screen alerts.",
        "<strong>Unified E2EE Inspector</strong>: `Changes (incl. ?? green untracked diffs) | Files | Sandboxed Port Preview`.",
        "<strong>Multimodal Composer</strong>: Synced Snippets, rich-text safe `⌘V` screenshot paste, `⌘F` search, `⌘J` jump, and `.md` export.",
      ],
    },
    web_mobile: {
      titleZh: "Web 手机端 / Safari PWA (v1.1.0)",
      titleEn: "Web Mobile / Safari PWA (v1.1.0)",
      res: "< 900px · 100dvh + iOS Lock-Screen Push",
      pointsZh: [
        "<strong>iOS 主屏幕锁屏推送</strong>：添加到主屏幕后一键订阅 W3C VAPID Web Push，Agent 待审批时直接点亮 iPhone 锁屏。",
        "<strong>移动端三合一检视弹窗</strong>：右上角 `±` 一键查看 `??` 新文件 Git Diff、文件树与沙箱化远端端口预览。",
        "<strong>云同步快捷指令条</strong>：拇指单手点选 `Run Tests & Fix` 等云端 Snippets，免打字驱动 5 大 AI Agent。",
      ],
      pointsEn: [
        "<strong>iOS Home-Screen Push</strong>: One-click W3C VAPID subscription lights up iOS lock screen on `Needs-You`.",
        "<strong>Mobile Inspector Sheet</strong>: Tap `±` for E2EE `Changes (?? diffs) | Files | Sandboxed Port Preview`.",
        "<strong>Thumb Snippet Strip</strong>: One-tap synced prompt snippets + zero-config `SpeechRecognition`.",
      ],
    },
    mac_gpui: {
      titleZh: "macOS GPUI 桌面端 (Coda.app v1.1.0)",
      titleEn: "macOS GPUI Desktop (Coda.app v1.1.0)",
      res: "Metal 120Hz · Self-Updater · 127.0.0.1 Proxy",
      pointsZh: [
        "<strong>应用内一键自更新</strong>：顶栏 `✨ 升级 v1.1.0` 自动下载并严格校验 `SHA256SUMS`、清理 `quarantine`、原子替换重启。",
        "<strong>127.0.0.1 全方法回环隧道</strong>：自动映射远端端口至本地 `127.0.0.1`，完整透传 `GET/POST/PUT/PATCH/DELETE` 与 `Set-Cookie`。",
        "<strong>置顶待确认收件箱 (`⌘J`)</strong>：侧栏置顶阻塞会话 + Dock 角标提醒，配合 `⌘D` 分屏、`⌘F` 搜索与 `.md` 导出。",
        "<strong>工作区 `git clone` 引导与截图直传</strong>：支持远端目录浏览 / `git clone` 启动 5 大 Agent，以及 `⌘V` 截图加密上传。",
      ],
      pointsEn: [
        "<strong>In-App Self-Updater</strong>: Header `✨ Update v1.1.0` verifies `SHA256SUMS`, strips `quarantine`, swaps atomically & restarts.",
        "<strong>127.0.0.1 Full-Method Tunnel</strong>: Proxies `GET/POST/PUT/PATCH/DELETE` + `Location`/`Set-Cookie` over E2EE.",
        "<strong>Pinned Attention Inbox (`⌘J`)</strong>: Cyclic jump to blocked agents + `⌘D` split + `⌘F` search + `.md` export.",
        "<strong>Git Clone Bootstrapper & `⌘V` Image</strong>: Clone remote repos on launch + paste screenshots directly into PTY.",
      ],
    },
    ipad: {
      titleZh: "iPadOS 分栏工作台 (v1.1.0)",
      titleEn: "iPadOS Split Workbench (v1.1.0)",
      res: "Regular Split · 288pt Sidebar · Swift 6",
      pointsZh: [
        "<strong>Swift 6 多主机并发路由</strong>：多台主机独立维护中继票据，侧栏实时展示主机 CPU/RAM 与一键 `OTA v1.1.0`。",
        "<strong>相册 `PhotosPicker` 与剪贴板图直传</strong>：一键将 iPad 截图或设计稿经 E2EE 传至远端工作区供 Agent 视觉分析。",
        "<strong>免配置 `SFSpeechRecognizer` + 云端 Snippets</strong>：无需填写 API Key 即可原生语音转写，亦可切换 E2EE 守护进程 Whisper。",
      ],
      pointsEn: [
        "<strong>Multi-Host Relay & Health</strong>: Live CPU/RAM badges and 1-tap zero-downtime `OTA v1.1.0` per host.",
        "<strong>PhotosPicker & Clipboard Paste</strong>: Upload screenshots/mockups over E2EE directly into the prompt.",
        "<strong>Zero-Config Speech & Snippets</strong>: Native `SFSpeechRecognizer` + E2EE daemon Whisper + synced Snippets.",
      ],
    },
    iphone: {
      titleZh: "iPhone 原生客户端 (v1.1.0)",
      titleEn: "iPhone Native SwiftUI (v1.1.0)",
      res: "390×844 · Dynamic Island · Swift 6",
      pointsZh: [
        "<strong>随身审批与快速恢复</strong>：配合 Bark / Telegram / PWA 锁屏推送，掏出手机 1 秒点击 `1 · Yes` 放行阻塞任务。",
        "<strong>好友邀请与令牌改密</strong>：在移动端设置页即可一键复制好友邀请深链或为忘密好友签发单次重置链接。",
        "<strong>三合一检视与相册直传</strong>：单手查看 `??` 新文件 Diff、沙箱网页预览，并通过 `PhotosPicker` 发送报错截图。",
      ],
      pointsEn: [
        "<strong>1-Tap Approval Loop</strong>: Pair with Web Push / Bark / Telegram to approve blocked `Needs-You` agents on the go.",
        "<strong>Friend Invite & Token Reset</strong>: Copy `/register?invite=` links or issue 1-click admin password reset links.",
        "<strong>Mobile Inspector & Photos</strong>: Inspect `??` untracked diffs, sandboxed ports, and attach photos via E2EE.",
      ],
    },
    iphone_duo: {
      titleZh: "iPhone Duo 折叠屏三态工作台 (v1.1.0)",
      titleEn: "iPhone Duo Foldable (3 Postures · v1.1.0)",
      res: "466×678 ↔ 890×626 ↔ 626×890",
      pointsZh: [
        "<strong>7.6\" 展开双栏 + 内联三合一检视屏</strong>：左叶运行终端，跨过 24pt 折痕在右叶直接切换 `Changes(??) | Files | Port Preview`。",
        "<strong>笔电半折控制甲板</strong>：下半屏集成 5 大 Agent 快速切换胶囊、云同步 Saved Snippets、截图上传与完整双排键盘。",
        "<strong>瞬时动态列重排</strong>：外屏 58 列 ↔ 展开双栏 ↔ 笔电 80 列无缝触发远端 `TIOCSWINSZ`。",
      ],
      pointsEn: [
        "<strong>7.6\" Unfolded Inline Inspector</strong>: Terminal on Leaf 1 + full `Changes(??) | Files | Port Preview` on Leaf 2 across 24pt crease.",
        "<strong>Tabletop Control Deck</strong>: Lower leaf packs 5-agent switcher, synced Snippets, photo paste, voice & tactile keypad.",
        "<strong>Instant Column Reflow</strong>: Seamless `GhosttyReflowEngine` resize across Cover, Unfolded, and Tabletop poses.",
      ],
    },
  };

  function renderSpecDrawer() {
    const spec = PLATFORM_SPECS[state.platform];
    const pts = state.lang === "zh" ? spec.pointsZh : spec.pointsEn;
    document.getElementById("spec-drawer-title").textContent = t("v1.1.0 交互说明与设计标注", "v1.1.0 UX Notes & Tokens");
    document.getElementById("spec-drawer-content").innerHTML = `
      <div class="spec-block">
        <h4>${esc(state.lang === "zh" ? spec.titleZh : spec.titleEn)}</h4>
        <div class="mono" style="font-size:11px;color:var(--green);margin-bottom:8px;">${esc(spec.res)}</div>
        <ul>${pts.map((p) => `<li>${p}</li>`).join("")}</ul>
      </div>
      <div class="spec-block">
        <h4>${esc(t("v1.1.0 必测交互清单", "v1.1.0 Interactive Try-Out"))}</h4>
        <ul>
          <li>${t("点击 <strong>+ 启动器</strong> 体验 5 大 AI Agent 与 <strong>Git Clone 引导</strong>。", "Click <strong>+</strong> to launch all 5 AI Agents or toggle <strong>Git Clone</strong>.")}</li>
          <li>${t("切换检视器 <strong>Changes (??) / Files / Port Preview</strong> 查看新建文件绿色补丁与 `127.0.0.1` 隧道。", "Switch <strong>Changes (??) / Files / Port Preview</strong> for untracked diffs & live tunnels.")}</li>
          <li>${t("在输入栏点击 <strong>云同步 Snippet 胶囊</strong>、<strong>📎 ⌘V 图</strong> 或 <strong>🎙 语音</strong>。", "Try <strong>Synced Snippet capsules</strong>, <strong>📎 ⌘V Img</strong>, and <strong>🎙 Voice</strong>.")}</li>
          <li>${t("打开 <strong>⚙ 设置</strong> 体验 <strong>一键锁屏 Web Push</strong> 与 <strong>好友重置链接</strong>。", "Open <strong>⚙ Settings</strong> for <strong>1-Click Web Push</strong> & <strong>Friend Reset Links</strong>.")}</li>
        </ul>
      </div>
      <div class="spec-block">
        <h4>${esc(t("共享设计 Token", "Shared Design Tokens"))}</h4>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#000000;"></span>Web OLED</span><span>#000000</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#0a0b0e;"></span>GPUI Canvas</span><span>#0A0B0E</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#f59e0b;"></span>Needs You / OTA</span><span>#F59E0B</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#38bdf8;"></span>Working / Tunnel</span><span>#38BDF8</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#22c55e;"></span>Direct / Diff+ / ??</span><span>#22C55E</span></div>
      </div>
    `;
  }

  // ===================== Top Chrome & Master Render =====================
  const DEVICE_LIST = [
    ["web_desktop", "🌐", "Web 桌面端", "Web Desktop"],
    ["web_mobile", "📲", "Web PWA", "Web PWA"],
    ["mac_gpui", "🖥", "macOS GPUI", "macOS GPUI"],
    ["ipad", "⬛", "iPadOS", "iPadOS"],
    ["iphone", "📱", "iPhone", "iPhone"],
    ["iphone_duo", "📖", "iPhone Duo", "iPhone Duo"],
  ];

  function renderSubbar() {
    const el = document.getElementById("platform-subbar");
    let left = "";
    if (state.platform === "web_desktop") {
      left =
        [
          ["workbench", t("工作台 /console", "Console /console")],
          ["landing", t("官网 /", "Landing /")],
          ["invite", t("好友邀请 /register", "Invite /register")],
          ["reset", t("密码重置 /reset-password", "Reset /reset-password")],
          ["activate", t("设备授权 /activate", "Activate /activate")],
        ]
          .map(
            ([v, lbl]) =>
              `<button type="button" class="sub-pill ${state.webSubView === v ? "active" : ""}" data-web-sub="${esc(v)}">${esc(lbl)}</button>`
          )
          .join("") +
        `<button type="button" class="sub-pill ${state.showOnboarding ? "active" : ""}" data-toggle-onboarding>👋 ${esc(t("三步引导卡", "3-Step Guide"))}</button>`;
    } else if (state.platform === "iphone") {
      left = `
        <button type="button" class="sub-pill ${state.iphoneScreen === "list" ? "active" : ""}" data-iphone-screen="list">${esc(t("收件箱 / 空间列表", "Inbox / Spaces"))}</button>
        <button type="button" class="sub-pill ${state.iphoneScreen === "terminal" ? "active" : ""}" data-iphone-screen="terminal">${esc(t("全屏终端视图", "Full-Screen Terminal"))}</button>
        <button type="button" class="sub-pill ${state.showOnboarding ? "active" : ""}" data-toggle-onboarding>👋 ${esc(t("三步引导卡", "3-Step Guide"))}</button>
        <button type="button" class="sub-pill" data-open-settings-tab="friends">🤝 ${esc(t("邀请/重置", "Friends"))}</button>
      `;
    } else if (state.platform === "iphone_duo") {
      left = [
        ["cover", t('5.4" 外屏单手态', '5.4" Cover')],
        ["inner_landscape", t('7.6" 展开双栏态', '7.6" Unfolded Split')],
        ["tabletop", t("笔电半折态 (Tabletop)", "Tabletop Laptop")],
      ]
        .map(
          ([v, lbl]) =>
            `<button type="button" class="sub-pill ${state.duoPosture === v ? "active" : ""}" data-duo-posture="${esc(v)}">${esc(lbl)}</button>`
        )
        .join("");
    } else if (state.platform === "mac_gpui") {
      left = `
        <button type="button" class="sub-pill ${!state.macSplit ? "active" : ""}" data-mac-split="0">${esc(t("单终端工作台", "Single Pane"))}</button>
        <button type="button" class="sub-pill ${state.macSplit ? "active" : ""}" data-mac-split="1">${esc(t("双分屏 (⌘D)", "Split (⌘D)"))}</button>
        <button type="button" class="sub-pill ${state.showOnboarding ? "active" : ""}" data-toggle-onboarding>👋 ${esc(t("三步引导卡", "3-Step Guide"))}</button>
        <button type="button" class="sub-pill" data-open-modal="launcher">+ ${esc(t("5 大 Agent / Clone", "5 Agents / Clone"))}</button>
        <button type="button" class="sub-pill" data-open-modal="palette">⌘K</button>
        <button type="button" class="sub-pill" data-open-modal="settings">⚙ ${esc(t("设置/推送/好友", "Settings"))}</button>
      `;
    } else if (state.platform === "ipad") {
      left = `
        <button type="button" class="sub-pill ${state.ipadSidebar ? "active" : ""}" data-toggle-ipad-sidebar>${esc(t("切换侧边栏 (⌘B)", "Toggle Sidebar (⌘B)"))}</button>
        <button type="button" class="sub-pill" data-open-modal="inspector">± ${esc(t("三合一检视器", "Unified Inspector"))}</button>
        <button type="button" class="sub-pill" data-open-modal="compose">✎ ${esc(t("Composer / Snippets / 语音", "Composer & Snippets"))}</button>
        <button type="button" class="sub-pill ${state.showOnboarding ? "active" : ""}" data-toggle-onboarding>👋 ${esc(t("三步引导卡", "3-Step Guide"))}</button>
      `;
    } else if (state.platform === "web_mobile") {
      left = `
        <button type="button" class="sub-pill ${state.webMobileDrawer ? "active" : ""}" data-toggle-web-drawer>☰ ${esc(t("抽屉侧边栏", "Slide Drawer"))}</button>
        <button type="button" class="sub-pill ${state.webMobileKeypad ? "active" : ""}" data-toggle-web-keypad>⌨ ${esc(t("双排触控键", "Touch Keypad"))}</button>
        <button type="button" class="sub-pill" data-open-modal="inspector">± ${esc(t("Files / Diff / Port", "Inspector"))}</button>
        <button type="button" class="sub-pill" data-open-settings-tab="alerts">🔔 ${esc(t("iOS 锁屏推送", "iOS Web Push"))}</button>
      `;
    }
    const spec = PLATFORM_SPECS[state.platform];
    el.innerHTML = `<div class="subbar-group">${left}</div><div class="subbar-meta">${esc(spec.res)}</div>`;
  }

  function render() {
    document.getElementById("studio-sub").textContent = t(
      "v1.1.0 · 全端可交互高保真原型台",
      "v1.1.0 · 6-Platform Interactive Prototype Studio"
    );
    document.getElementById("device-tabs").innerHTML = DEVICE_LIST.map(
      ([id, icon, zh, en]) =>
        `<button type="button" class="device-tab ${state.platform === id ? "active" : ""}" data-platform="${esc(id)}">${esc(icon)} ${esc(state.lang === "zh" ? zh : en)}</button>`
    ).join("");

    const route = ROUTES[state.routeIdx];
    const routeBtn = document.getElementById("btn-route-sim");
    routeBtn.textContent = state.lang === "zh" ? route.labelZh : route.labelEn;
    routeBtn.className = `studio-chip route ${route.isRelay ? "relay" : ""}`;

    const needsBtn = document.getElementById("btn-trigger-needs");
    const nCount = needsYouSessions().length;
    needsBtn.textContent = t(`▲ 触发待确认 (${nCount})`, `▲ Trigger Needs-You (${nCount})`);
    needsBtn.className = "studio-chip accent";

    const notesBtn = document.getElementById("btn-toggle-notes");
    notesBtn.textContent = state.showNotes ? t("隐藏标注", "Hide Notes") : t("设计标注", "Show Notes");
    document.getElementById("spec-drawer").classList.toggle("collapsed", !state.showNotes);
    document.getElementById("btn-toggle-lang").textContent = state.lang === "zh" ? "EN" : "中文";

    renderSubbar();
    renderSpecDrawer();

    const stage = document.getElementById("stage-viewport");
    if (state.platform === "web_desktop") stage.innerHTML = renderWebDesktop();
    else if (state.platform === "web_mobile") stage.innerHTML = renderWebMobile();
    else if (state.platform === "mac_gpui") stage.innerHTML = renderMacGPUI();
    else if (state.platform === "ipad") stage.innerHTML = renderIPad();
    else if (state.platform === "iphone") stage.innerHTML = renderIPhone();
    else if (state.platform === "iphone_duo") stage.innerHTML = renderIPhoneDuo();

    if (currentModalKind && !document.getElementById("proto-modal-backdrop").classList.contains("hidden")) {
      openModal(currentModalKind);
    }
  }

  // ===================== Event Delegation & Shortcuts =====================
  document.addEventListener("click", (ev) => {
    if (ev.target.closest("[data-stop-prop]") && !ev.target.closest("button")) {
      return;
    }

    const delSnip = ev.target.closest("[data-del-snippet]");
    if (delSnip) {
      ev.stopPropagation();
      deleteSnippet(delSnip.dataset.delSnippet);
      return;
    }

    const btn = ev.target.closest(
      "[data-platform],[data-web-sub],[data-select-sid],[data-approve],[data-key],[data-open-modal],[data-open-settings-tab],[data-close-modal],[data-toggle-voice],[data-toggle-host-mode],[data-mac-split],[data-mac-drawer],[data-mac-insp],[data-toggle-web-split],[data-toggle-ipad-sidebar],[data-iphone-back],[data-iphone-tab],[data-iphone-screen],[data-toggle-iphone-keypad],[data-duo-posture],[data-duo-mode],[data-toggle-web-drawer],[data-close-web-drawer],[data-toggle-web-keypad],[data-launch-agent],[data-launcher-host],[data-boot-mode],[data-pick-folder],[data-settings-tab],[data-submit-compose],[data-insp-tab],[data-select-file],[data-select-port],[data-preview-method],[data-jump-needs],[data-toggle-term-search],[data-close-term-search],[data-export-md],[data-self-update],[data-upgrade-host],[data-apply-snippet],[data-save-snippet],[data-run-snippet-now],[data-paste-shot],[data-clear-shot],[data-toggle-web-push],[data-toggle-webhook],[data-new-invite],[data-issue-reset],[data-preview-web-route],[data-complete-invite],[data-toggle-onboarding],[data-voice-mode],[data-toast]"
    );
    if (!btn) {
      if (ev.target.id === "proto-modal-backdrop") closeModal();
      return;
    }

    if (btn.dataset.toast) showToast(btn.dataset.toast);
    if (btn.dataset.platform) {
      state.platform = btn.dataset.platform;
      render();
      return;
    }
    if (btn.dataset.webSub) {
      state.webSubView = btn.dataset.webSub;
      render();
      return;
    }
    if (btn.dataset.completeInvite !== undefined) {
      state.webSubView = "workbench";
      state.showOnboarding = true;
      showToast(t("🎉 注册成功！已自动展开新用户三步上手引导卡", "🎉 Account created! Showing 3-Step Friend Quickstart Card"));
      render();
      return;
    }
    if (btn.dataset.toggleOnboarding !== undefined) {
      state.showOnboarding = !state.showOnboarding;
      render();
      return;
    }
    if (btn.dataset.selectSid) {
      selectSession(btn.dataset.selectSid, btn.dataset.enterMobile === "1");
      if (btn.dataset.closeAfter === "1") closeModal();
      return;
    }
    if (btn.dataset.approve) {
      approveSession(btn.dataset.sid, btn.dataset.approve);
      return;
    }
    if (btn.dataset.jumpNeeds !== undefined) {
      if (btn.dataset.closeAfter === "1") closeModal();
      jumpNextNeedsYou();
      return;
    }
    if (btn.dataset.toggleTermSearch !== undefined) {
      if (btn.dataset.closeAfter === "1") closeModal();
      state.termSearchOpen = !state.termSearchOpen;
      render();
      if (state.termSearchOpen) {
        const inp = document.getElementById("proto-term-search-input");
        if (inp) inp.focus();
      }
      return;
    }
    if (btn.dataset.closeTermSearch !== undefined) {
      state.termSearchOpen = false;
      state.termSearchQuery = "";
      render();
      return;
    }
    if (btn.dataset.exportMd) {
      if (btn.dataset.closeAfter === "1") closeModal();
      const target = state.sessions.find((x) => x.id === btn.dataset.exportMd) || activeSession();
      exportSessionMarkdown(target);
      return;
    }
    if (btn.dataset.selfUpdate !== undefined) {
      triggerMacSelfUpdate();
      return;
    }
    if (btn.dataset.upgradeHost) {
      upgradeHostOta(btn.dataset.upgradeHost);
      return;
    }
    if (btn.dataset.applySnippet) {
      const ta = document.getElementById("modal-compose-ta");
      if (ta) state.voiceDraft = ta.value;
      applySnippet(btn.dataset.applySnippet);
      return;
    }
    if (btn.dataset.saveSnippet !== undefined) {
      const ta = document.getElementById("modal-compose-ta");
      if (ta) state.voiceDraft = ta.value;
      saveCurrentDraftAsSnippet();
      return;
    }
    if (btn.dataset.runSnippetNow) {
      const sn = state.snippets.find((x) => x.id === btn.dataset.runSnippetNow);
      closeModal();
      if (sn) sendCommand(sn.prompt);
      return;
    }
    if (btn.dataset.pasteShot !== undefined) {
      const ta = document.getElementById("modal-compose-ta");
      if (ta) state.voiceDraft = ta.value;
      simulateScreenshotPaste();
      return;
    }
    if (btn.dataset.clearShot !== undefined) {
      state.attachedShot = null;
      render();
      return;
    }
    if (btn.dataset.key) {
      const k = btn.dataset.key;
      if (k === "enter") sendCommand("", btn.dataset.sid);
      else if (k === "^C") approveSession(btn.dataset.sid, "esc");
      else showToast(t(`已通过 E2EE pty_input 发送按键 [${k}]`, `Sent key [${k}] over E2EE pty_input`));
      return;
    }
    if (btn.dataset.openSettingsTab) {
      state.settingsTab = btn.dataset.openSettingsTab;
      openModal("settings");
      return;
    }
    if (btn.dataset.openModal) {
      openModal(btn.dataset.openModal);
      return;
    }
    if (btn.dataset.closeModal !== undefined) {
      closeModal();
      return;
    }
    if (btn.dataset.toggleVoice !== undefined) {
      const ta = document.getElementById("modal-compose-ta");
      if (ta) state.voiceDraft = ta.value;
      toggleVoice();
      return;
    }
    if (btn.dataset.toggleHostMode !== undefined) {
      state.allowRemoteControl = !state.allowRemoteControl;
      showToast(
        state.allowRemoteControl
          ? t("已开启被控模式（Coda.app hybrid 监听就绪）", "Host remote control enabled (hybrid)")
          : t("已关闭被控模式（Coda.app 切换为纯控制端）", "Host remote control disabled (client-only)")
      );
      render();
      return;
    }
    if (btn.dataset.toggleWebSplit !== undefined) {
      state.webSplit = !state.webSplit;
      render();
      return;
    }
    if (btn.dataset.macSplit !== undefined) {
      state.macSplit = btn.dataset.macSplit === "1";
      render();
      return;
    }
    if (btn.dataset.macInsp) {
      const targetTab = btn.dataset.macInsp;
      if (state.macRightDrawer === targetTab && state.inspectorTab === targetTab) {
        state.macRightDrawer = null;
      } else {
        state.macRightDrawer = targetTab;
        state.inspectorTab = targetTab;
      }
      render();
      return;
    }
    if (btn.dataset.macDrawer !== undefined) {
      state.macRightDrawer =
        btn.dataset.macDrawer === "none" ? null : state.macRightDrawer === btn.dataset.macDrawer ? null : btn.dataset.macDrawer;
      render();
      return;
    }
    if (btn.dataset.inspTab) {
      state.inspectorTab = btn.dataset.inspTab;
      if (state.platform === "mac_gpui" && state.macRightDrawer && state.macRightDrawer !== "mesh") {
        state.macRightDrawer = btn.dataset.inspTab;
      }
      render();
      return;
    }
    if (btn.dataset.selectFile) {
      state.selectedFilePath = btn.dataset.selectFile;
      render();
      return;
    }
    if (btn.dataset.selectPort) {
      state.selectedPort = Number(btn.dataset.selectPort) || 5173;
      render();
      return;
    }
    if (btn.dataset.previewMethod) {
      state.previewMethod = btn.dataset.previewMethod;
      render();
      return;
    }
    if (btn.dataset.toggleIpadSidebar !== undefined) {
      state.ipadSidebar = !state.ipadSidebar;
      render();
      return;
    }
    if (btn.dataset.iphoneBack !== undefined) {
      state.iphoneScreen = "list";
      render();
      return;
    }
    if (btn.dataset.iphoneTab) {
      state.iphoneTab = btn.dataset.iphoneTab;
      render();
      return;
    }
    if (btn.dataset.iphoneScreen) {
      state.iphoneScreen = btn.dataset.iphoneScreen;
      render();
      return;
    }
    if (btn.dataset.toggleIphoneKeypad !== undefined) {
      state.iphoneKeypad = !state.iphoneKeypad;
      render();
      return;
    }
    if (btn.dataset.duoPosture) {
      state.duoPosture = btn.dataset.duoPosture;
      render();
      return;
    }
    if (btn.dataset.duoMode) {
      state.duoSplitMode = btn.dataset.duoMode;
      render();
      return;
    }
    if (btn.dataset.toggleWebDrawer !== undefined) {
      state.webMobileDrawer = !state.webMobileDrawer;
      render();
      return;
    }
    if (btn.dataset.closeWebDrawer !== undefined) {
      state.webMobileDrawer = false;
      render();
      return;
    }
    if (btn.dataset.toggleWebKeypad !== undefined) {
      state.webMobileKeypad = !state.webMobileKeypad;
      render();
      return;
    }
    if (btn.dataset.launcherHost) {
      state.launcherHostId = btn.dataset.launcherHost;
      openModal("launcher");
      return;
    }
    if (btn.dataset.bootMode) {
      state.launcherBootMode = btn.dataset.bootMode;
      openModal("launcher");
      return;
    }
    if (btn.dataset.pickFolder) {
      state.launcherFolder = btn.dataset.pickFolder;
      openModal("launcher");
      return;
    }
    if (btn.dataset.launchAgent) {
      const folderInput = document.getElementById("launcher-folder-input");
      const cloneUrlInput = document.getElementById("launcher-clone-url");
      const cloneDirInput = document.getElementById("launcher-clone-dir");
      if (folderInput) state.launcherFolder = folderInput.value.trim() || state.launcherFolder;
      if (cloneUrlInput) state.launcherCloneUrl = cloneUrlInput.value.trim() || state.launcherCloneUrl;
      if (cloneDirInput) state.launcherCloneDir = cloneDirInput.value.trim() || state.launcherCloneDir;
      launchNewSession(btn.dataset.launchAgent);
      return;
    }
    if (btn.dataset.settingsTab) {
      state.settingsTab = btn.dataset.settingsTab;
      openModal("settings");
      return;
    }
    if (btn.dataset.toggleWebPush !== undefined) {
      state.webPushEnabled = !state.webPushEnabled;
      state.webPushCount = state.webPushEnabled ? 2 : 0;
      showToast(
        state.webPushEnabled
          ? t("🔔 已注册 Service Worker (/sw.js) 并订阅 VAPID 锁屏推送", "🔔 Subscribed to W3C VAPID Web Push (/sw.js)")
          : t("已取消当前设备的 Web Push 锁屏订阅", "Unsubscribed Web Push on this device")
      );
      render();
      return;
    }
    if (btn.dataset.toggleWebhook) {
      const k = btn.dataset.toggleWebhook;
      state.webhooks[k] = !state.webhooks[k];
      render();
      return;
    }
    if (btn.dataset.newInvite !== undefined) {
      const suffix = Math.random().toString(36).slice(2, 8).toUpperCase();
      state.inviteCode = `CODA-${suffix}`;
      showToast(t(`已生成新好友邀请码 ${state.inviteCode}`, `Generated invite code ${state.inviteCode}`));
      render();
      return;
    }
    if (btn.dataset.issueReset) {
      const handle = btn.dataset.issueReset;
      const rand = Math.random().toString(36).slice(2, 12) + Math.random().toString(36).slice(2, 12);
      state.lastResetHandle = handle;
      state.lastResetToken = `rst_${rand}`;
      showToast(
        t(
          `🔑 已为 @${handle} 生成 24h 单次重置链接并复制：https://codahub.dev/reset-password?token=${state.lastResetToken.slice(0, 14)}...`,
          `🔑 Copied 24h reset link for @${handle}: https://codahub.dev/reset-password?token=${state.lastResetToken.slice(0, 14)}...`
        )
      );
      render();
      return;
    }
    if (btn.dataset.previewWebRoute) {
      closeModal();
      state.platform = "web_desktop";
      state.webSubView = btn.dataset.previewWebRoute;
      render();
      return;
    }
    if (btn.dataset.voiceMode) {
      state.voiceMode = btn.dataset.voiceMode;
      render();
      return;
    }
    if (btn.dataset.submitCompose !== undefined) {
      const ta = document.getElementById("modal-compose-ta");
      sendCommand(ta ? ta.value : state.voiceDraft);
      closeModal();
      return;
    }
  });

  document.addEventListener("input", (ev) => {
    const target = ev.target;
    if (target && target.id === "proto-term-search-input") {
      const cursor = target.selectionStart;
      state.termSearchQuery = target.value;
      render();
      const nextInp = document.getElementById("proto-term-search-input");
      if (nextInp) {
        nextInp.focus();
        if (typeof cursor === "number") nextInp.setSelectionRange(cursor, cursor);
      }
    } else if (target && target.matches(".composer-stack .term-input")) {
      state.voiceDraft = target.value;
    }
  });

  document.addEventListener("keydown", (ev) => {
    const mod = ev.metaKey || ev.ctrlKey;
    const isSplitShortcut =
      (ev.metaKey && !ev.ctrlKey && !ev.altKey && !ev.shiftKey && ev.key.toLowerCase() === "d") ||
      (ev.ctrlKey && ev.shiftKey && !ev.metaKey && !ev.altKey && ev.key.toLowerCase() === "d");
    const modalOpen = !document.getElementById("proto-modal-backdrop").classList.contains("hidden");
    if (mod && ev.key.toLowerCase() === "k") {
      ev.preventDefault();
      openModal("palette");
    } else if (mod && ev.key.toLowerCase() === "j") {
      ev.preventDefault();
      jumpNextNeedsYou();
    } else if (isSplitShortcut && !modalOpen) {
      if (state.platform === "web_desktop" && state.webSubView === "workbench") {
        ev.preventDefault();
        state.webSplit = !state.webSplit;
        render();
      } else if (state.platform === "mac_gpui") {
        ev.preventDefault();
        state.macSplit = !state.macSplit;
        render();
      }
    } else if (mod && ev.key.toLowerCase() === "f") {
      ev.preventDefault();
      state.termSearchOpen = !state.termSearchOpen;
      render();
      if (state.termSearchOpen) {
        const inp = document.getElementById("proto-term-search-input");
        if (inp) inp.focus();
      }
    } else if (ev.key === "Escape") {
      if (!document.getElementById("proto-modal-backdrop").classList.contains("hidden")) {
        closeModal();
      } else if (state.termSearchOpen) {
        state.termSearchOpen = false;
        render();
      }
    }
  });

  document.addEventListener("submit", (ev) => {
    const form = ev.target.closest("[data-cmd-form]");
    if (!form) return;
    ev.preventDefault();
    const input = form.querySelector("input");
    sendCommand(input ? input.value : "", form.dataset.cmdForm);
  });

  document.getElementById("btn-route-sim").addEventListener("click", () => {
    state.routeIdx = (state.routeIdx + 1) % ROUTES.length;
    const r = ROUTES[state.routeIdx];
    showToast(t(`已切换模拟链路：${r.labelZh}`, `Simulated route: ${r.labelEn}`));
    render();
  });

  document.getElementById("btn-trigger-needs").addEventListener("click", () => {
    const s = state.sessions[0];
    s.state = "needs_you";
    s.statusMsg = "Claude Code 请求执行 `cargo test --workspace` 并推送远程分支";
    s.lines.push({ cls: "t-amber", text: "⚠ OSC 7501 [blocked/permission]: Approve running full test suite?" });
    state.activeSessionId = s.id;
    showToast(t("🔔 已触发 OSC 7501 待确认提醒（全端角标 + VAPID 锁屏推送同步高亮）", "🔔 Triggered OSC 7501 Needs-You event"));
    render();
  });

  document.getElementById("btn-toggle-notes").addEventListener("click", () => {
    state.showNotes = !state.showNotes;
    render();
  });
  document.getElementById("btn-close-notes").addEventListener("click", () => {
    state.showNotes = false;
    render();
  });
  document.getElementById("btn-toggle-lang").addEventListener("click", () => {
    state.lang = state.lang === "zh" ? "en" : "zh";
    render();
  });

  render();
})();
