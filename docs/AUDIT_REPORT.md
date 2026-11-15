# Coda 全仓库架构、安全与逻辑审计报告及修复追踪清单

> **审计基准**：`main` 分支（v0.3.9）全量源码审阅
> **覆盖范围**：`coda-protocol`、`coda-apple-ffi`、`migrations`、`coda-server`、`coda-herdr`、`coda-daemon`、`coda-desktop`、`apple/` (Swift 6)、`web/` (TypeScript)、`deploy/` 与 `.github/workflows/`
> **执行原则**：先全量建档，随后按四端协议强同步约束逐一彻底修复并全量实测验证，现已全部修复并完成五套端到端验证。

---

## 状态总览

| 优先级 | 类别 | 当前状态 |
| :--- | :--- | :--- |
| **P0** | 密码学安全、身份认证绕过、注入、私钥暴露与高危并发互锁 | 已全部修复并验证 |
| **P1** | 生产可用性、数据库事务/连接池、防 DoS、沙箱边界与死锁 | 已全部修复并验证 |
| **P2** | 终端仿真准确性、跨端同步竞态、输入法与核心交互逻辑缺陷 | 已全部修复并验证 |
| **P3** | Docker/脚本完整性校验、密钥权限与 CI/CD 工程化缺陷 | 已全部修复并验证 |

---

## 一、加密协议与 FFI 安全边界 (`coda-protocol` / `coda-apple-ffi`)

### 1. [x] `[P0][CRYPTO-01]` 静态-静态 X25519 + 固定盐 + `seq = 1` 导致 ChaCha20-Poly1305 `(key, nonce)` 跨会话重用
- **涉及文件**：
  - `crates/coda-protocol/src/lib.rs#L127-L169`
  - `crates/coda-daemon/src/lib.rs`
  - `apple/Sources/CodaKit/CodaNetworkClient.swift`
  - `web/src/crypto.ts#L62-L93`
- **问题根因**：
  - `E2eeSession::derive`（及 Swift `CodaE2EESession`、Web `BrowserE2EESession`）直接对双方长期静态 X25519 密钥做 ECDH，并使用全局常量盐 `b"coda-zk-mesh-salt-v1"` 派生密钥；每个新会话均从 `next_tx_seq = 1` 开始且 `nonce = [dir_tag, 0, 0, 0, seq]`。
  - 每次进程重启、页面刷新、并发打开多个终端或跨网络重连都会用**完全相同的 `(key, nonce)` 加密不同明文**，在 AEAD 下直接泄露多帧明文 XOR 并允许构造 Poly1305 伪造标签（同时违背项目所宣称的 Forward Secrecy）。
- **修复方案**：
  - **Direct P2P 通道**：在 `DirectP2pHello` 与 `DirectP2pAck` 引入每次连接随机生成的临时 X25519 公钥（`client_ephemeral_x25519_pub_b64` / `daemon_ephemeral_x25519_pub_b64`），使用双向静态+临时 ECDH 拼接（`static_shared || eph_shared`）及临时公钥哈希盐，并结合双方设备 UUID 派生前向保密会话密钥，实现真 PFS 与单次密钥唯一性。
  - **Relay 通道**：引入 `RelaySessionAuth`，每次初始化中继会话由发起端生成 16 字节 CSPRNG 随机盐 `session_nonce_b64`（作为 HKDF 盐并写入 AEAD AAD + 绑定双端 UUID），且用发送端 `Ed25519` 私钥签名时间戳与会话盐。

### 2. [x] `[P0][CRYPTO-02]` Daemon Direct P2P (`:24444`) 接收自报公钥未校验账号白名单 & 客户端未验证 `DirectP2pAck` 身份
- **涉及文件**：
  - `crates/coda-daemon/src/lib.rs` (`handle_direct_p2p_connection`)
  - `crates/coda-protocol/src/lib.rs#L331-L345`
  - `apple/Sources/CodaKit/CodaNetworkClient.swift`
  - `web/src/app.ts#L920-L956`
