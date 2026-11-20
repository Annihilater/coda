# Coda `v0.4.0` 二次全栈深度审计与修复追踪清单

> **审计基准**：`v0.4.0`（`d6d98d8`）全量源码审阅  
> **覆盖范围**：`coda-protocol`、`coda-apple-ffi`、`migrations`、`coda-server`、`coda-herdr`、`coda-daemon`、`coda-desktop`、`apple/`、`web/`、`deploy/`、`.github/workflows/`  
> **修复进度**：**48 / 48 全部修复完成并通过全栈 5 套自动化验证**  
> **产品决策确认**：
> 1. 补齐 Web 与 Apple 客户端真实语音录音 + Whisper 兼容转写链路（#42）；
> 2. macOS `Coda.app` 支持双向被控，并在客户端提供显式开关控制是否允许本机被远端控制（#8 / #31）；
> 3. `coda-daemon` 支持与主控制面及国内独立 `--relay-only` 节点多路并发保活，确保在线状态与 Web/移动端极速丝滑直通（#27）。

---

## 一、安全漏洞与信任边界缺陷（12 项）

- [x] **#1 `[Critical]` 同账号 `role = "daemon"` 权限越界与横向移动**：限制 `daemon` 角色设备码登录仅签发受控主机凭据（或非控制端作用域），在 `RelayHub` 与 `coda-daemon` 强制校验发起控制/直连请求的对端 `role != "daemon"`。
- [x] **#2 `[Critical]` `.env.example` 占位 `CODA_RELAY_SECRET` 绕过服务端检查**：在 `coda-server/src/main.rs` 拦截所有含 `change_me`/`change-me` 子串及 `.env.example` 示例串的弱密钥；同步修正 `parse_dotenv_line` 引号转义解析。
- [x] **#3 `[High]` 设备吊销不彻底**：`auth_sessions` 关联 `device_id` 并在删除设备时级联清理会话；`RelayHub` 绑定校验与入站帧循环校验设备未吊销/槽位未被取代；关闭 `/v1/tunnel/ws` 时完整终止读写双向；Direct P2P 接入 `TrustedPeersStore` 实时吊销驱逐通知。
- [x] **#4 `[High]` Cloudflare/反向代理下限流坍缩为单桶 `127.0.0.1` 与内存/Argon2 DoS**：新增 `CODA_TRUST_PROXY_HEADERS`（及默认信任回环/私网代理 `CF-Connecting-IP`/`X-Real-IP`）；给 `RateLimiter` 增加硬容量上限与淘汰；注册先验 `open_registration`/邀请码再跑 Argon2id；对 `device/info`、`pair-token` 及 WS 升级加限流，提升配对码/邀请码熵与冲突重试。
- [x] **#5 `[High]` 未认证前 8 MiB 帧分配 & 2 MiB `endpoints_json` 落库膨胀**：在 `:24443`、`/v1/tunnel/ws`、`/api/v1/events/ws` 与 `:24444` 认证前强制 `MAX_AUTH_FRAME_BYTES`（16 KiB）上限；HTTP 注册/配对接口严格校验 `endpoints` 数组长度与元素大小。
- [x] **#6 `[High]` `CORS: Allow-Origin *`、WS 无 `Origin` 校验与 `X-Forwarded-Host` 投毒**：收敛 CORS 与 WS `Origin` 校验（仅允许同源、`CODA_PUBLIC_URL`、显式白名单或非浏览器原生客户端）；未信任代理时忽略外部 `X-Forwarded-Host`；`/api/v1/events/ws` 监听登出/会话吊销立即断开。
- [x] **#7 `[High]` Apple Direct P2P 空公钥绕过钉扎 & 相对 `cwd` `dlopen` 劫持**：空 `expectedDaemonEd25519Pubkey`/`expectedDaemonX25519Pubkey` 直接 Fail-Closed 拒绝；`CodaFFILoader` 仅在测试/显式环境变量下读取开发路径，移除任意 `cwd` 相对路径加载与 `#filePath` 泄露。
- [x] **#8 `[High]` `Coda.app` 静默开启 `0.0.0.0:24444` 被控监听 & `~/.config/coda` 权限/凭据保护**：新增“允许远端控制本机（Host 模式）”开关与状态指示；确保 `~/.config/coda` 目录强制 `0700`；`Coda.app` 私钥与 Token 优先使用 macOS Keychain（CLI/测试回落 `0600`），同步更新 `is_sensitive_path` 覆盖 `desktop_client_identity.json`。
- [x] **#9 `[High]` 运行时无校验 `curl | sh` 自动安装 `herdr-core`**：`install-daemon.sh` 支持 `CODA_SKIP_HC_INSTALL=1` 与版本/校验控制，运行时安装增加明确日志/受控开关并优先使用校验或已安装二进制，避免静默无条件执行远程脚本。
- [x] **#10 `[High]` 受控端文件/命令沙箱补盲**：`UploadTerminalFile` 严格清洗 `filename`（禁止 `..`/`/`/隐藏配置覆盖并校验大小）；`is_sensitive_path` 补充拦截 `.claude`、`.codex`、`.gemini`、`.config/gh`、`.cargo/credentials*`、`*.key`、`.zsh_history`/`.bash_history`、`terraform.tfstate` 与 `herdr` 语音密钥目录；`ListWorkspaces` 使用 `-c core.fsmonitor=false`；`HttpLocalPreview` 限制敏感内部管理端口与响应头；`WorkspacesSnapshot` 不明文广播 `voice_config.api_key`。
- [x] **#11 `[Medium]` `coda-apple-ffi` 并发 `&mut` UB、`supports_zstd` 纳入签名、Web 防重放缓存与协议校验对称**：给 `CodaFfiSession` 加内部 `Mutex`；将 `supports_zstd` 纳入 `DirectP2pHello` 与 `RelaySessionAuth` Ed25519 签名（并向后兼容校验）；Web `openRelayPacket` 增加 10 分钟 `seenRelayNonces` 防重放缓存并统一 `MAX_HANDSHAKE_SKEW_MS = 120_000`；`E2eeSession::open_message` 强校验 `session_nonce_b64` 与明文长度上限并清零密钥；校验 `AdvertiseEndpoints` 地址格式防 SSRF。
- [x] **#12 `[Medium]` 裸机 `systemd` 非 root 运行与 `sudo install-daemon.sh` 权限防护**：`coda-server.service` / `coda-relay-cn.service` 与安装脚本创建专用系统用户 `coda` 并开启基础 systemd 沙箱；`install-daemon.sh` 防止 `sudo` 污染普通用户 `$HOME` 所有权。

