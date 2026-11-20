(() => {
  "use strict";

  // ===================== Shared Live State =====================
  const ROUTES = [
    { id: "lan", labelZh: "⚡ 局域网直连 · 1.4ms", labelEn: "⚡ LAN Direct · 1.4ms", short: "LAN P2P", isRelay: false },
    { id: "ipv6", labelZh: "🚀 公网 IPv6 直连 · 9.2ms", labelEn: "🚀 CN IPv6 P2P · 9.2ms", short: "IPv6 P2P", isRelay: false },
    { id: "rawtcp", labelZh: "🔒 国内免备案 TCP :24443 · 24ms", labelEn: "🔒 CN Raw TCP :24443 · 24ms", short: "CN TCP", isRelay: true },
    { id: "wss", labelZh: "☁️ 海外 CDN WSS · 48ms", labelEn: "☁️ CDN WSS · 48ms", short: "CDN WSS", isRelay: true },
  ];

  const state = {
    lang: "zh",
    platform: "mac_gpui", // web_desktop | web_mobile | mac_gpui | ipad | iphone | iphone_duo
    routeIdx: 0,
    showNotes: true,
    webSubView: "workbench", // workbench | landing | activate
    webMobileDrawer: false,
    webMobileKeypad: true,
    macLayout: "workbench", // workbench | terminals
    macSplit: false,
    macRightDrawer: "mesh", // null | changes | files | mesh
    allowRemoteControl: true,
    ipadSidebar: true,
    iphoneTab: "inbox", // inbox | spaces
    iphoneScreen: "list", // list | terminal
    iphoneKeypad: false,
    duoPosture: "inner_landscape", // cover | inner_landscape | tabletop
    duoSplitMode: "split", // split | diff | dual | canvas
    activeSessionId: "s1",
    secondarySessionId: "s2",
    voiceRecording: false,
    voiceDraft: "",
    settingsTab: "hosts",
    hosts: [
      { id: "h-macmini", name: "Mac mini · 家里主力", platform: "macos", role: "hybrid", online: true, branches: ["main", "feat/dirty-vt"] },
      { id: "h-gpu01", name: "prod-gpu-01 · 阿里云", platform: "linux-x86_64", role: "daemon", online: true, branches: ["main", "release/v0.5"] },
      { id: "h-euler", name: "openeuler-arm64", platform: "linux-aarch64", role: "daemon", online: false, branches: ["main"] },
    ],
    sessions: [
      {
        id: "s1",
        hostId: "h-macmini",
        hostName: "Mac mini · 家里主力",
        workspace: "coda",
        color: "#2563eb",
        branch: "feat/dirty-vt",
        agent: "claude",
        title: "实现 Web VT 行级脏区 Diff 渲染",
        state: "needs_you",
        statusMsg: "Claude Code 请求执行 `cd web && bun test` 并写入 src/vt.ts",
        diffAdded: 148,
        diffDeleted: 34,
        lines: [
          { cls: "t-cyan", text: "╭─ claude-code · coda (feat/dirty-vt) · herdr-core t1" },
          { cls: "", text: "│ 已定位 web/src/app.ts:965 全量 innerHTML 重建瓶颈，准备切换为脏行节点池更新：" },
          { cls: "hunk", text: "@@ -412,6 +412,18 @@ export class WebVtTerminal {" },
          { cls: "del", text: "-  renderHtml(showCursor: boolean): string {" },
          { cls: "add", text: "+  private dirtyRows = new Set<number>();" },
          { cls: "add", text: "+  patchDomRows(container: HTMLElement, showCursor: boolean): number {" },
          { cls: "add", text: "+    // Skip rows under active text selection to preserve copy drag" },
          { cls: "", text: "" },
          { cls: "t-amber", text: "⚠ Permission required: Run `cd web && bun test && bun run build`?" },
          { cls: "prompt", text: "❯ 1. Yes, allow once" },
          { cls: "", text: "  2. Always allow for this workspace" },
          { cls: "", text: "  3. Cancel (Esc)" },
        ],
      },
      {
        id: "s2",
        hostId: "h-gpu01",
        hostName: "prod-gpu-01 · 阿里云",
        workspace: "coda-relay",
        color: "#059669",
        branch: "release/v0.5",
        agent: "codex",
        title: "检查多路中继并发保活日志",
        state: "working",
        statusMsg: "Running cargo test -p coda-daemon -- --nocapture",
        diffAdded: 42,
        diffDeleted: 9,
        lines: [
          { cls: "t-cyan", text: "codex@prod-gpu-01 ~/coda-relay (release/v0.5)" },
          { cls: "t-dim", text: "$ coda-daemon status" },
          { cls: "t-green", text: "● daemon online · pid=41822 · herdr-core v0.1.24" },
          { cls: "", text: "  primary relay : wss://codahub.dev/v1/tunnel/ws (connected, 44ms)" },
          { cls: "", text: "  cn raw tcp    : cn-relay.codahub.dev:24443 (connected, 19ms)" },
          { cls: "", text: "  direct p2p    : [::]:24444 (dual-stack IPv4+IPv6 ready)" },
          { cls: "t-dim", text: "  relay ticket  : valid for 27d 14h (auto-renew < 7d)" },
        ],
      },
      {
        id: "s3",
        hostId: "h-macmini",
        hostName: "Mac mini · 家里主力",
        workspace: "coda-apple",
        color: "#7c3aed",
        branch: "main",
        agent: "gemini",
        title: "验证 CodaSelfTest 与 Whisper 录音",
        state: "done",
        unread: true,
        statusMsg: "All Apple Swift 6 self-tests passed",
        diffAdded: 86,
        diffDeleted: 12,
        lines: [
          { cls: "t-dim", text: "$ cd apple && swift build -c release && ./.build/release/CodaSelfTest" },
          { cls: "", text: "Build complete! (11.42s)" },
          { cls: "t-green", text: "[OK] ALL CODA APPLE NATIVE SELF-TESTS PASSED!" },
          { cls: "t-dim", text: "✓ CodaVoiceRecorder HTTPS/localhost URL guard verified" },
        ],
      },
      {
        id: "s4",
        hostId: "h-gpu01",
        hostName: "prod-gpu-01 · 阿里云",
        workspace: "infra-deploy",
        color: "#d97706",
        branch: "main",
        agent: "shell",
        title: "生产容器监控 Shell",
        state: "idle",
        statusMsg: "Idle · bash",
        diffAdded: 0,
        diffDeleted: 0,
        lines: [
          { cls: "t-dim", text: "coda@prod-gpu-01:~/coda-deploy-bundle$ ./status.sh" },
          { cls: "t-green", text: "OK: {\"status\":\"ok\",\"online_devices\":3,\"relay_nodes\":2}" },
          { cls: "", text: "coda@prod-gpu-01:~/coda-deploy-bundle$ " },
        ],
      },
    ],
    files: [
      { path: "web/src/vt.ts", status: "M", added: 92, del: 18 },
      { path: "web/src/app.ts", status: "M", added: 56, del: 16 },
      { path: "crates/coda-desktop/src/lib.rs", status: "M", added: 112, del: 24 },
      { path: "apple/Sources/CodaUI/CodaVoiceRecorder.swift", status: "A", added: 184, del: 0 },
    ],
  };

  // ===================== Helpers =====================
  const t = (zh, en) => (state.lang === "zh" ? zh : en);
  const esc = (s) =>
    String(s ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");

  const activeSession = () => state.sessions.find((s) => s.id === state.activeSessionId) || state.sessions[0];
  const secondarySession = () =>
    state.sessions.find((s) => s.id === state.secondarySessionId && s.id !== state.activeSessionId) ||
    state.sessions.find((s) => s.id !== state.activeSessionId) ||
    state.sessions[0];
  const attentionCount = () => state.sessions.filter((s) => s.state === "needs_you" || (s.state === "done" && s.unread)).length;

  let toastTimer = null;
  function showToast(msg) {
    const el = document.getElementById("proto-toast");
    el.textContent = msg;
    el.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.classList.add("hidden"), 2600);
  }

  function badgeHtml(s) {
    const map = {
      needs_you: t("待确认", "Needs you"),
      working: t("运行中", "Working"),
      done: s.unread ? t("已完成 · 新", "Done · New") : t("已完成", "Done"),
      idle: t("空闲", "Idle"),
    };
    return `<span class="badge ${s.state}">${map[s.state] || s.state}</span>`;
  }

  function avatarHtml(s, size = 28) {
    const ch = (s.workspace || "C")[0].toUpperCase();
    return `<span class="avatar" style="width:${size}px;height:${size}px;background:${s.color}28;border:1px solid ${s.color}66;color:${s.color};font-size:${Math.round(size * 0.44)}px;">${ch}</span>`;
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
      s.lines.push({ cls: "t-dim", text: "$ cd web && bun test && bun run build" });
      s.lines.push({ cls: "t-green", text: "✓ 12 pass, 0 fail (62 expect calls) · Built web/dist/" });
      s.state = "done";
      s.unread = false;
      s.statusMsg = "Completed · 12 pass";
      showToast(t(`已批准 ${s.workspace}（多端状态已实时同步）`, `Approved ${s.workspace} (synced across all views)`));
    }
    render();
  }

  function sendCommand(text, targetId = state.activeSessionId) {
    const clean = (text || "").trim();
    const s = state.sessions.find((x) => x.id === targetId) || activeSession();
    if (!s) return;
    if (!clean) {
      s.lines.push({ cls: "t-dim", text: "↵ (bare Enter sent via pty_input)" });
    } else {
      s.lines.push({ cls: "t-cyan", text: `$ ${clean}` });
      s.lines.push({ cls: "t-dim", text: `[pty_input · E2EE ${ROUTES[state.routeIdx].short}] executed in ${s.workspace} (${s.hostName})` });
      if (s.state === "idle") s.state = "working";
    }
    state.voiceDraft = "";
    render();
  }

  function toggleVoice() {
    if (state.voiceRecording) {
      state.voiceRecording = false;
      const sample = t(
        "帮我把终端脏行渲染的单元测试补齐并跑一遍构建",
        "Add unit tests for dirty-row VT rendering and run build"
      );
      state.voiceDraft = state.voiceDraft ? `${state.voiceDraft} ${sample}` : sample;
      showToast(t("🎙 Whisper 转写完成，已填入输入框", "🎙 Whisper transcript inserted"));
    } else {
      state.voiceRecording = true;
      showToast(t("🎙 正在录音（再次点击停止并调用 /audio/transcriptions）...", "🎙 Recording... click again to transcribe"));
    }
    render();
  }

  function launchNewSession(agent, hostId, workspace) {
    const host = state.hosts.find((h) => h.id === hostId) || state.hosts[0];
    const id = "s" + (state.sessions.length + 1);
    const colors = ["#0891b2", "#db2777", "#4f46e5", "#0d9488"];
    const next = {
      id,
      hostId: host.id,
      hostName: host.name,
      workspace: workspace || "coda",
      color: colors[state.sessions.length % colors.length],
      branch: host.branches[0] || "main",
      agent,
      title: `${agent} · ${t("新建会话", "New Session")}`,
      state: "working",
      statusMsg: `Launched ${agent}`,
      diffAdded: 0,
      diffDeleted: 0,
      lines: [
        { cls: "t-cyan", text: `● Launched ${agent} on ${host.name} (~/${workspace || "coda"})` },
        { cls: "t-dim", text: `  E2EE v2 session established via ${ROUTES[state.routeIdx].short}` },
        { cls: "", text: `${agent}> ` },
      ],
    };
    state.sessions.unshift(next);
    state.activeSessionId = id;
    closeModal();
    showToast(t(`已在 ${host.name} 启动 ${agent}`, `Launched ${agent} on ${host.name}`));
    render();
  }

  // ===================== Shared Fragments =====================
  function renderTerminalLines(s) {
    return (
      s.lines
        .map((l) => `<div class="t-line ${l.cls || ""}">${esc(l.text) || "&nbsp;"}</div>`)
        .join("") + `<div class="t-line"><span class="t-cursor"></span></div>`
    );
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
          <button class="pill-btn amber" data-approve="1" data-sid="${s.id}">1 · Yes</button>
          <button class="pill-btn amber" data-approve="2" data-sid="${s.id}">2 · Always</button>
          <button class="pill-btn" data-approve="esc" data-sid="${s.id}">Esc</button>
          ${compact ? "" : `<span class="mono" style="margin-left:auto;font-size:10.5px;color:var(--muted);">OSC 7501 · permission</span>`}
        </div>
      </div>
    `;
  }

  function renderKeypad(sid) {
    return `
      <div class="keypad">
        <div class="keypad-grid">
          <div class="keypad-row">
            <button class="k-btn" data-key="esc" data-sid="${sid}">esc</button>
            <button class="k-btn" data-key="ctrl" data-sid="${sid}">ctrl</button>
            <button class="k-btn" data-key="tab" data-sid="${sid}">tab</button>
            <button class="k-btn" data-key="⇧tab" data-sid="${sid}">⇧tab</button>
            <button class="k-btn" data-key="↑" data-sid="${sid}">↑</button>
            <button class="k-btn" data-key="⌫" data-sid="${sid}">⌫</button>
          </div>
          <div class="keypad-row">
            <button class="k-btn danger" data-key="^C" data-sid="${sid}">^C</button>
            <button class="k-btn" data-key="^D" data-sid="${sid}">^D</button>
            <button class="k-btn" data-key="/" data-sid="${sid}">/</button>
            <button class="k-btn" data-key="←" data-sid="${sid}">←</button>
            <button class="k-btn" data-key="↓" data-sid="${sid}">↓</button>
            <button class="k-btn" data-key="→" data-sid="${sid}">→</button>
          </div>
        </div>
        <button class="k-enter" data-key="enter" data-sid="${sid}">↵</button>
      </div>
    `;
  }

  function renderGitDiffPanel(s) {
    return `
      <div style="padding:10px 12px;border-bottom:1px solid var(--studio-border);display:flex;align-items:center;justify-content:space-between;">
        <div>
          <div style="font-size:12.5px;font-weight:600;">${esc(s.workspace)} · Git Diff</div>
          <div class="mono" style="font-size:10.5px;color:var(--muted);">${esc(s.branch)} · <span style="color:var(--green);">+${s.diffAdded}</span> <span style="color:var(--red);">-${s.diffDeleted}</span></div>
        </div>
        <span class="badge working">E2EE</span>
      </div>
      <div class="term-screen" style="font-size:11px;">
        <div class="t-line t-cyan">M  web/src/vt.ts</div>
        <div class="t-line t-cyan">M  web/src/app.ts</div>
        <div class="t-line">&nbsp;</div>
        <div class="t-line hunk">@@ -408,8 +408,16 @@ export class WebVtTerminal {</div>
        <div class="t-line del">-  renderHtml(showCursor = true): string {</div>
        <div class="t-line del">-    return this.rows.map(...).join("");</div>
        <div class="t-line add">+  private dirtyRows = new Set&lt;number&gt;();</div>
        <div class="t-line add">+  renderRowHtml(row: number, showCursor: boolean): string {</div>
        <div class="t-line add">+    // Dirty-row incremental DOM patch (60fps zero-jitter)</div>
        <div class="t-line add">+    this.dirtyRows.delete(row);</div>
        <div class="t-line add">+    return buildStyledSpans(this.grid[row]);</div>
        <div class="t-line add">+  }</div>
      </div>
    `;
  }

  // ===================== 1. Web Desktop Console =====================
  function renderWebDesktop() {
    const s = activeSession();
    const route = ROUTES[state.routeIdx];
    const urlPath =
      state.webSubView === "landing" ? "https://codahub.dev/" :
      state.webSubView === "activate" ? "https://codahub.dev/activate?code=K7M9-4X2Q" :
      "https://codahub.dev/app";

    let inner = "";
    if (state.webSubView === "landing") {
      inner = `
        <div class="landing-wrap">
          <div style="display:flex;align-items:center;justify-content:space-between;">
            <div style="display:flex;align-items:center;gap:8px;font-weight:700;font-size:15px;">
              <span class="dot online"></span> Coda <span class="mono" style="font-size:11px;color:var(--green);">v0.5.0</span>
            </div>
            <div style="display:flex;gap:8px;">
              <button class="pill-btn" data-web-sub="activate">${t("设备授权 /activate", "Device /activate")}</button>
              <button class="pill-btn primary" data-web-sub="workbench">${t("进入 Web 控制台 →", "Open Console →")}</button>
            </div>
          </div>
          <div class="landing-hero">
            <div>
              <h1>${t("把家用 Mac、Linux 服务器与手机连成一张零知识加密开发网", "One zero-knowledge encrypted mesh for Mac, Linux & Mobile")}</h1>
              <p>${t("支持 Claude Code / Codex / Gemini 实时全双工 PTY 终端、免备案国内裸 TCP 中继 + 海外 CDN 双轨并发保活、以及浏览器/原生端 Whisper 语音转写输入。", "Full-duplex herdr-core PTYs, domestic no-ICP raw TCP + CDN WSS multi-relay keep-alive, and Whisper voice dictation across Web & Apple.")}</p>
              <div style="display:flex;gap:8px;flex-wrap:wrap;">
                <button class="pill-btn primary" data-web-sub="workbench">${t("立即体验工作台", "Launch Workbench")}</button>
                <button class="pill-btn" data-open-modal="settings">${t("一键安装命令", "One-Liner Install")}</button>
              </div>
            </div>
            <div class="dl-card" style="background:#090b10;">
              <div class="mono" style="font-size:11px;color:var(--green);">$ curl -fsSL https://codahub.dev/install.sh | sh</div>
              <div class="mono" style="font-size:11px;color:var(--muted);margin-top:6px;">==> [1/4] herdr-core verified (or native PTY fallback)<br/>==> [2/4] coda-daemon v0.5.0 SHA256 OK<br/>==> [3/4] Browser approval: K7M9-4X2Q<br/>==> [4/4] Multi-relay keep-alive active</div>
            </div>
          </div>
          <div class="dl-grid">
            <div class="dl-card">
              <h4>macOS Desktop (Apple Silicon)</h4>
              <p>Coda-Desktop-macos-aarch64-v0.5.0.dmg · Rust GPUI 120Hz + Keychain + Host 被控开关</p>
            </div>
            <div class="dl-card">
              <h4>Linux / macOS Daemon</h4>
              <p>coda-daemon-v0.5.0 · 支持 stop/restart/logout/uninstall、原生 PTY 兜底与 30 天自动续签</p>
            </div>
            <div class="dl-card">
              <h4>Deploy Bundle &amp; GHCR</h4>
              <p>coda-deploy-bundle-v0.5.0.tar.gz · 非 root systemd 沙箱 + Docker Compose + 离线镜像回落</p>
            </div>
          </div>
        </div>
      `;
    } else if (state.webSubView === "activate") {
      inner = `
        <div class="landing-wrap" style="align-items:center;justify-content:center;">
          <div class="dl-card" style="max-width:420px;width:100%;padding:22px;gap:12px;">
            <div style="font-size:16px;font-weight:700;">${t("批准主机入网 (RFC 8628)", "Approve Host Login (RFC 8628)")}</div>
            <p>${t("正在将新设备绑定到当前账号 @vampire（E2EE 公钥指纹已校验）", "Bind this device to @vampire with verified Ed25519/X25519 keys")}</p>
            <div class="mono" style="font-size:26px;font-weight:700;color:var(--amber);text-align:center;padding:12px;background:#08090c;border-radius:10px;border:1px solid var(--border);">K7M9-4X2Q</div>
            <div style="font-size:12px;color:var(--muted);">
              ${t("主机名称", "Host")}: <strong style="color:var(--text);">Mac mini · 家里主力</strong> · <span class="mono">macos / hybrid</span>
            </div>
            <div style="display:flex;gap:8px;margin-top:4px;">
              <button class="pill-btn primary" style="flex:1;justify-content:center;padding:8px;" data-web-sub="workbench" data-toast="${t("已批准主机 K7M9-4X2Q 入网", "Approved K7M9-4X2Q")}">${t("确认批准入网", "Approve Device")}</button>
              <button class="pill-btn" data-web-sub="workbench">${t("取消", "Cancel")}</button>
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
                <span class="dot online"></span> Coda Web
              </div>
              <div style="display:flex;gap:4px;">
                <button class="icon-btn" title="${t("新建 Agent/Shell", "New Agent/Shell")}" data-open-modal="launcher">+</button>
                <button class="icon-btn" title="${t("设置中心", "Settings")}" data-open-modal="settings">⚙</button>
              </div>
            </div>
            <div class="wb-sidebar-scroll">
              <div class="sec-label"><span>${t("在线主机", "HOSTS")} (${state.hosts.filter((h) => h.online).length}/${state.hosts.length})</span></div>
              ${state.hosts
                .map(
                  (h) => `
                  <div style="display:flex;align-items:center;justify-content:space-between;padding:5px 7px;border-radius:7px;background:rgba(255,255,255,0.02);font-size:11.5px;">
                    <span style="display:flex;align-items:center;gap:6px;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                      <span class="dot ${h.online ? "online" : "offline"}"></span>${esc(h.name)}
                    </span>
                    <span class="mono" style="font-size:10px;color:var(--dim);">${h.role}</span>
                  </div>`
                )
                .join("")}
              <div class="sec-label" style="margin-top:4px;"><span>${t("活跃会话", "SESSIONS")}</span><span>${state.sessions.length}</span></div>
              ${state.sessions
                .map(
                  (sess) => `
                  <button class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${sess.id}">
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
              <span class="mono" style="font-size:10.5px;color:${route.isRelay ? "var(--sky)" : "var(--green)"};">🔒 E2EE · HTTPS WSS</span>
              <button class="pill-btn" data-open-modal="settings">${t("设置", "Settings")}</button>
            </div>
          </aside>

          <section class="term-col">
            <div class="term-head">
              <div style="min-width:0;">
                <div style="font-size:13px;font-weight:600;">${esc(s.title)}</div>
                <div class="mono" style="font-size:10.5px;color:var(--muted);">${esc(s.hostName)} · ${esc(s.workspace)} (${esc(s.branch)}) · <span style="color:var(--green);">Dirty-Row VT</span></div>
              </div>
              <div style="display:flex;gap:6px;align-items:center;">
                <button class="pill-btn" data-open-modal="launcher">+ ${t("新建", "New")}</button>
                <button class="pill-btn danger" data-approve="esc" data-sid="${s.id}">^C</button>
              </div>
            </div>
            ${renderNeedsBanner(s)}
            <div class="term-screen">${renderTerminalLines(s)}</div>
            <form class="term-input-bar" data-cmd-form="${s.id}">
              <input class="term-input" placeholder="${t("直接向终端发送按键或指令（回车即发，支持空回车）...", "Send command or bare Enter directly to PTY...")}" value="${esc(state.voiceDraft)}" />
              <button type="button" class="mic-btn ${state.voiceRecording ? "recording" : ""}" data-toggle-voice>
                🎙 ${state.voiceRecording ? t("停止", "Stop") : t("语音", "Voice")}
              </button>
              <button type="submit" class="pill-btn primary">${t("发送", "Send")}</button>
            </form>
          </section>

          <aside class="right-drawer">
            ${renderGitDiffPanel(s)}
          </aside>
        </div>
      `;
    }

    return `
      <div class="frame-browser">
        <div class="chrome-bar">
          <div class="traffic-lights"><span class="tl red"></span><span class="tl yellow"></span><span class="tl green"></span></div>
          <div class="url-bar">🔒 ${urlPath}</div>
          <span class="mono" style="font-size:10.5px;color:var(--muted);">Tab Session · sessionStorage</span>
        </div>
        ${inner}
      </div>
    `;
  }

  // ===================== 2. Web Mobile / Safari PWA =====================
  function renderWebMobile() {
    const s = activeSession();
    return `
      <div class="frame-phone">
        <div class="ios-status">
          <span>9:41</span>
          <div class="dynamic-island"></div>
          <span>5G 100%</span>
        </div>
        <div class="term-head" style="padding:7px 10px;">
          <button class="icon-btn" data-toggle-web-drawer>☰</button>
          <div style="min-width:0;flex:1;text-align:center;">
            <div style="font-size:12.5px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(s.workspace)} · ${esc(s.agent)}</div>
            <div class="mono" style="font-size:10px;color:var(--muted);">100dvh PWA · ${esc(s.branch)}</div>
          </div>
          <button class="icon-btn ${state.webMobileKeypad ? "active" : ""}" data-toggle-web-keypad title="Keypad">⌨</button>
        </div>
        ${renderNeedsBanner(s, true)}
        <div class="term-screen" style="font-size:11px;padding:10px;">${renderTerminalLines(s)}</div>
        ${state.webMobileKeypad ? renderKeypad(s.id) : ""}
        <form class="term-input-bar" style="padding:7px 9px;" data-cmd-form="${s.id}">
          <input class="term-input" placeholder="${t("一步直发命令...", "Send directly...")}" value="${esc(state.voiceDraft)}" />
          <button type="button" class="mic-btn ${state.voiceRecording ? "recording" : ""}" data-toggle-voice>🎙</button>
          <button type="submit" class="pill-btn sky">↵</button>
        </form>
        <div class="home-indicator"></div>
        ${
          state.webMobileDrawer
            ? `
          <div class="mobile-drawer-backdrop" data-close-web-drawer>
            <div class="mobile-drawer-panel" onclick="event.stopPropagation()">
              <div style="padding:12px;border-bottom:1px solid var(--border);display:flex;justify-content:space-between;align-items:center;">
                <strong>Coda PWA ${t("会话抽屉", "Drawer")}</strong>
                <button class="icon-btn" data-close-web-drawer>✕</button>
              </div>
              <div class="wb-sidebar-scroll">
                ${state.sessions
                  .map(
                    (sess) => `
                  <button class="sess-item ${sess.id === s.id ? "active" : ""}" data-select-sid="${sess.id}" data-enter-mobile="1">
                    ${avatarHtml(sess, 26)}
                    <div class="sess-meta">
                      <div class="sess-title">${esc(sess.workspace)}</div>
                      <div class="sess-sub">${esc(sess.title)}</div>
                    </div>
                    ${badgeHtml(sess)}
                  </button>`
                  )
                  .join("")}
              </div>
              <div class="wb-sidebar-foot">
                <button class="pill-btn" data-open-modal="launcher">+ ${t("新建", "New")}</button>
                <button class="pill-btn" data-open-modal="settings">⚙ ${t("设置", "Settings")}</button>
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
            <strong style="font-size:12px;">${t("云网与被控设置", "Mesh & Host Mode")}</strong>
            <button class="icon-btn" data-mac-drawer="none">✕</button>
          </div>
          <div class="right-drawer-body">
            <div class="host-card">
              <div>
                <div class="host-card-title">${t("允许远端控制本机", "Allow Remote Control")}</div>
                <div class="host-card-sub">${state.allowRemoteControl ? t("已开启 · 本机可被手机/公司 Mac 遥控", "ON · Controllable from phone/other Mac") : t("已关闭 · 仅作为纯控制端运行", "OFF · Client-only mode")}</div>
              </div>
              <button class="pill-btn ${state.allowRemoteControl ? "green" : ""}" data-toggle-host-mode>
                ${state.allowRemoteControl ? t("已开启", "ON") : t("已关闭", "OFF")}
              </button>
            </div>
            <div class="sec-label" style="margin-top:6px;"><span>${t("已配对主机", "PAIRED HOSTS")}</span><span>${state.hosts.length}</span></div>
            ${state.hosts
              .map(
                (h) => `
                <div class="host-card" style="flex-direction:column;align-items:stretch;gap:5px;">
                  <div style="display:flex;align-items:center;justify-content:space-between;">
                    <span style="display:flex;align-items:center;gap:6px;font-weight:600;"><span class="dot ${h.online ? "online" : "offline"}"></span>${esc(h.name)}</span>
                    <span class="mono" style="font-size:10px;color:var(--muted);">${h.role}</span>
                  </div>
                  <div style="display:flex;justify-content:space-between;align-items:center;">
                    <span class="mono" style="font-size:10px;color:var(--dim);">${h.platform}</span>
                    <button class="pill-btn" style="padding:2px 7px;font-size:10.5px;" data-open-modal="settings">${t("改名 / 管理", "Manage")}</button>
                  </div>
                </div>`
              )
              .join("")}
            <div style="font-size:10.5px;color:var(--dim);padding:6px 4px;">
              🔐 ${t("会话 Token 已存入 macOS Keychain（磁盘零明文）", "Session token secured in macOS Keychain")}
            </div>
          </div>
        </aside>
      `;
    } else if (state.macRightDrawer === "changes") {
      rightDrawerHtml = `<aside class="right-drawer">${renderGitDiffPanel(s)}</aside>`;
    } else if (state.macRightDrawer === "files") {
      rightDrawerHtml = `
        <aside class="right-drawer">
          <div class="term-head">
            <strong style="font-size:12px;">${t("工作区文件", "Workspace Files")}</strong>
            <button class="icon-btn" data-mac-drawer="none">✕</button>
          </div>
          <div class="right-drawer-body">
            ${state.files
              .map(
                (f) => `
                <div class="host-card">
                  <span class="mono" style="font-size:11px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${esc(f.path)}</span>
                  <span class="mono" style="font-size:10px;color:${f.status === "A" ? "var(--green)" : "var(--amber)"};">${f.status}</span>
                </div>`
              )
              .join("")}
          </div>
        </aside>
      `;
    }

    return `
      <div class="frame-mac">
        <div class="chrome-bar">
          <div style="display:flex;align-items:center;gap:12px;">
            <div class="traffic-lights"><span class="tl red"></span><span class="tl yellow"></span><span class="tl green"></span></div>
            <span style="font-size:12px;font-weight:700;">Coda.app</span>
            <span class="mono" style="font-size:10.5px;color:var(--muted);">Metal 120Hz · ${state.allowRemoteControl ? "hybrid" : "client"}</span>
          </div>
          <div style="display:flex;align-items:center;gap:5px;">
            <button class="pill-btn ${!state.macSplit ? "primary" : ""}" data-mac-split="0">${t("单窗", "Single")}</button>
            <button class="pill-btn ${state.macSplit ? "primary" : ""}" data-mac-split="1">${t("分屏 ⌘D", "Split ⌘D")}</button>
            <button class="pill-btn" data-open-modal="palette">⌘K</button>
          </div>
          <div style="display:flex;align-items:center;gap:5px;">
            <button class="pill-btn ${state.macRightDrawer === "changes" ? "sky" : ""}" data-mac-drawer="changes">Changes</button>
            <button class="pill-btn ${state.macRightDrawer === "files" ? "sky" : ""}" data-mac-drawer="files">Files</button>
            <button class="pill-btn ${state.macRightDrawer === "mesh" ? "sky" : ""}" data-mac-drawer="mesh">Mesh</button>
            <button class="icon-btn" data-open-modal="settings" title="Settings ⌘,">⚙</button>
          </div>
        </div>
        <div class="wb-body">
          <aside class="wb-sidebar" style="width:236px;">
            <div class="wb-sidebar-scroll">
              <div class="sec-label"><span>${t("工作区 & 终端", "SPACES & TERMINALS")}</span><button class="icon-btn" style="width:20px;height:20px;" data-open-modal="launcher">+</button></div>
              ${state.sessions
                .map(
                  (sess) => `
                  <button class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${sess.id}">
                    ${avatarHtml(sess, 24)}
                    <div class="sess-meta">
                      <div class="sess-title">${esc(sess.workspace)}</div>
                      <div class="sess-sub">${esc(sess.hostName)} · ${esc(sess.agent)}</div>
                    </div>
                    ${badgeHtml(sess)}
                  </button>`
                )
                .join("")}
            </div>
            <div class="wb-sidebar-foot">
              <span class="mono" style="font-size:10px;color:${route.isRelay ? "var(--sky)" : "var(--green)"};">${route.short}</span>
              <button class="pill-btn ${state.allowRemoteControl ? "green" : ""}" style="font-size:10.5px;padding:3px 7px;" data-toggle-host-mode>
                ${state.allowRemoteControl ? t("被控: 开", "Host: ON") : t("被控: 关", "Host: OFF")}
              </button>
            </div>
          </aside>

          <section class="term-col">
            <div style="flex:1;min-height:0;display:flex;gap:1px;background:#1e222b;">
              <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#0a0b0e;">
                <div class="term-head">
                  <span style="font-size:12px;font-weight:600;">${esc(s.workspace)} · ${esc(s.title)}</span>
                  <span class="mono" style="font-size:10px;color:var(--muted);">Bracketed Paste · Mouse Select</span>
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
                    <span class="badge ${s2.state}">${s2.state}</span>
                  </div>
                  <div class="term-screen">${renderTerminalLines(s2)}</div>
                </div>`
                  : ""
              }
            </div>
            <form class="term-input-bar" data-cmd-form="${s.id}">
              <input class="term-input" placeholder="${t("直接键入终端或回车发送（支持 Cmd+C 选区复制 / Cmd+V 粘贴）...", "Type directly into GPUI PTY (Cmd+C selection / Bracketed Paste)...")}" value="${esc(state.voiceDraft)}" />
              <button type="submit" class="pill-btn primary">${t("执行", "Run")}</button>
            </form>
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
          <span>9:41 · Tue Nov 20</span>
          <span class="mono" style="font-size:11px;color:var(--muted);">Coda · iPadOS 18 Split View</span>
          <span>Wi-Fi · 96%</span>
        </div>
        <div class="wb-body">
          ${
            state.ipadSidebar
              ? `
            <aside class="wb-sidebar" style="width:272px;background:#13151a;">
              <div style="padding:10px 12px;display:flex;align-items:center;justify-content:space-between;">
                <span style="font-weight:700;font-size:15px;"><span class="dot online" style="margin-right:6px;"></span>Coda</span>
                <div style="display:flex;gap:6px;align-items:center;">
                  ${attentionCount() > 0 ? `<span class="red-badge" style="background:var(--amber);color:#000;">${attentionCount()}</span>` : ""}
                  <button class="icon-btn" data-open-modal="launcher">+</button>
                </div>
              </div>
              <div class="wb-sidebar-scroll">
                <div class="sec-label"><span>${t("工作空间（多分支）", "SPACES & WORKTREES")}</span></div>
                ${state.hosts
                  .filter((h) => h.online)
                  .map(
                    (h) => `
                  <div style="background:#0b0c0f;border:1px solid var(--border);border-radius:9px;padding:8px;display:flex;flex-direction:column;gap:4px;">
                    <div style="font-size:11.5px;font-weight:600;display:flex;justify-content:space-between;">
                      <span><span class="dot online" style="margin-right:5px;"></span>${esc(h.name)}</span>
                      <span class="mono" style="font-size:10px;color:var(--sky);">${ROUTES[state.routeIdx].short}</span>
                    </div>
                    <div class="mono" style="font-size:10px;color:var(--muted);">${h.branches.map((b) => `⎇ ${b}`).join("  ·  ")}</div>
                  </div>`
                  )
                  .join("")}
                <div class="sec-label" style="margin-top:4px;"><span>${t("收件箱优先级", "INBOX PRIORITY")}</span></div>
                ${state.sessions
                  .map(
                    (sess) => `
                  <button class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${sess.id}">
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
                <button class="pill-btn" data-open-modal="settings">⚙ ${t("设置", "Settings")}</button>
                <button class="pill-btn" data-open-modal="launcher">+ ${t("新建终端", "New Terminal")}</button>
              </div>
            </aside>`
              : ""
          }

          <section class="term-col">
            <div class="term-head">
              <div style="display:flex;align-items:center;gap:8px;">
                <button class="icon-btn" data-toggle-ipad-sidebar>◧</button>
                <div>
                  <div style="font-size:13px;font-weight:600;">${esc(s.title)}</div>
                  <div class="mono" style="font-size:10.5px;color:var(--muted);">${esc(s.workspace)} · ${esc(s.branch)} · ${esc(s.hostName)}</div>
                </div>
              </div>
              <div style="display:flex;gap:6px;">
                <button class="pill-btn" data-open-modal="diff">± Diff</button>
                <button class="pill-btn" data-open-modal="compose">✎ ${t("撰写 / 🎙语音", "Compose / 🎙Voice")}</button>
              </div>
            </div>
            ${renderNeedsBanner(s)}
            <div class="term-screen">${renderTerminalLines(s)}</div>
            <div style="padding:8px 12px 12px;display:flex;align-items:center;justify-content:center;gap:8px;flex-wrap:wrap;">
              <div style="background:rgba(26,29,36,0.96);border:1px solid var(--border);border-radius:999px;padding:4px 8px;display:flex;gap:4px;">
                <button class="pill-btn" data-key="esc" data-sid="${s.id}">esc</button>
                <button class="pill-btn" data-key="tab" data-sid="${s.id}">tab</button>
                <button class="pill-btn" data-key="⇧tab" data-sid="${s.id}">⇧tab</button>
                <button class="pill-btn danger" data-key="^C" data-sid="${s.id}">^C</button>
              </div>
              <button class="mic-btn ${state.voiceRecording ? "recording" : ""}" style="border-radius:999px;" data-open-modal="compose">🎙 ${t("语音撰写", "Voice Compose")}</button>
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
          <button class="pill-btn" data-iphone-back>‹ ${t("收件箱", "Inbox")}${att > 0 ? ` (${att})` : ""}</button>
          <div style="min-width:0;flex:1;text-align:center;">
            <div style="font-size:12px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(s.workspace)}</div>
            <div class="mono" style="font-size:9.5px;color:var(--muted);">${esc(s.branch)} · ${ROUTES[state.routeIdx].short}</div>
          </div>
          <button class="icon-btn" data-open-modal="diff" title="Git Diff">±</button>
        </div>
        ${renderNeedsBanner(s, true)}
        <div class="term-screen" style="font-size:11px;padding:10px;">${renderTerminalLines(s)}</div>
        ${state.iphoneKeypad ? renderKeypad(s.id) : ""}
        <div style="padding:6px 10px 8px;display:flex;align-items:center;gap:6px;">
          <div style="flex:1;background:#1a1d24;border:1px solid var(--border);border-radius:999px;padding:3px 6px;display:flex;gap:3px;overflow-x:auto;">
            <button class="pill-btn" style="padding:3px 7px;font-size:10.5px;" data-key="esc" data-sid="${s.id}">esc</button>
            <button class="pill-btn" style="padding:3px 7px;font-size:10.5px;" data-key="tab" data-sid="${s.id}">tab</button>
            <button class="pill-btn" style="padding:3px 7px;font-size:10.5px;" data-key="⇧tab" data-sid="${s.id}">⇧tab</button>
            <button class="pill-btn ${state.iphoneKeypad ? "sky" : ""}" style="padding:3px 7px;font-size:10.5px;" data-toggle-iphone-keypad>⌨</button>
            <button class="pill-btn danger" style="padding:3px 7px;font-size:10.5px;" data-key="^C" data-sid="${s.id}">^C</button>
          </div>
          <button class="icon-btn" style="border-radius:50%;width:34px;height:34px;" data-open-modal="compose" title="Compose & Voice">✎</button>
        </div>
      `;
    } else if (state.iphoneTab === "inbox") {
      screen = `
        <div style="padding:8px 16px 6px;display:flex;align-items:center;justify-content:space-between;">
          <h2 style="font-size:24px;font-weight:700;">${t("收件箱", "Inbox")}</h2>
          <div style="display:flex;gap:6px;align-items:center;">
            <span class="mono" style="font-size:10px;color:var(--green);background:var(--elevated);border:1px solid var(--border);padding:3px 7px;border-radius:999px;">${ROUTES[state.routeIdx].short}</span>
            <button class="icon-btn" style="border-radius:50%;" data-open-modal="settings">⚙</button>
          </div>
        </div>
        <div style="flex:1;overflow-y:auto;padding:8px 14px 72px;display:flex;flex-direction:column;gap:10px;">
          ${state.sessions
            .map(
              (sess) => `
            <button class="sess-item ${sess.state === "needs_you" ? "needs" : ""}" style="background:#13151a;border-color:${sess.state === "needs_you" ? "rgba(245,158,11,0.45)" : "var(--border)"};padding:11px;" data-select-sid="${sess.id}" data-enter-mobile="1">
              ${avatarHtml(sess, 34)}
              <div class="sess-meta">
                <div class="sess-title">${esc(sess.workspace)} · ${esc(sess.title)}</div>
                <div class="sess-sub">${esc(sess.branch)} · ${esc(sess.hostName)}</div>
                ${sess.state === "needs_you" ? `<div style="font-size:10.5px;color:var(--amber);margin-top:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${esc(sess.statusMsg)}</div>` : ""}
              </div>
              ${badgeHtml(sess)}
            </button>`
            )
            .join("")}
        </div>
        <div class="mobile-bottom-pill">
          <button class="mobile-tab-btn active" data-iphone-tab="inbox">${t("收件箱", "Inbox")}${att > 0 ? `<span class="red-badge">${att}</span>` : ""}</button>
          <button class="mobile-tab-btn" data-iphone-tab="spaces">${t("空间", "Spaces")}</button>
        </div>
      `;
    } else {
      screen = `
        <div style="padding:8px 16px 6px;display:flex;align-items:center;justify-content:space-between;">
          <h2 style="font-size:24px;font-weight:700;">${t("空间", "Spaces")}</h2>
          <button class="icon-btn" style="border-radius:50%;" data-open-modal="launcher">+</button>
        </div>
        <div style="flex:1;overflow-y:auto;padding:8px 14px 72px;display:flex;flex-direction:column;gap:12px;">
          ${state.hosts
            .map(
              (h) => `
            <div style="background:#13151a;border:1px solid var(--border);border-radius:14px;padding:12px;display:flex;flex-direction:column;gap:8px;">
              <div style="display:flex;justify-content:space-between;align-items:center;">
                <span style="font-size:13px;font-weight:600;"><span class="dot ${h.online ? "online" : "offline"}" style="margin-right:6px;"></span>${esc(h.name)}</span>
                <span class="mono" style="font-size:10px;color:var(--muted);">${h.platform}</span>
              </div>
              <div class="mono" style="font-size:10.5px;color:var(--muted);">${h.branches.map((b) => `⎇ ${b}`).join(" · ")}</div>
              <div style="display:grid;grid-template-columns:1fr 1fr;gap:6px;">
                ${state.sessions
                  .filter((x) => x.hostId === h.id)
                  .map(
                    (sess) => `
                  <button class="sess-item" style="flex-direction:column;align-items:flex-start;gap:4px;background:#0b0c0f;border-color:var(--border);" data-select-sid="${sess.id}" data-enter-mobile="1">
                    <div style="display:flex;width:100%;justify-content:space-between;align-items:center;">
                      <span class="mono" style="font-size:10.5px;font-weight:600;">${esc(sess.agent)}</span>
                      ${badgeHtml(sess)}
                    </div>
                    <div class="sess-sub" style="width:100%;">${esc(sess.title)}</div>
                  </button>`
                  )
                  .join("")}
              </div>
            </div>`
            )
            .join("")}
        </div>
        <div class="mobile-bottom-pill">
          <button class="mobile-tab-btn" data-iphone-tab="inbox">${t("收件箱", "Inbox")}${att > 0 ? `<span class="red-badge">${att}</span>` : ""}</button>
          <button class="mobile-tab-btn active" data-iphone-tab="spaces">${t("空间", "Spaces")}</button>
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
                  <div style="font-size:12.5px;font-weight:600;">${esc(s.workspace)} · ${esc(s.agent)}</div>
                  <div class="mono" style="font-size:10px;color:var(--muted);">5.4" Cover (466×678) · 58×28</div>
                </div>
                <button class="icon-btn" data-open-modal="compose">✎</button>
              </div>
              ${renderNeedsBanner(s, true)}
              <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
              ${renderKeypad(s.id)}
            </div>
            <div class="duo-rail">
              <div style="font-size:11.5px;font-weight:700;"><span class="dot online" style="margin-right:4px;"></span>Duo</div>
              <span class="mono" style="font-size:9px;color:var(--green);">58×28</span>
              <button class="duo-rail-btn active" data-duo-posture="cover">${t("外屏", "Cover")}</button>
              <button class="duo-rail-btn" data-duo-posture="inner_landscape">${t("展开", "Unfold")}</button>
              <button class="duo-rail-btn" data-duo-posture="tabletop">${t("笔电", "Laptop")}</button>
              <div style="margin-top:auto;display:flex;flex-direction:column;gap:6px;width:100%;align-items:center;">
                <button class="icon-btn" data-open-modal="settings">⚙</button>
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
                <span style="font-size:12.5px;font-weight:700;">Coda Duo · ${t("笔电半折态", "Tabletop Laptop Pose")}</span>
                <span class="mono" style="font-size:10px;color:var(--green);">626×890 · 80×18</span>
              </div>
              <div style="flex:1;min-height:0;display:flex;flex-direction:column;background:#08090c;">
                <div class="term-screen" style="font-size:11.5px;">${renderTerminalLines(s)}</div>
              </div>
              <div class="fold-crease-h">─── FOLD CREASE · 24PT .division ───</div>
              <div style="flex:1;min-height:0;padding:10px 12px;display:flex;flex-direction:column;gap:8px;background:#0b0c0f;overflow-y:auto;">
                <div style="display:flex;gap:5px;overflow-x:auto;">
                  ${state.sessions
                    .map(
                      (sess) => `
                    <button class="pill-btn ${sess.id === s.id ? "sky" : ""}" style="white-space:nowrap;font-size:10.5px;" data-select-sid="${sess.id}">${esc(sess.workspace)}/${esc(sess.agent)}</button>`
                    )
                    .join("")}
                </div>
                ${renderNeedsBanner(s, true)}
                <form class="term-input-bar" style="padding:0;border:none;background:transparent;" data-cmd-form="${s.id}">
                  <input class="term-input" placeholder="${t("向下半屏发送命令或语音追问...", "Send command to upper screen...")}" value="${esc(state.voiceDraft)}" />
                  <button type="button" class="mic-btn ${state.voiceRecording ? "recording" : ""}" data-toggle-voice>🎙</button>
                  <button type="submit" class="pill-btn sky">↑</button>
                </form>
                ${renderKeypad(s.id)}
              </div>
            </div>
            <div class="duo-rail">
              <div style="font-size:11.5px;font-weight:700;">Duo</div>
              <button class="duo-rail-btn" data-duo-posture="cover">${t("外屏", "Cover")}</button>
              <button class="duo-rail-btn" data-duo-posture="inner_landscape">${t("双栏", "Split")}</button>
              <button class="duo-rail-btn active" data-duo-posture="tabletop">${t("笔电", "Laptop")}</button>
              <div style="margin-top:auto;"><button class="icon-btn" data-open-modal="settings">⚙</button></div>
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
            <strong style="font-size:12px;">${t("左侧目录 Leaf 1", "Directory Leaf 1")}</strong>
            <button class="pill-btn sky" style="font-size:10.5px;padding:3px 8px;" data-open-modal="launcher">+ ${t("新建终端", "New")}</button>
          </div>
          <div class="wb-sidebar-scroll">
            ${state.sessions
              .map(
                (sess) => `
              <button class="sess-item ${sess.id === s.id ? "active" : ""} ${sess.state === "needs_you" ? "needs" : ""}" data-select-sid="${sess.id}">
                ${avatarHtml(sess, 26)}
                <div class="sess-meta">
                  <div class="sess-title">${esc(sess.workspace)} · ${esc(sess.title)}</div>
                  <div class="sess-sub">${esc(sess.agent)} · ${esc(sess.branch)}</div>
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
            <button class="pill-btn" data-open-modal="compose">🎙 ${t("语音", "Voice")}</button>
          </div>
          ${renderNeedsBanner(s, true)}
          <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
        </div>
      `;
    } else if (state.duoSplitMode === "diff") {
      leftLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#08090c;">
          <div class="term-head"><strong style="font-size:12px;">L1 · ${esc(s.workspace)}</strong></div>
          <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
        </div>
      `;
      rightLeaf = `<div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#0a0b0e;">${renderGitDiffPanel(s)}</div>`;
    } else {
      leftLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#08090c;">
          <div class="term-head"><strong style="font-size:12px;">L1 · ${esc(s.workspace)}</strong>${badgeHtml(s)}</div>
          <div class="term-screen" style="font-size:11px;">${renderTerminalLines(s)}</div>
        </div>
      `;
      rightLeaf = `
        <div style="flex:1;min-width:0;display:flex;flex-direction:column;background:#08090c;">
          <div class="term-head"><strong style="font-size:12px;">L2 · ${esc(s2.workspace)}</strong>${badgeHtml(s2)}</div>
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
                <strong style="font-size:12.5px;">Coda Duo 7.6"</strong>
                <span class="mono" style="font-size:10px;color:var(--green);">433 + 24 Crease + 349 + 84 Rail</span>
              </div>
              <div style="display:flex;gap:4px;">
                ${[
                  ["split", t("分栏", "Split")],
                  ["diff", t("差异", "Diff")],
                  ["dual", t("双窗", "Dual")],
                  ["canvas", t("全宽", "Canvas")],
                ]
                  .map(
                    ([m, label]) =>
                      `<button class="pill-btn ${state.duoSplitMode === m ? "primary" : ""}" style="padding:3px 8px;font-size:10.5px;" data-duo-mode="${m}">${label}</button>`
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
            <button class="duo-rail-btn ${state.duoSplitMode === "split" ? "active" : ""}" data-duo-mode="split">◧<span>${t("分栏", "Split")}</span></button>
            <button class="duo-rail-btn ${state.duoSplitMode === "diff" ? "active" : ""}" data-duo-mode="diff">±<span>${t("差异", "Diff")}</span></button>
            <button class="duo-rail-btn ${state.duoSplitMode === "dual" ? "active" : ""}" data-duo-mode="dual">▯▯<span>${t("双窗", "Dual")}</span></button>
            <button class="duo-rail-btn ${state.duoSplitMode === "canvas" ? "active" : ""}" data-duo-mode="canvas">⛶<span>${t("全宽", "Canvas")}</span></button>
            <div style="margin-top:auto;"><button class="icon-btn" data-open-modal="settings">⚙</button></div>
          </div>
        </div>
      </div>
    `;
  }

  // ===================== Modals =====================
  function openModal(kind) {
    const backdrop = document.getElementById("proto-modal-backdrop");
    const modal = document.getElementById("proto-modal");
    const s = activeSession();

    if (kind === "launcher") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>${t("启动远程 Agent / Shell 终端", "Launch Remote Agent / Shell")}</span>
          <button class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="display:grid;grid-template-columns:1fr 1fr;gap:8px;">
          ${[
            ["claude", "Claude Code", "Anthropic CLI · OSC 7501"],
            ["codex", "Codex CLI", "OpenAI Coding Agent"],
            ["gemini", "Gemini CLI", "Google Gemini Coding CLI"],
            ["shell", "Shell (Bash/Zsh)", t("原生 PTY / herdr-core 兼容", "Native PTY / herdr-core")],
          ]
            .map(
              ([id, name, desc]) => `
            <button class="host-card" style="flex-direction:column;align-items:flex-start;text-align:left;" data-launch-agent="${id}">
              <div class="host-card-title">${name}</div>
              <div class="host-card-sub">${desc}</div>
            </button>`
            )
            .join("")}
        </div>
      `;
    } else if (kind === "compose") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>${t("撰写多行指令 & Whisper 语音输入", "Compose Prompt & Whisper Dictation")}</span>
          <button class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="font-size:11.5px;color:var(--muted);">${esc(s.workspace)} · ${esc(s.agent)} · ${esc(s.hostName)}</div>
        <textarea id="modal-compose-ta" class="term-input" rows="5" placeholder="${t("输入长提示词，或点击下方语音按钮调用 Whisper 转写...", "Write a multiline prompt or tap Voice for Whisper transcription...")}">${esc(state.voiceDraft)}</textarea>
        <div style="display:flex;align-items:center;gap:8px;justify-content:space-between;">
          <div style="display:flex;gap:6px;">
            <button class="pill-btn" data-toast="${t("📎 已通过 E2EE upload_terminal_file 附加文件", "📎 File attached via E2EE")}">📎 ${t("上传文件", "Attach")}</button>
            <button class="mic-btn ${state.voiceRecording ? "recording" : ""}" data-toggle-voice>
              🎙 ${state.voiceRecording ? t("停止并转写", "Stop & Transcribe") : t("Whisper 语音", "Whisper Voice")}
            </button>
          </div>
          <button class="pill-btn primary" data-submit-compose>${t("发送至终端 ↑", "Send ↑")}</button>
        </div>
      `;
    } else if (kind === "diff") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>${esc(s.workspace)} · Git Diff</span>
          <button class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="max-height:58vh;overflow-y:auto;background:#08090c;border-radius:10px;border:1px solid var(--border);">${renderGitDiffPanel(s)}</div>
      `;
    } else if (kind === "palette") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>⌘K ${t("命令面板", "Command Palette")}</span>
          <button class="icon-btn" data-close-modal>✕</button>
        </div>
        <input class="term-input" placeholder="${t("搜索工作区、终端或操作...", "Jump to workspace, terminal, or command...")}" autofocus />
        <div style="display:flex;flex-direction:column;gap:6px;">
          ${state.sessions
            .map(
              (sess) => `
            <button class="sess-item" data-select-sid="${sess.id}" data-close-after="1">
              ${avatarHtml(sess, 24)}
              <div class="sess-meta"><div class="sess-title">${esc(sess.workspace)} · ${esc(sess.title)}</div><div class="sess-sub">${esc(sess.hostName)}</div></div>
              ${badgeHtml(sess)}
            </button>`
            )
            .join("")}
        </div>
      `;
    } else if (kind === "settings") {
      modal.innerHTML = `
        <div class="proto-modal-head">
          <span>⚙ ${t("全端设置与生命周期中心", "Settings & Account Lifecycle")}</span>
          <button class="icon-btn" data-close-modal>✕</button>
        </div>
        <div style="display:flex;gap:5px;flex-wrap:wrap;">
          ${[
            ["hosts", t("主机与配对", "Hosts & Pair")],
            ["security", t("密码与会话", "Security & Sessions")],
            ["voice", t("Whisper 语音", "Voice API")],
          ]
            .map(
              ([id, lbl]) =>
                `<button class="pill-btn ${state.settingsTab === id ? "primary" : ""}" data-settings-tab="${id}">${lbl}</button>`
            )
            .join("")}
        </div>
        ${
          state.settingsTab === "hosts"
            ? `
          <div class="host-card">
            <div>
              <div class="host-card-title">${t("允许远端控制本机（Coda.app Host 模式）", "Allow Remote Control (Host Mode)")}</div>
              <div class="host-card-sub">${t("关闭后仅作为控制端，不再接受远端接入", "Toggle whether this Mac can be controlled remotely")}</div>
            </div>
            <button class="pill-btn ${state.allowRemoteControl ? "green" : ""}" data-toggle-host-mode>${state.allowRemoteControl ? "ON" : "OFF"}</button>
          </div>
          ${state.hosts
            .map(
              (h) => `
            <div class="host-card">
              <div>
                <div class="host-card-title"><span class="dot ${h.online ? "online" : "offline"}" style="margin-right:6px;"></span>${esc(h.name)}</div>
                <div class="host-card-sub mono">${h.platform} · ${h.role}</div>
              </div>
              <div style="display:flex;gap:5px;">
                <button class="pill-btn" data-toast="${t(`已重命名主机 ${h.name}`, `Renamed ${h.name}`)}">${t("改名", "Rename")}</button>
                <button class="pill-btn danger" data-toast="${t(`已吊销并移除 ${h.name}（级联断开隧道）`, `Revoked & removed ${h.name}`)}">${t("移除", "Remove")}</button>
              </div>
            </div>`
            )
            .join("")}`
            : state.settingsTab === "security"
            ? `
          <div class="host-card" style="flex-direction:column;align-items:stretch;gap:8px;">
            <div class="host-card-title">${t("修改密码（自动轮换 Session Token）", "Change Password (Auto Token Rotation)")}</div>
            <input class="term-input" type="password" placeholder="${t("当前密码", "Current password")}" />
            <input class="term-input" type="password" placeholder="${t("新密码（至少 8 位）", "New password (min 8 chars)")}" />
            <button class="pill-btn primary" data-toast="${t("密码已更新并轮换新会话 Token", "Password updated & session rotated")}">${t("更新密码", "Update Password")}</button>
          </div>
          <div class="host-card">
            <div>
              <div class="host-card-title">${t("活跃登录会话 (3)", "Active Sessions (3)")}</div>
              <div class="host-card-sub">MacBook Pro · iPhone 16 Pro · Web Safari</div>
            </div>
            <button class="pill-btn danger" data-toast="${t("已注销其他全部设备会话", "Signed out all other sessions")}">${t("全部踢下线", "Logout All")}</button>
          </div>`
            : `
          <div class="host-card" style="flex-direction:column;align-items:stretch;gap:8px;">
            <div class="host-card-title">${t("Whisper 兼容语音接口（强制 HTTPS / localhost）", "Whisper-Compatible Voice Endpoint (HTTPS / localhost)")}</div>
            <input class="term-input" value="https://api.openai.com/v1" />
            <input class="term-input" value="gpt-4o-mini-transcribe" />
            <input class="term-input" type="password" value="sk-••••••••••••••••" />
            <button class="pill-btn sky" data-toast="${t("语音配置已安全保存至 Keychain / 本地存储", "Voice config saved securely")}">${t("保存并同步", "Save & Sync")}</button>
          </div>`
        }
      `;
    }

    backdrop.classList.remove("hidden");
  }

  function closeModal() {
    document.getElementById("proto-modal-backdrop").classList.add("hidden");
  }

  // ===================== Spec Annotations =====================
  const PLATFORM_SPECS = {
    web_desktop: {
      titleZh: "Web 桌面端控制台",
      titleEn: "Web Desktop Console",
      res: "≥ 900px · OLED #000000",
      pointsZh: [
        "<strong>三大页面态</strong>：免登录官网下载页 / RFC 8628 `/activate` 审批页 / 三栏 E2EE 终端工作台。",
        "<strong>直接原始键盘流</strong>：终端容器聚焦后直接向远端发 `pty_input` 与 Bracketed Paste，支持空回车、vim 与密码盲打。",
        "<strong>脏行 Diff 渲染</strong>：v0.6 升级行级脏区更新节点池，60fps 刷屏不打断鼠标拖选。",
        "<strong>多标签隔离</strong>：`sessionStorage` 独立标签页身份 + 10 分钟防重放缓存，多开不互踢。",
      ],
      pointsEn: [
        "<strong>3 Core States</strong>: Public Landing/Downloads, RFC 8628 `/activate`, and 3-pane E2EE Workbench.",
        "<strong>Direct PTY Input</strong>: Focused terminal streams raw keystrokes & bracketed paste directly via `pty_input`.",
        "<strong>Dirty-Row Diff</strong>: Row-level DOM node pool patching preserves text selection at 60fps.",
        "<strong>Tab Isolation</strong>: Per-tab `sessionStorage` identity + 10-min nonce replay cache.",
      ],
    },
    web_mobile: {
      titleZh: "Web 手机端 / Safari PWA",
      titleEn: "Web Mobile / Safari PWA",
      res: "< 900px · 100dvh + Safe-Area",
      pointsZh: [
        "<strong>抽屉侧边栏</strong>：窄屏默认折叠左侧栏，点击左上角 `☰` 以浮层抽屉 + 遮罩展开，不挤压终端视口。",
        "<strong>100dvh 适配</strong>：消除 iOS Safari 底部工具栏与地址栏动态缩放遮挡。",
        "<strong>一步直发 + 触控键盘</strong>：输入栏回车直发命令；展开双排终端键（`esc`/`ctrl`/`tab`/`⇧tab`/`↑`/`⌫`/`^C`）。",
        "<strong>HTTPS 诚实路由</strong>：自动过滤 Safari 拦截的 `ws://127.0.0.1`，走加密 WSS 通道。",
      ],
      pointsEn: [
        "<strong>Slide-over Drawer</strong>: Sidebar collapses below 900px into an overlay drawer + backdrop.",
        "<strong>100dvh Viewport</strong>: Prevents Safari toolbar occlusion with safe-area insets.",
        "<strong>1-Step Input & Keypad</strong>: Single-tap command submit + toggleable 2-row tactile keypad.",
        "<strong>Mixed-Content Safe</strong>: Filters blocked local `ws://` on HTTPS pages cleanly.",
      ],
    },
    mac_gpui: {
      titleZh: "macOS GPUI 桌面端 (Coda.app)",
      titleEn: "macOS GPUI Desktop (Coda.app)",
      res: "Metal 120Hz · Obsidian #0A0B0E",
      pointsZh: [
        "<strong>双向互控显式开关</strong>：Mesh 抽屉与设置页提供 `Allow Remote Control` 开关，随时切换 `hybrid`（被控+主控）与 `client`（仅主控）。",
        "<strong>独立守护进程共存</strong>：检测到独立 `coda-daemon` 运行时自动共享配置不抢端口，登出不误删 CLI 配置。",
        "<strong>Keychain 零明文</strong>：`Coda.app` 会话 Token 自动存入 macOS Keychain 并擦除磁盘 JSON 明文。",
        "<strong>原生终端体验</strong>：单元格鼠标拖选复制、`?2004h` Bracketed Paste、1200 行像素平滑滚轮、分屏 (`⌘D`) 与命令面板 (`⌘K`)。",
      ],
      pointsEn: [
        "<strong>Explicit Host Toggle</strong>: Switch `Allow Remote Control` (`hybrid` vs `client`) in Mesh Drawer & Settings.",
        "<strong>Daemon Coexistence</strong>: Detects standalone `coda-daemon`, avoids port conflicts, preserves CLI config on sign-out.",
        "<strong>Keychain Storage</strong>: Stores session token in macOS Keychain and scrubs plaintext JSON on disk.",
        "<strong>Native Terminal UX</strong>: Mouse cell drag selection, Bracketed Paste, 1200-line smooth scroll, split panes (`⌘D`), `⌘K` palette.",
      ],
    },
    ipad: {
      titleZh: "iPadOS 分栏工作台",
      titleEn: "iPadOS Split Workbench",
      res: "Regular Split · 288pt Sidebar",
      pointsZh: [
        "<strong>双栏自适应</strong>：288pt 可折叠侧边栏（⌘B）集成 Spaces 多分支 Worktree 树 + Inbox 优先级分组。",
        "<strong>原生低延迟输入</strong>：快捷栏与输入直走 `sendPtyInput`，零 3 秒延迟与双回车问题。",
        "<strong>真实 Whisper 语音</strong>：Compose 弹窗集成 `AVAudioRecorder` 16kHz AAC 录音 + `/audio/transcriptions` 转写。",
      ],
      pointsEn: [
        "<strong>Adaptive Split</strong>: Collapsible 288pt Sidebar (`⌘B`) combining multi-branch Spaces + priority Inbox.",
        "<strong>Low-Latency PTY</strong>: Quick dock & inputs route directly through `sendPtyInput`.",
        "<strong>Real Whisper Voice</strong>: `AVAudioRecorder` 16kHz AAC capture + Whisper transcription inside Compose sheet.",
      ],
    },
    iphone: {
      titleZh: "iPhone 原生 SwiftUI 客户端",
      titleEn: "iPhone Native SwiftUI",
      res: "390×844 · Dynamic Island",
      pointsZh: [
        "<strong>Inbox 优先工作流</strong>：按 `待确认 (Needs you)` → `异常` → `已完成(未读)` → `运行中` → `空闲` 强优先级排序，一眼处理阻塞 Agent。",
        "<strong>OSC 7501 审批横幅</strong>：进入待确认终端顶部自动浮现 `1 · Yes` / `2 · Always` / `Esc` 一键放行条。",
        "<strong>悬浮快捷条 + 双排触控键</strong>：拇指单手覆盖 `esc`/`tab`/`⇧tab`/`^C`，捏合手势防抖缩放字号。",
      ],
      pointsEn: [
        "<strong>Priority Inbox</strong>: Sorted strictly by `Needs you` → `Error` → `Done (unread)` → `Working` → `Idle`.",
        "<strong>OSC 7501 Banner</strong>: One-tap `1 · Yes` / `2 · Always` / `Esc` approval pills at top of terminal.",
        "<strong>Thumb-Friendly Dock</strong>: Floating quick keys + 2-row tactile keypad + debounced pinch font zoom.",
      ],
    },
    iphone_duo: {
      titleZh: "iPhone Duo 折叠屏三态工作台",
      titleEn: "iPhone Duo Foldable (3 Postures)",
      res: "466×678 ↔ 890×626 ↔ 626×890",
      pointsZh: [
        "<strong>5.4\" 外屏态 (466×678)</strong>：单手全屏终端 + 右侧 84pt 系统轨，58 列自动重排。",
        "<strong>7.6\" 展开双栏态 (890×626)</strong>：精确避让中央 24pt `.division` 物理折痕（`433 + 24 + 349 + 84`），支持 `分栏 / 差异 / 双窗 / 全宽` 4 种工作模式。",
        "<strong>笔电半折态 (Tabletop 626×890)</strong>：上半屏纯终端视口 + 水平 24pt 折痕 + 下半屏专属控制甲板（会话切换 + 审批条 + 输入框 + 完整双排键盘）。",
      ],
      pointsEn: [
        "<strong>5.4\" Cover (466×678)</strong>: One-handed 58-col terminal + 84pt trailing system rail.",
        "<strong>7.6\" Unfolded (890×626)</strong>: Displaces both leaves around the 24pt `.division` fold crease with 4 modes (`Split / Diff / Dual / Canvas`).",
        "<strong>Tabletop Laptop (626×890)</strong>: Upper ANSI viewport + horizontal 24pt fold crease + lower Tabletop Control Deck.",
      ],
    },
  };

  function renderSpecDrawer() {
    const spec = PLATFORM_SPECS[state.platform];
    const pts = state.lang === "zh" ? spec.pointsZh : spec.pointsEn;
    document.getElementById("spec-drawer-title").textContent = t("交互说明与设计标注", "UX Notes & Tokens");
    document.getElementById("spec-drawer-content").innerHTML = `
      <div class="spec-block">
        <h4>${esc(state.lang === "zh" ? spec.titleZh : spec.titleEn)}</h4>
        <div class="mono" style="font-size:11px;color:var(--green);margin-bottom:8px;">${esc(spec.res)}</div>
        <ul>${pts.map((p) => `<li>${p}</li>`).join("")}</ul>
      </div>
      <div class="spec-block">
        <h4>${t("共享设计 Token", "Shared Design Tokens")}</h4>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#000000;"></span>Web OLED</span><span>#000000</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#0a0b0e;"></span>GPUI Canvas</span><span>#0A0B0E</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#0b0c0f;"></span>Apple Canvas</span><span>#0B0C0F</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#f59e0b;"></span>Needs You</span><span>#F59E0B</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#38bdf8;"></span>Working</span><span>#38BDF8</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#22c55e;"></span>Direct / Diff+</span><span>#22C55E</span></div>
        <div class="spec-token-row"><span><span class="spec-swatch" style="background:#f85149;"></span>Error / Diff-</span><span>#F85149</span></div>
      </div>
      <div class="spec-block">
        <h4>${t("可交互演示提示", "Interactive Tips")}</h4>
        <ul>
          <li>${t("点击任意终端中的 <strong>1 · Yes</strong> 批准权限，切换其他设备标签可见状态全局联动。", "Click <strong>1 · Yes</strong> in any view; status syncs across all 6 device frames.")}</li>
          <li>${t("点击顶部 <strong>路径模拟</strong> 按钮可体验 LAN / IPv6 / 免备案 TCP / CDN WSS 切换。", "Click the top <strong>Route</strong> pill to cycle LAN / IPv6 / Raw TCP / CDN WSS.")}</li>
          <li>${t("点击 <strong>触发待确认</strong> 可重新弹出 OSC 7501 审批横幅。", "Click <strong>Trigger Needs-You</strong> to replay the OSC 7501 approval banner.")}</li>
        </ul>
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
      left = [
        ["workbench", t("终端工作台 /app", "Workbench /app")],
        ["landing", t("官网下载页 /", "Landing /")],
        ["activate", t("设备授权 /activate", "Activate /activate")],
      ]
        .map(([v, lbl]) => `<button class="sub-pill ${state.webSubView === v ? "active" : ""}" data-web-sub="${v}">${lbl}</button>`)
        .join("");
    } else if (state.platform === "iphone") {
      left = [
        ["list", t("收件箱 / 空间列表", "Inbox / Spaces")],
        ["terminal", t("全屏终端视图", "Full-Screen Terminal")],
      ]
        .map(([v, lbl]) => `<button class="sub-pill ${state.iphoneScreen === v ? "active" : ""}" data-iphone-screen="${v}">${lbl}</button>`)
        .join("");
    } else if (state.platform === "iphone_duo") {
      left = [
        ["cover", t("5.4\" 外屏单手态", "5.4\" Cover")],
        ["inner_landscape", t("7.6\" 展开双栏态", "7.6\" Unfolded Split")],
        ["tabletop", t("笔电半折态 (Tabletop)", "Tabletop Laptop")],
      ]
        .map(([v, lbl]) => `<button class="sub-pill ${state.duoPosture === v ? "active" : ""}" data-duo-posture="${v}">${lbl}</button>`)
        .join("");
    } else if (state.platform === "mac_gpui") {
      left = `
        <button class="sub-pill ${!state.macSplit ? "active" : ""}" data-mac-split="0">${t("单终端工作台", "Single Workbench")}</button>
        <button class="sub-pill ${state.macSplit ? "active" : ""}" data-mac-split="1">${t("主副双分屏 (⌘D)", "Split Panes (⌘D)")}</button>
        <button class="sub-pill" data-open-modal="palette">⌘K ${t("命令面板", "Palette")}</button>
        <button class="sub-pill" data-open-modal="settings">⚙ ${t("设置与改名/改密", "Settings")}</button>
      `;
    } else if (state.platform === "ipad") {
      left = `
        <button class="sub-pill ${state.ipadSidebar ? "active" : ""}" data-toggle-ipad-sidebar>${t("切换侧边栏 (⌘B)", "Toggle Sidebar (⌘B)")}</button>
        <button class="sub-pill" data-open-modal="diff">± ${t("查看 Git Diff", "Git Diff Sheet")}</button>
        <button class="sub-pill" data-open-modal="compose">🎙 ${t("撰写 / 语音弹窗", "Compose / Voice")}</button>
      `;
    } else if (state.platform === "web_mobile") {
      left = `
        <button class="sub-pill ${state.webMobileDrawer ? "active" : ""}" data-toggle-web-drawer>☰ ${t("抽屉侧边栏", "Slide Drawer")}</button>
        <button class="sub-pill ${state.webMobileKeypad ? "active" : ""}" data-toggle-web-keypad>⌨ ${t("双排触控键", "Touch Keypad")}</button>
      `;
    }
    const spec = PLATFORM_SPECS[state.platform];
    el.innerHTML = `<div class="subbar-group">${left}</div><div class="subbar-meta">${esc(spec.res)}</div>`;
  }

  function render() {
    document.getElementById("studio-sub").textContent = t(
      "v0.5.0 · 全端可交互高保真原型台",
      "v0.5.0 · Multi-Platform Interactive Prototype"
    );
    document.getElementById("device-tabs").innerHTML = DEVICE_LIST.map(
      ([id, icon, zh, en]) =>
        `<button class="device-tab ${state.platform === id ? "active" : ""}" data-platform="${id}">${icon} ${state.lang === "zh" ? zh : en}</button>`
    ).join("");

    const route = ROUTES[state.routeIdx];
    const routeBtn = document.getElementById("btn-route-sim");
    routeBtn.textContent = state.lang === "zh" ? route.labelZh : route.labelEn;
    routeBtn.className = `studio-chip route ${route.isRelay ? "relay" : ""}`;

    const needsBtn = document.getElementById("btn-trigger-needs");
    needsBtn.textContent = t("▲ 触发待确认", "▲ Trigger Needs-You");
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
  }

  // ===================== Event Delegation =====================
  document.addEventListener("click", (ev) => {
    const btn = ev.target.closest("[data-platform],[data-web-sub],[data-select-sid],[data-approve],[data-key],[data-open-modal],[data-close-modal],[data-toggle-voice],[data-toggle-host-mode],[data-mac-split],[data-mac-drawer],[data-toggle-ipad-sidebar],[data-iphone-back],[data-iphone-tab],[data-iphone-screen],[data-toggle-iphone-keypad],[data-duo-posture],[data-duo-mode],[data-toggle-web-drawer],[data-close-web-drawer],[data-toggle-web-keypad],[data-launch-agent],[data-settings-tab],[data-submit-compose],[data-toast]");
    if (!btn) {
      if (ev.target.id === "proto-modal-backdrop") closeModal();
      return;
    }

    if (btn.dataset.toast) showToast(btn.dataset.toast);
    if (btn.dataset.platform) { state.platform = btn.dataset.platform; render(); return; }
    if (btn.dataset.webSub) { state.webSubView = btn.dataset.webSub; render(); return; }
    if (btn.dataset.selectSid) {
      selectSession(btn.dataset.selectSid, btn.dataset.enterMobile === "1");
      if (btn.dataset.closeAfter === "1") closeModal();
      return;
    }
    if (btn.dataset.approve) { approveSession(btn.dataset.sid, btn.dataset.approve); return; }
    if (btn.dataset.key) {
      const k = btn.dataset.key;
      if (k === "enter") sendCommand("", btn.dataset.sid);
      else if (k === "^C") approveSession(btn.dataset.sid, "esc");
      else showToast(t(`已通过 E2EE pty_input 发送按键 [${k}]`, `Sent key [${k}] over E2EE pty_input`));
      return;
    }
    if (btn.dataset.openModal) { openModal(btn.dataset.openModal); return; }
    if (btn.dataset.closeModal !== undefined) { closeModal(); return; }
    if (btn.dataset.toggleVoice !== undefined) {
      toggleVoice();
      if (!document.getElementById("proto-modal-backdrop").classList.contains("hidden")) openModal("compose");
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
      if (!document.getElementById("proto-modal-backdrop").classList.contains("hidden")) openModal("settings");
      return;
    }
    if (btn.dataset.macSplit !== undefined) { state.macSplit = btn.dataset.macSplit === "1"; render(); return; }
    if (btn.dataset.macDrawer !== undefined) {
      state.macRightDrawer = btn.dataset.macDrawer === "none" ? null : (state.macRightDrawer === btn.dataset.macDrawer ? null : btn.dataset.macDrawer);
      render();
      return;
    }
    if (btn.dataset.toggleIpadSidebar !== undefined) { state.ipadSidebar = !state.ipadSidebar; render(); return; }
    if (btn.dataset.iphoneBack !== undefined) { state.iphoneScreen = "list"; render(); return; }
    if (btn.dataset.iphoneTab) { state.iphoneTab = btn.dataset.iphoneTab; render(); return; }
    if (btn.dataset.iphoneScreen) { state.iphoneScreen = btn.dataset.iphoneScreen; render(); return; }
    if (btn.dataset.toggleIphoneKeypad !== undefined) { state.iphoneKeypad = !state.iphoneKeypad; render(); return; }
    if (btn.dataset.duoPosture) { state.duoPosture = btn.dataset.duoPosture; render(); return; }
    if (btn.dataset.duoMode) { state.duoSplitMode = btn.dataset.duoMode; render(); return; }
    if (btn.dataset.toggleWebDrawer !== undefined) { state.webMobileDrawer = !state.webMobileDrawer; render(); return; }
    if (btn.dataset.closeWebDrawer !== undefined) { state.webMobileDrawer = false; render(); return; }
    if (btn.dataset.toggleWebKeypad !== undefined) { state.webMobileKeypad = !state.webMobileKeypad; render(); return; }
    if (btn.dataset.launchAgent) { launchNewSession(btn.dataset.launchAgent, state.hosts[0].id, "coda"); return; }
    if (btn.dataset.settingsTab) { state.settingsTab = btn.dataset.settingsTab; openModal("settings"); return; }
    if (btn.dataset.submitCompose !== undefined) {
      const ta = document.getElementById("modal-compose-ta");
      sendCommand(ta ? ta.value : state.voiceDraft);
      closeModal();
      return;
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
    showToast(t("已触发 OSC 7501 待确认提醒（全端同步高亮）", "Triggered OSC 7501 Needs-You event"));
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