- **问题根因**：
  - Daemon 处理 `DirectP2pHello` 时仅用帧内自报的 `client_ed25519_pub_b64` 验签，未核对该 `client_device_id` 与公钥是否属于同一账号；同一局域网下任意攻击者只需知道目标主机 `device_id` 即可冒充合法客户端建立 P2P 控制隧道。
  - 相反，客户端收到 `DirectP2pAck` 时 `coda-daemon` 未对其签名且客户端直接信任帧内返回的 `daemon_x25519_pub_b64`（Web 版优先取 `frame.daemon_x25519_pub_b64`），易被局域网 MITM 替换公钥截获全部操作。
- **修复方案**：
  - Daemon 维护来自控制面 `/api/v1/relay/trusted-peers` 与 `AuthAck.trusted_peers` 的可信同租户设备公钥表 `TrustedPeersStore`（并落盘缓存于 `daemon.json` 供离线 LAN 验签），严格校验接入设备 `id + ed25519_pubkey + x25519_pubkey` 且拒绝非本账号设备。
  - `DirectP2pAck` 增加 `daemon_ed25519_pub_b64`、`timestamp_ms`、`signature_b64`（覆盖双向临时公钥），全端客户端必须强校验与预期主机静态公钥匹配且签名有效。

### 3. [x] `[P0][CRYPTO-03]` Relay `RoutePacket` 缺乏认证且未校验发送方设备归属，允许无签名会话覆盖重置
- **涉及文件**：
  - `crates/coda-daemon/src/lib.rs`
  - `crates/coda-server/src/db.rs`
  - `web/src/app.ts#L803-L809`
- **问题根因**：
  - Daemon 收到未建会话或 `e2ee.seq == 1` 的中继 `RoutePacket` 时无条件按帧中自带 `sender_x25519_pub_b64` 替换旧会话，没有任何 Ed25519 签名或租户归属校验，恶意中继或伪造包可直接劫持会话或重放旧 `seq = 1` 帧。
- **修复方案**：
  - 全端为中继首包/未建会话包附加经 `Ed25519` 签名的 `RelaySessionAuth`（含 `session_nonce_b64` 与新鲜时间戳），Daemon 校验：1）`src_device_id` 存在于同账号 `TrustedPeersStore`；2）签名与公钥完全匹配；3）`session_nonce_b64` 未出现在最近 10 分钟已见防重放缓存中；4）严禁以未认证 `seq == 1` 覆写已有会话。

### 4. [x] `[P1][CRYPTO-04]` X25519 密钥协商未校验 `was_contributory()`，未防范低阶公钥
- **涉及文件**：`crates/coda-protocol/src/lib.rs#L137`, `apple/Sources/CodaKit/CodaCoreModels.swift`, `web/src/crypto.ts#L68`
- **修复方案**：在 Rust 检查 `shared.was_contributory()`，在 Swift/Web 显式检测全零共享密钥（`allSatisfy { $0 == 0 }`），全零直接拒绝协商。

### 5. [x] `[P1][CRYPTO-05]` 严格单调递增 `seq <= prev` 在 `ControlP0` 与 `BulkP1` 双通道并发下导致误杀丢包；Web 端零防重放
- **涉及文件**：`crates/coda-protocol/src/lib.rs#L201-L205`, `apple/Sources/CodaKit/CodaCoreModels.swift`, `web/src/crypto.ts#L95-L101`
- **修复方案**：三端统一实现基于 `u64` / `BigInt` 的 **64 位滑动防重放窗口 (`ReplayWindow`)**，并在 AEAD 认证解密成功**之后**更新窗口状态；同时支持合理窗口内乱序到达（如 `ControlP0` 超越 `BulkP1`），彻底拦截重放及伪造包污染窗口。