---

## 二、核心运行时 Bug 与崩溃/断连故障（14 项）

- [x] **#13 `[Critical]` GPUI 副屏缩放退出 Panic 闪退**：`VtScreenBuffer::resize` 同步调整 `alt_saved.grid`，`leave_alt_screen` 兜底对齐当前 `rows`/`cols`，彻底消除 `index out of bounds`。
- [x] **#14 `[Critical]` `coda-daemon` 重启或中继重连后 Web & Apple E2EE 永久失联**：受控端遇到未知会话/缺少 `session_auth` 时向对端回传结构化 `RelayWireFrame::SessionReset`；Web 与 Apple 收到 `session_reset` 或 `peer_presence(online=true)` 时自动清空旧 E2EE 会话、重新签名握手并重新 `pty_attach`。
- [x] **#15 `[Critical]` Web & Apple VT 解析器误执行 `CSI >1u` / `CSI >4;2m`**：严格区分 CSI 私有前缀（`?`, `>`, `<`, `=`）与中间字节（`0x20..0x2F`），阻止 Kitty 键盘协议与 `modifyOtherKeys` 误触发 `SCORC` 光标跳回 `(0,0)` 与全局下划线/暗淡样式。
- [x] **#16 `[Critical]` Finder 启动 `Coda.app` 找不到 `claude`/`codex` 且 LaunchServices 参数致退出**：`HerdrClient` 与 `coda-daemon` 注入完整增强 `PATH`（含 `~/.nvm/versions/node/*/bin`、`volta`、`fnm`、`pnpm`、`mise`、`asdf`、`nix` 及 `GIT_TERMINAL_PROMPT=0`）；`coda-desktop/src/main.rs` 过滤 macOS `-psn_*` / `-NS*` / `-Apple*` 系统启动参数。
- [x] **#17 `[Critical]` GPUI 远端事件阻塞终端流丢包 & 行高未锁死挤出底部光标**：拆分 `HerdrLiveEvent` 同步至独立防抖后台任务不阻塞 `PtyOutputChunk`，`RecvError::Lagged` 时自动触发 `ReadTerminalScreen` 重同步；终端每行锁定 `.h(px(line_h)).overflow_hidden().whitespace_nowrap()`。
- [x] **#18 `[High]` 三端 VT 终端引擎关键序列修复**：
  - GPUI `pty.rs`：修复 `Ctrl+L` 清屏保留旧 scrollback、补齐 `CSI E`/`CSI F`、冒号 SGR `38:2:...`、`DCS`/`APC` 吞并、`DECSTBM` 光标归位、`?25l` 隐藏光标、宽字符 `CSI P`/`@`、修复远端 `Ctrl+C` 双重中断；
  - Web `vt.ts`：修复 `38:2::R:G:B` 多冒号解析、`DECSTBM` `cursorRow === scrollBottom` 与 `IL`/`DL` 边界、`?1049h/l` 滚动区/保存光标隔离、补充 `DCS`/`APC` 吞并、组合字符 0 宽度与光标渲染；
  - Apple `GhosttySurfaceEngine.swift`：补齐 `bg` 背景色与反显 `SGR 7`、修复主屏 `treatLFAsCRLF` 破坏 Raw TUI、支持 JSON `OSC 7501`、避免 Markdown `- item` 误标红、缓存预编译正则并降低重建开销。