### 6. [x] `[P1][CRYPTO-06]` `zstd` 无限长度解压存在内存炸弹 (Zip Bomb / OOM)
- **涉及文件**：`crates/coda-protocol/src/lib.rs#L214-L232`, `web/src/crypto.ts#L99`
- **修复方案**：定义 `MAX_DECOMPRESSED_BYTES = MAX_WIRE_FRAME_BYTES * 2` (16 MiB)，Rust 改用 `Decoder` + `Read::take(MAX_DECOMPRESSED_BYTES + 1)` 限长读取，Web 校验帧头解压长度及输出缓冲上限。

### 7. [x] `[P2][CRYPTO-07]` 基于 `"swift-"` `req_id` 前缀协商禁用 zstd 脆弱且损坏 Swift 跨端兼容性
- **涉及文件**：`crates/coda-protocol/src/lib.rs#L221-L223`
- **修复方案**：在 `DirectP2pHello` 与 `RelaySessionAuth` 中引入显式能力布尔字段 `supports_zstd`（默认 `true`），Swift 客户端发送 `supports_zstd: false` 并在需要时走统一能力协商或安全的 Base64 字节 FFI 接口。

### 8. [x] `[P1][FFI-01]` `coda-apple-ffi` C-ABI 边界缺少 `catch_unwind` 且 `CStr` NUL 截断不安全
- **涉及文件**：`crates/coda-apple-ffi/src/lib.rs#L20-L123`
- **修复方案**：所有 `extern "C"` 函数统一包裹 `std::panic::catch_unwind`；新增基于 Base64 编解码、无 NUL 截断风险的 `coda_ffi_zstd_decompress_b64_to_b64`；补充完整单元测试确保 `cargo test -p coda-apple-ffi` 可独立运行验证。

---

## 二、控制面数据库、HTTP API 与 RelayHub (`migrations` / `coda-server`)

### 9. [x] `[P0][SVR-01]` `CODA_RELAY_SECRET` 默认硬编码秘密使全网部署可用公开 Secret 伪造任意租户中继票据
- **涉及文件**：`crates/coda-server/src/main.rs`, `crates/coda-server/src/lib.rs`, `deploy/docker-compose.yml`
- **修复方案**：生产非回环绑定下若 `CODA_RELAY_SECRET` 缺失或匹配已知默认占位串则直接拒绝启动；显式 `--dev` 或纯粹回环地址下则自动生成一次性 32 字节密码学随机密钥；同时从 `relay_secret` 确定性派生站点 Ed25519 `server_pubkey_b64` 避免重启漂移。

### 10. [x] `[P0][SVR-02]` `effective_public_base_url` 轻信非校验 `Host` / `X-Forwarded-Host` 导致 `/install.sh` 命令注入与中继 URL 投毒
- **涉及文件**：`crates/coda-server/src/lib.rs` (`effective_public_base_url`, `install_script_handler`, `public_site_info`)
- **修复方案**：对 `Host` / `X-Forwarded-Host` 仅取首段，严格校验仅含 `[A-Za-z0-9.\-:\[\]]`（严禁引号、`$`、反引号、换行、空格等任意字符），`X-Forwarded-Proto` 仅接受 `http`/`https`，无效值一律回退受控默认值。

### 11. [x] `[P0][SVR-03]` 认证与配对端点缺少限流，且密码校验阻塞 Tokio 异步线程并存在长度 DoS
- **涉及文件**：`crates/coda-server/src/lib.rs`, `crates/coda-server/src/db.rs`
- **修复方案**：增加滑动窗口限流器保护登录、注册、认领配对码与设备授权；限制密码字节长度（`6..=256`）并将 Argon2 运算移至 `tokio::task::spawn_blocking`；用户名不存在时执行等时虚拟校验消除账户枚举侧信道。

### 12. [x] `[P0][SVR-04]` `device_auth_requests` 明文落库保存 `issued_session_token` 且轮询未校验过期时间
- **涉及文件**：`migrations/001_init.sql#L102`, `crates/coda-server/src/db.rs` (`approve_device_auth`, `poll_device_auth`)
- **修复方案**：使用 `SHA256("coda-devcode-wrap:" || device_code)` 流密钥对数据库内待换取的 `issued_session_token` 做对称加扰（库表泄露也无法解密出 Bearer Token）；`poll_device_auth` 通过单条原子 `UPDATE ... WHERE status = 'approved' AND expires_at > NOW() RETURNING ...` 同步清空加密字段并校验有效期。

### 13. [x] `[P1][DB-01]` 单一共享 `tokio_postgres::Client` 无连接池、无断线重连且注册/邀请码/管理员竞态
- **涉及文件**：`crates/coda-server/src/db.rs`
- **问题根因**：单连接高并发串行排队，一旦 PostgreSQL 重启永久报错；`register_user_with_policy` 先查用户数与邀请码再插入，异常会烧掉邀请码，并发首注册可产出多管理员；`handle` 允许 `@` 导致与邮箱混淆；`pairing_tokens` 使用 `gen_range(100..999)` 少一位数；`devices.ed25519_pubkey` 全局 `UNIQUE` 跨租户抢注；`devices.platform` 缺少 `'windows'`。
- **修复方案**：实现具备健康检查与自动重连的多连接池；事务内加 `pg_advisory_xact_lock` + 原子 `UPDATE invite_codes ... RETURNING`；强制校验 `handle`（3~32 位字母数字下划线短横线，禁止 `@`）并按是否含 `@` 精确查询邮箱或用户名；修复配对码范围为 `100..=999` + 冲突重试；迁移约束改用租户内联合唯一索引 `idx_devices_user_ed25519` 并补充 `'windows'` 平台支持。

### 14. [x] `[P1][SVR-05]` 缺少登出注销与设备吊销 API、`/api/v1/health` 假绿灯及 CDN 错误缓存静态 404 回落页
- **涉及文件**：`crates/coda-server/src/lib.rs`, `crates/coda-server/src/db.rs`
- **修复方案**：新增 `POST /api/v1/auth/logout` 与 `DELETE /api/v1/devices/{id}`（实时断开中继并推事件）；`/api/v1/health` 真实执行数据库 `SELECT 1`；`cache_control_middleware` 校验证实非 `text/html` 再加 `immutable`；`/download/{artifact}` 回落直连文件名而非列表页；定期后台清理过期数据库记录；WebSocket URL 支持 Header/`Sec-WebSocket-Protocol` 传递 Bearer 并限制 `.max_message_size(MAX_WIRE_FRAME_BYTES)`。

### 15. [x] `[P0][RELAY-01]` `RelayHub` 转发同步 `.await` 触发 AB-BA 互锁/跨端队头阻塞、无认证前超时/帧限与双通道 HoL
- **涉及文件**：`crates/coda-server/src/relay.rs`
- **修复方案**：转发 `RoutePacket` 改用 `try_send` 非阻塞投递（`BulkP1` 满载即丢，慢消费者阻塞 `ControlP0` 则断开清理，杜绝跨客户端卡死）；认证前实施 5 秒握手超时、`MAX_AUTH_FRAME_BYTES = 16 KiB` 帧限制、90 秒空闲读超时；`TenantBucket` 修复精确令牌债务计算并仅对 `BulkP1` 节流，保障 `ControlP0` 零阻塞直通；`register_lane` 主动中止被取代的旧代次连接读任务并精确判定通道计数后再更新 `is_online`；校验 `AdvertiseEndpoints` 权限及长度、清理孤立桶与订阅。

---

## 三、本地终端桥与守护进程 (`coda-herdr` / `coda-daemon`)

### 16. [x] `[P1][HERDR-01]` `PtyBridgeHandle::write_raw` 短写丢字节、`dup` 缺少 `-1` 检查及 `FD_CLOEXEC` 缺失
- **涉及文件**：`crates/coda-herdr/src/lib.rs`
- **修复方案**：实现完整的阻塞/重试写循环直至全部字节落入 PTY；校验 `dup() >= 0` 后再调用 `File::from_raw_fd`，对 `master_fd`/`reader_fd` 显式设置 `FD_CLOEXEC` 并正确配置子进程 `setsid()` + `TIOCSCTTY`；`hc` 断连时立刻唤醒所有挂起 RPC 并支持重连；`ensure_or_install_hc_binary` 加进程级互斥锁并校验下载字节魔数。