- [x] **#19 `[High]` 浏览器多标签抢线风暴 & `generation` 回退 120s 黑洞**：Web 使用按标签页独立的 `sessionStorage` 临时客户端身份不互踢（同时 `coda-daemon` 按 `(src_device_id, session_nonce_b64)` 隔离会话槽位）；`RelayHub::register_lane` 在 `AuthAck` 前校验代次并在客户端重启时允许带新鲜凭据的新连接平滑接管（Apple 客户端 `currentGeneration` 改为毫秒级单调时间戳）。
- [x] **#20 `[High]` 大陆原生 IPv6 直连默认仅监听 IPv4**：支持双栈 `[::]:24444`（`IPV6_V6ONLY = false`，若内核不支持 IPv6 自动回退 `0.0.0.0:24444`）同时接听 IPv4 与公网 IPv6 直连。
- [x] **#21 `[High]` iOS 备份恢复 Keychain / `UserDefaults` 不一致锁定**：`ensureClientDeviceRegistered` 与 `refreshMachinesAndSessions` 核对本机 `ed25519Pubkey`/`x25519Pubkey` 与服务端是否一致，不匹配则自动重新注册新 `deviceId`，并在设置页提供重置本地密钥选项。
- [x] **#22 `[High]` 缺失 `hc` 或 `/usr/bin/hc` 冲突导致 `coda-daemon` 终端全挂**：`discover_hc_binary` 验证 `--version` 避开 `herbstclient`；在 `coda-daemon` 中内置原生 Unix `openpty` Shell 兜底会话管理（当 `hc` 不可用时仍可启动 Shell、收发实时 PTY 流、调整尺寸与关闭会话），且 Direct P2P 延迟动态获取 `herdr` 不永久缓存 `None`。
- [x] **#23 `[High]` 裸机升级 `ETXTBSY` 崩溃 & `install-daemon.sh` 跳过旧版升级**：所有裸机安装脚本使用临时文件 + 原子 `mv` 替换二进制，避免 `Text file busy`；`install-daemon.sh` 自动比对版本并覆盖升级已有 `coda-daemon`。
- [x] **#24 `[High]` Apple 客户端中继心跳与断线自动重连**：为 `connectOverseasCDNWSS` 与 `connectDomesticRawTCPAsync` 增加 25 秒 `keep_alive` 心跳保活，清理 `CodaWorkspaceStore` Actor 竞态（`preparedClient()` 等待 token/handler 就绪、`pendingRefresh` 合并防丢刷新、`signOut` 安全清理）。
- [x] **#25 `[Medium]` `coda-herdr` / `coda-daemon` 阻塞写 PTY、`reader_fd` 泄漏、事件断流与 UTF-8 截断**：`master_fd` 设为 `O_NONBLOCK` 并在异步任务中安全写入；关闭 PTY 时通过 stop-pipe 立即退出读线程与 `hc attach`；`spawn_herdr_event_forwarder` 加入断线自动重连；修复按字节硬截断多字节 UTF-8 与 `PtyInput` 预附加回退误发回车。
- [x] **#26 `[Medium]` 数据库真连接池 + TLS 支持、`RelayHub` 非阻塞限流与 Presence 竞态修复**：`Db` 升级为真正多连接并发读写池（自动健康重连）并支持 `sslmode`（`rustls` TLS）；`RelayHub` 移除单连接读循环内 `BulkP1` 阻塞 `sleep`、将 `broadcast_presence_to_tenant` 纳入 `online_sync_lock` 串行、`LivePeerSession` 遇到 `peer_backpressure` 不致命断开；测试数据库锁升级为 `pg_advisory_lock`。

---

## 三、架构与状态逻辑矛盾（8 项）