### 17. [x] `[P1][DAEMON-01]` `in_rx.recv()` 单协程串行处理 RPC 导致本地 HTTP 预览或目录扫描卡死全节点所有终端
- **涉及文件**：`crates/coda-daemon/src/lib.rs`
- **修复方案**：通过有界 `Semaphore` + `tokio::spawn` 并发分派 `handle_peer_request_full`，同时给 `HttpLocalPreviewRequest` 加上 10 秒超时、非特权端口检查与 4 MiB 流式读取上限；序列化回复超过 `MAX_WIRE_FRAME_BYTES` 时主动回传可读错误帧。

### 18. [x] `[P0][DAEMON-02]` `ListDirectory` / `ReadFilePreview` 允许穿越访问任意宿主敏感文件且整文件 `fs::read` 易 OOM
- **涉及文件**：`crates/coda-daemon/src/lib.rs`
- **修复方案**：在工作区/活跃终端根路径范围校验的基础上显式拦截 `~/.ssh`、`~/.gnupg`、`~/.aws`、`~/.kube`、`~/.config/coda`、`/etc` 等高敏目录；`ReadFilePreview` 改用 `tokio::fs::File::open` + `AsyncReadExt::take(max_bytes + 1)` 流式定长读取。

### 19. [x] `[P1][DAEMON-03]` 缺少单进程 `flock` 文件锁、`generation = 1` 与桌面内嵌宿主互相踢掉、WebSocket `Pong` 未 flush、`daemon.json` 明文 0644 写入
- **涉及文件**：`crates/coda-daemon/src/lib.rs`, `crates/coda-daemon/src/main.rs`
- **修复方案**：实现 `coda-daemon.lock` 非阻塞排他文件锁（桌面端据此可靠感知外部守护进程运行状态）；中继重连使用单调递增的毫秒级 `generation`；WS 循环正确 `flush` `Pong` 与 25s `KeepAlive`；`DaemonConfig::save` 采用 `0600` 临时文件 + 原子 `rename`；`coda-daemon login` 支持 `CODA_LOGIN_PASSWORD` 环境变量与 `--password-stdin`。

---

## 四、macOS GPUI 原生桌面端 (`coda-desktop`)

### 20. [x] `[P0][DESK-01]` GPUI 主渲染/UI 线程阻塞同步执行 `git`、`osascript` 与整文件读取，且 50Hz 帧率深克隆数十万行终端状态
- **涉及文件**：`crates/coda-desktop/src/lib.rs`, `crates/coda-desktop/src/git_inspector.rs`
- **修复方案**：所有 `compute_repo_snapshot`、`list_workspace_dir`、`read_workspace_file_preview`、`osascript` 目录选择器全部异步卸载到后台线程池执行并加定长/边界校验（`read` + `take`、`--end-of-options`、`-c core.quotePath=false` 修复中文路径）；终端行快照采用 `Arc` 与真实内容哈希 (`DefaultHasher`) 缓存，彻底消灭主线程卡顿与无效全量重建。

### 21. [x] `[P0][DESK-02]` `pty.rs` 在 `fork()` 后调用非异步信号安全 `putenv`、缺少 `FD_CLOEXEC`、关闭留僵尸进程且 `write()` 阻塞主线程
- **涉及文件**：`crates/coda-desktop/src/pty.rs`
- **修复方案**：在 `fork()` 前于父进程预构建环境数组并通过 `execve` 启动子进程；对相关 FD 设置 `FD_CLOEXEC`；关闭终端时向负进程组发送 `SIGHUP` 并在后台清理协程 `waitpid`；写入 PTY 采用非阻塞 + 短超时通道机制。

### 22. [x] `[P1][DESK-03]` 终端仿真解析、动态行列 Resize、快捷键与中文 IME 输入法多处残缺
- **涉及文件**：`crates/coda-desktop/src/pty.rs`, `crates/coda-desktop/src/lib.rs`
- **修复方案**：
  - `VtScreenBuffer` 支持 UTF-8 OSC 解码、`ESC (`/`ESC )` 字符集序列吞并、`osc_buf`/`csi_buf` 上限及东亚宽字符双列单元格占位对齐。
  - 根据真实视口宽高与字号动态计算并同步本地/远端 PTY `cols`/`rows`。
  - 修复 `Ctrl+C` 重复发送三次信号，补齐 `Ctrl+A..Z`/`Home`/`End`/`Delete`/`PageUp`/`PageDown` 按键映射，并为 `CodaGpuWorkspaceView` 实现 GPUI `EntityInputHandler` 原生支持中文/CJK 输入法选词上屏。

### 23. [x] `[P1][DESK-04]` 桌面端重启无限注册新设备、并发建连竞争、退出登录不重置嵌入式宿主与多处 UI 状态 Bug
- **涉及文件**：`crates/coda-desktop/src/lib.rs`, `crates/coda-desktop/src/config.rs`
- **修复方案**：持久化桌面客户端身份至 `~/.config/coda/desktop-client-identity.json` (`0600`)；配置与布局文件采用带锁 `0600` 原子替换并移除多线程 `set_var` UB；建连加按主机单飞锁与异步同步；退出登录彻底关闭/重置嵌入式宿主与远端缓冲；校验外部 `open` URL 白名单；修复分屏作用域、网格滚动、指令面板滚动定位、`/` 路径边界、`machine_id` 分组与自测隔离临时目录。

---

## 五、Apple Swift 6 全平台客户端 (`apple/`)

### 24. [x] `[P0][APPLE-01]` 设备私钥、Session Token 与 Voice API Key 明文存入 `UserDefaults` 且事件 WebSocket 通过 URL 查询串泄漏 Token
- **涉及文件**：`apple/Sources/CodaKit/CodaCoreModels.swift`, `apple/Sources/CodaKit/CodaNetworkClient.swift`
- **修复方案**：引入 `CodaSecretStore`（优先基于 `Security` 框架系统钥匙串，自动迁移并抹除历史 `UserDefaults` 明文，且在无钥匙串签名 CLI 单测下支持 `0600` 私密目录受控回退）；`/api/v1/events/ws` 移除 `?token=` 查询参数，仅通过 `Authorization: Bearer` 与 `Sec-WebSocket-Protocol: coda-auth, coda-bearer.<token>` 传输鉴权凭证。

### 25. [x] `[P1][APPLE-02]` `GhosttyReflowEngine` 退化为无状态正则剥离，导致 `\r` 进度条刷屏、ANSI 光标定位失效、UTF-8 中文跨包截断乱码
- **涉及文件**：`apple/Sources/CodaGhosttyKit/GhosttySurfaceEngine.swift`
- **修复方案**：重写为真正的二维有状态 VT100/ANSI 屏幕网格+回滚缓冲+主/副屏切换模拟器（支持 `\r` 原地覆写、`CSI H/J/K/A/B/C/D/G`、持久 SGR 真彩、宽字符双列与 reflow 重排），内置 `utf8Carryover: Data` 自动衔接被 TCP/WS 分片截断的多字节中文字符。

### 26. [x] `[P1][APPLE-03]` 8 秒定时明文覆盖彩色 PTY 缓冲区、断线未清空已附加集合、全盘扫描过频与状态清理遗漏
- **涉及文件**：`apple/Sources/CodaUI/CodaWorkspaceStore.swift`, `apple/Sources/CodaKit/CodaNetworkClient.swift`, `apple/Sources/CodaUI/*.swift`, `apple/Sources/CodaMacApp/CodaMacApp.swift`
- **修复方案**：停止每 8 秒用纯文本覆盖实时 ANSI 屏幕缓冲（仅未收到流时首帧兜底），断连时清空 `attachedSessionIDs` 自动重连附加、切换时发送 `pty_detach`；节流 `listWorkspaces`；修复 `updateServerURL`/`signOut` 状态残余；实现 `setVoiceConfig` RPC；移出 `probeLanUDP` 出 Actor 并设 220ms 总截止；修复双分屏 `.sheet` 重复挂载、捏合字号缩放触发 `PtyResize`、MenuBar 状态真实绑定及自测 UDP 句柄泄漏。