- [x] **#27 `[High]` 国内 `--relay-only` 节点与主控多路并发保活**：`coda-daemon` 同时维护与主控面及国内独立 `--relay-only` 中继的并发连接（共享加密会话与 PTY 表），确保 `devices.is_online` 始终准确、Web 端随时直通，移动端/桌面端自动走国内 `:24443` 低延迟通道。
- [x] **#28 `[High]` 30 天票据与会话自动续期**：服务端新增由设备 Ed25519 签名的 `POST /api/v1/relay/renew-ticket` 续签端点及 `auth_sessions` 滑动续期；`coda-daemon` 提前 7 天或在过期错误时自动续签并原子落盘 `daemon.json`，重连采用指数退避（1s → 30s）。
- [x] **#29 `[High]` 修复 `./update.sh` 锁死旧版本**：`coda.sh update` 自动探测最新 Release tag 并升级 `.env` 中固定的 `CODA_IMAGE`（支持 `--keep-tag` / `--latest`）；`--systemd` 模式调用 `install-server.sh` / `install-relay-cn.sh --version latest` 完成二进制更新；`release.yml` 不再锁死 `coda.sh`。
- [x] **#30 `[High]` 修复自建服务器 `CODA_PUBLIC_URL` 默认值陷阱**：`.env.example`、`coda.sh` 与 `install-server.sh` 默认留空 `CODA_PUBLIC_URL=`；修复内网无端口 HTTP 域名（如 `http://coda.lan`）被误强制改写成 `https://`。
- [x] **#31 `[High]` 解决 macOS `coda-daemon` 与 `Coda.app` 冲突与自连**：`Coda.app` 持久化复用本机身份并过滤本机 `device_id`；独立 `coda-daemon` 运行时平滑共存不抢端口；`Coda.app` 退出登录保留外部 `daemon.json`；`dev.coda.daemon.plist` 配置 `ThrottleInterval=30` 与 `SuccessfulExit=false`。
- [x] **#32 `[High]` 诚实呈现 Web 端连接路径并消除 Safari Mixed Content 报错**：`collectDirectWsCandidates` 在 `https://` 下过滤被 Safari 拦截的 `ws://127.0.0.1:24444`，并准确呈现 Web/原生客户端连接路径说明。
- [x] **#33 `[Medium]` 尊重 `CODA_RELEASE_VERSION` 固定版本、缓存 GitHub 探测失败与 `/install.sh` 仓库参数化**：显式设置 `CODA_RELEASE_VERSION` 时 `/download/*` 严格使用固定版本；未固定时给 GitHub 探测增加 5 分钟负缓存；`/install.sh` 动态注入 `CODA_GITHUB_REPO` 与 `CODA_RELEASE_VERSION`。
- [x] **#34 `[Medium]` 完善 CI 路径触发与 `coda-server` `SIGTERM` 优雅关闭**：`ci.yml` 将 `coda-daemon`/`coda-herdr` 加入 `apple_desktop`、`deploy/install-daemon.sh` 加入 `rust_core`；`release.yml` 纳入 `cargo test --release -p coda-server --lib`；`coda-server` 监听 `SIGINT` + `SIGTERM` 实现优雅停机。

---

## 四、用户使用与运维操作不便痛点（14 项）