---

## 六、Web 在线控制台 (`web/`)

### 27. [x] `[P0][WEB-01]` 多处直接拼接未转义字段进入 `innerHTML` 构成存储/反射型 XSS，且无 CSP 策略
- **涉及文件**：`web/src/app.ts`, `web/index.html`
- **修复方案**：`escapeHtml` 完整转义 `& < > " '`，对所有用户输入、账号邮件/昵称、设备名、路径、授权码、配对命令与下载链接实施转义与协议白名单校验；将前置主题初始化逻辑移至主脚本首行并删除 `index.html` 内联 `<script>`，配置严谨的 `Content-Security-Policy` Meta 头；清理 `build.ts` 中多余无哈希 `dist/app.js`。

### 28. [x] `[P1][WEB-02]` 每次 PTY 输出即整页重构 `root.innerHTML`，毁灭焦点、中文输入法选词与弹窗，且 WebSocket 无重连/异常处理
- **涉及文件**：`web/src/app.ts`, `web/src/crypto.ts`, `web/src/vt.ts`
- **修复方案**：
  - 实现轻量二维浏览器终端状态机 `web/src/vt.ts`（含 `TextDecoder` 流式 UTF-8 跨分片解码与安全转义 HTML 行渲染），`pty_output_chunk` 仅通过 `rAF` 增量更新 `#term-box` 视口，完整保留输入焦点、IME 组合状态、滚动位置与打开的设置面板。
  - 真实渲染 `this.repos` 并清除控制台写死的假指标；按真实尺寸计算 `cols`/`rows` 并防抖 `resize`；切终端发 `pty_detach`；校验 8 MiB 上传上限；`toB64` 分块编码；WebSocket 加心跳、异常捕获、`onclose` 指数退避自动重连及 `signOut()` 注销清场。

---

## 七、部署与 CI/CD (`deploy/` / `.github/workflows/`)

### 29. [x] `[P1][DEPLOY-01]` `deploy/Dockerfile` 未拷贝 `deploy/` 与 `web/build.ts` 导致无法直接构建且默认以 root 运行
- **涉及文件**：`.dockerignore`, `deploy/Dockerfile`, `deploy/docker-compose.yml`, `deploy/.env.example`
- **修复方案**：新增 `.dockerignore`；`deploy/Dockerfile` 在 `web-builder` 补齐 `assets/`、`web/bun.lock`、`web/build.ts` 与 `bun install --frozen-lockfile && bun run build`，`rust-builder` 升级 `rust:bookworm` 并拷贝 `deploy/install-daemon.sh` 以满足 `coda-server` 编译期 `include_str!` 依赖，最终阶段创建并切换非特权用户 `coda`（UID `10001`）；`docker-compose.yml` 强制要求设置 `POSTGRES_PASSWORD` 与 `CODA_RELAY_SECRET`（缺少时 fail-closed），并同 `.env.example` 一并默认收紧 `CODA_OPEN_REGISTRATION=false`。

### 30. [x] `[P1][DEPLOY-02]` 安装运维脚本无 SHA256 校验、`umask` / `.env` 不安全、`--password` 暴露于 `ps` 且 `install-daemon.sh` `WorkingDirectory` 与 plist 缺失参数
- **涉及文件**：`deploy/coda.sh`, `deploy/install-server.sh`, `deploy/install-relay-cn.sh`, `deploy/install-daemon.sh`
- **修复方案**：下载包统一拉取 `SHA256SUMS-${VERSION}.txt`/`SHA256SUMS.txt` 校验并增加缺失/示例占位密钥 fail-closed 检查；强制 `umask 077` 与安全引号转义写入 `.env`，用纯 POSIX `load_env_file` 逐行解析替换 `. ./.env` 执行；`install-daemon.sh` 通过 stdin (`--password-stdin`) 并清除 `CODA_LOGIN_PASSWORD` 传递给 `coda-daemon login` 消除进程命令行泄露，修正并加引号约束 systemd `WorkingDirectory`/`ExecStart`，补全 LaunchAgent `--p2p-bind` 与 `xml_escape`，仅在 `CODA_ALLOW_LOCAL_TARGET=1` 时捡拾 CWD 下 `./target` 二进制。

### 31. [x] `[P2][CI-01]` `ci.yml` 遗漏 FFI 与镜像构建检查；`release.yml` 无测试门禁、权限过宽且预发布 Tag 会覆盖 `:latest`
- **涉及文件**：`.github/workflows/ci.yml`, `.github/workflows/release.yml`
- **修复方案**：`ci.yml` 增加 `coda-apple-ffi`、`bun test`、缺失密钥 Compose fail-closed 断言与 `docker build` 非 root 校验；`release.yml` 收敛顶层为 `contents: read` 并按 Job 最小授权、增加 `bun test` / Rust 核心测试 / Apple `swift test` + `CodaSelfTest` / GPUI `--self-test` 门禁、非 root 打包运行镜像、附带 `libcoda_apple_ffi.dylib`，并为带 `-` 后缀的预发布 Tag（如 `-rc`/`-beta`/`-alpha`）自动标记 `prerelease: true` 且跳过覆盖 `:latest` 镜像标签。

---

## 全量验证记录（已全部通过）

- [x] **Suite 1 (Rust Workspace)**：`DATABASE_URL=postgres://localhost/coda_test TEST_DATABASE_URL=postgres://localhost/coda_test cargo test --workspace` —— **31 passed, 0 failed**（`coda-apple-ffi`: 2、`coda-daemon`: 3、`coda-desktop`: 9、`coda-herdr`: 3、`coda-protocol`: 5、`coda-server` 单元测试 3+2 及集成测试 `e2e_multi_tenant_mesh`: 3、`e2e_realtime_herdr_mesh`: 1）。
- [x] **Suite 2 (GPUI Desktop Self-Test)**：`cargo run -p coda-desktop -- --self-test` —— 通过（`[OK] coda-desktop self-test passed: pty=live, git_repo=true, branch=main, diff_len=5000, layout=workbench`）。
- [x] **Suite 3 (Apple FFI + Swift 6 Native Self-Test)**：`cargo build --release -p coda-apple-ffi && cd apple && swift test && swift build -c release && ./.build/release/CodaSelfTest` —— Release 构建与原生自检全部通过（`[OK] ALL CODA APPLE NATIVE SELF-TESTS PASSED!`；本机 Swift 6 CommandLineTools 环境不含 Xcode `XCTest.framework`，故 `#if canImport(XCTest)` 用例在 `swift test` 下运行 0 条，核心加密/FFI/SecretStore/VT 行为均已在 `CodaSelfTest` 实测通过）。
- [x] **Suite 4 (Web Console Tests + Build)**：`cd web && bun test && bun run build` —— `bun test` **10 pass, 0 fail**（49 expect() calls）；`bun run build` 产出带哈希 `dist/app.0c5857cc7f.js`、`dist/app.js` 已删除、`dist/index.html` 仅 1 处外链 `<script>` 且包含 `<meta http-equiv="Content-Security-Policy" ... script-src 'self' ... object-src 'none' />`。
- [x] **Suite 5 (Deploy Shell / Compose / Non-Root Docker Image)**：
  - `deploy/*.sh` 全部通过 `sh -n` 语法检查；
  - 缺省密钥时 `docker compose -f deploy/docker-compose.yml config` 按预期失败退出，注入密钥后默认与 `--profile relay-cn` 均校验通过；
  - `docker build -f deploy/Dockerfile -t coda-verify:local .` 构建成功，`docker run --rm --entrypoint id coda-verify:local -u` 返回 `10001`（非 root），随后已执行 `docker rmi coda-verify:local` 清理验证镜像。