- [x] **#35 `[Critical]` Web 终端支持直接键盘输入（`pty_input`）、空回车、修复手机 `tab`/`/`/`⌫` 与桌面键盘切回**：为 `#term-box` 增加聚焦与实时键盘捕获（直发 `pty_input` 与 bracketed paste，支持空回车）；手机 `#kb-form` 一步直发；修正 `tab`/`/`/`⌫` 按键映射；桌面开启触控键盘后提供切回输入框按钮。
- [x] **#36 `[High]` 修复 Web 手机端布局挤压与滚动/选区保持**：窄屏（`<900px`）默认折叠侧边栏并用抽屉浮层 + 遮罩展示；改用 `100dvh` 防止 Safari 底栏遮挡；终端刷新保留拖选状态与 `#term-box` 滚动位置；动态测算字体真实宽高与响应式断点重绘。
- [x] **#37 `[High]` 完善 Web 空主机操作、Agent/目录选择启动、终端关闭与 `/activate` 状态修复**：Web `+` 按钮支持弹出启动器选择 Agent（`Shell` / `Claude Code` / `Codex` / `Gemini CLI`）、工作区目录与初始提示词；支持关闭/杀死终端（`kill_terminal`）；修复 `/activate` 取消残留弹窗与错误/成功反馈颜色区分。
- [x] **#38 `[Critical]` GPUI 桌面端支持鼠标拖选复制、Bracketed Paste 与 `Edit` 菜单**：实现终端单元格级鼠标拖选高亮与智能 `Cmd+C` 复制；支持 `?2004h` Bracketed Paste；补齐 macOS `Edit` 菜单、`Cmd+=/-/0` 缩放、`Ctrl+L`、`DECCKM` 与 `DSR \x1b[6n]` 自动应答。
- [x] **#39 `[High]` 优化 GPUI 桌面端输入框、登录按钮与弹窗错误提示**：支持 `Cmd+A` 全选覆盖、密码粘贴自动去末尾换行、弹窗内部直接渲染错误与加载状态、优化 CJK IME 候选框与预编辑体验。
- [x] **#40 `[High]` 修复 GPUI 桌面端关闭窗口无法从 Dock 恢复与优雅退出**：注册 `Application::on_reopen` 回调与 `NewWindow` 菜单项；退出时干净释放本地/远端 PTY。
- [x] **#41 `[Medium]` 优化 GPUI 桌面端滚轮手感、`Changes`/`Files` 自动刷新、双行号修复与面板列表滚动**：引入像素累加平滑触控板滚动与副屏滚轮保护（扩大回滚查看上限至 1200 行）；打开抽屉自动刷新并增加手动刷新；修复本地文件预览重复行号、软链目录规范化与切换 `server_url` 状态清理。
- [x] **#42 `[Critical]` 补齐 Web & Apple 真实语音录音 + Whisper 转写功能**：Web 端实现 `MediaRecorder` 录音 + Whisper 兼容 API 转写（及浏览器 `SpeechRecognition` 回退）；Apple 端在 `project.yml` 添加 `INFOPLIST_KEY_NSMicrophoneUsageDescription`，新增 `CodaVoiceRecorder`（`AVAudioRecorder` + `multipart/form-data` 上传转写）并在 `CodaComposePromptSheet` 接入实时录音/转写按钮。
- [x] **#43 `[Critical]` 修复 Apple 启动 Shell 用 `"shell"` 替代 `"zsh"` 并补齐 iPhone Duo 启动菜单**：全部 `"zsh"` 改为 `"shell"`；为展开态 iPhone Duo 目录页与 iPad/Mac 空详情页补齐 `+` 启动菜单与多分支 worktree 展示。
- [x] **#44 `[High]` 修复 Apple 快捷键名/实时输入延迟/双回车、软键盘焦点与注册邀请码**：iOS 快捷栏与输入直走低延迟 `sendPtyInput`，修正 `herdrKeyNames` 小写键名；修复 `directKeystrokeStrip` 条件挂载 `@FocusState` 焦点丢失与捏合缩放防抖；补齐终端终止菜单、注册邀请码输入与 RFC 8628 主机审批入口；解析并展示 `CodaAPIError` 真实服务端错误。
- [x] **#45 `[Medium]` 补齐账号与设备全生命周期管理（含设备删除/重命名、密码修改、会话注销与删除账号）**：服务端新增 `PATCH /api/v1/devices/{id}`、`POST /api/v1/auth/password`、`GET/DELETE /api/v1/auth/sessions`、`POST /api/v1/auth/logout-all`、`DELETE /api/v1/auth/me` 与 `DELETE /api/v1/invites/{code}`（非管理员 `GET /api/v1/invites` 返回 `403`）；在 Web、GPUI、Apple 三端补齐对应管理 UI。
- [x] **#46 `[Medium]` 补齐 `coda-daemon` 运维子命令与智能 `status`**：新增 `coda-daemon stop` / `restart` / `uninstall` / `logout`；`login`/`pair` 默认复用已有密钥并发送 `SIGHUP` 热重载配置；`status` 实时探测锁/进程存活、`hc` 可用性与票据剩余有效期；修复 `TrustedPeersStore` token 失效回退 `X-Coda-Relay-Ticket`。
- [x] **#47 `[Medium]` 修复无 Git 仓库服务器与 `>=15` 终端下的工作区发现**：`scan_git_workspaces` 独立预留 `$HOME` 仓库扫描额度；无 Git 仓库主机回退使用 `$HOME` 真实路径；`install-daemon.sh` `WorkingDirectory` 默认设为 `$HOME`。
- [x] **#48 `[Low]` 清理 Web 双重转义链接、Compose 密码 URL 编码、`Coda.app` 打包精简与日志治理**：移除 `app.ts` 对 `safeHref()` 的重复 `escapeHtml` 并安全化 hash 滚动；`docker-compose.yml` + `coda-server/src/db.rs` 通过 `PGPASSWORD` 原生支持含任意特殊字符的数据库密码；`release.yml` 打包 `Coda.app` 移除冗余 `coda-daemon` 并补齐 `NSLocalNetworkUsageDescription`；`install-daemon.sh` 对 `coda-daemon.log` 强制 `0600` 权限与 10 MiB 截断上限。
