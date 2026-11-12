# CODA · Zero-Knowledge AI Agent Mesh

**Coda** 是一套支持**多用户好友隔离、端到端零知识加密（Zero-Knowledge E2EE）**的跨设备 AI 远程开发云网，专为编排远端 **`herdr-core` (`hc`)** 中的 **Claude Code、Codex、Grok Build、Gemini CLI** 而打造。

## 核心架构亮点

1. **多租户账号与零知识加密 (`crates/coda-protocol` & `migrations/001_init.sql`)**
   - PostgreSQL 18 驱动的多用户体系：首个注册用户自动成为管理员，可签发**好友邀请码 (`CODA-XXXXXX`)**。
   - 每台设备本地生成并持久化 `Ed25519`（身份签名）+ `X25519`（密钥协商）密钥对；所有会话帧采用 **`X25519 + HKDF-SHA256 + ChaCha20-Poly1305`** 端到端加密并自动对终端刷屏启用 **`zstd` 流式压缩**（服务端纯盲转发，无法解密任何用户数据）。
2. **大陆免备案裸 IP 与海外 CDN 混合中继引擎 (`crates/coda-server`)**
   - **国内阿里云/腾讯云免备案极速通道 (`--raw-tcp-bind 0.0.0.0:24443`)**：纯二进制长度前缀帧 + `TCP_NODELAY`，**零 HTTP 头、零域名 SNI**，彻底规避国内未备案域名 80/443 阻断；支持 `--relay-only` 无数据库独立边缘节点部署！
   - **海外服务器 + CDN 边缘通道 (`/v1/tunnel/ws`)**：双轨优先级调度（`ControlP0` 按键与审批零排队 vs `BulkP1` 令牌桶租户公平限速）+ **75 秒预热无缝换轨（Make-Before-Break）**，免疫 CDN 100 秒断连。
3. **原生对接 `herdr-core` 全双工 PTY 流与工作区扫描 (`crates/coda-herdr` & `crates/coda-daemon`)**
   - **全双工 PTY 实时流 (`hc attach`)**：基于 Unix `openpty` + `TIOCSWINSZ` 动态重排，实时双向流式传输 24-bit ANSI/VT 终端画面与 Kitty Keyboard 按键序列，并在断开时通过 `0x1c` (`Ctrl+\`) 无损分离。
   - **Git 多仓库/Worktree 扫描 (`ListWorkspaces`)**：自动扫描主机 Git 仓库、`git worktree list --porcelain` 与 `+added / -deleted` 行数统计，并与运行中的 `herdr` 终端自动关联。
   - **远端文件/截图直传 (`[ 📎 ]` -> `hc terminal.put`)** 与 **语音模型配置持久化 (`[ 🎙 ]` -> `hc doc.set`)**。
4. **Phase 3 局域网 IPv4 / 大陆原生公网 IPv6 直连打洞引擎 (`DirectP2pClient` & `spawn_direct_p2p_listener`)**
   - 250ms Happy Eyeballs 竞速探测：同局域网（`< 2ms`）或三大运营商原生公网 IPv6（`2408:`/`2409:`/`240e:`，`< 12ms`）自动建立端到端加密点对点直连，**零服务器中继带宽消耗**；不通时毫秒级无感回退至 `:24443` 免备案裸 TCP 或 `:443` CDN WSS。
5. **三大客户端矩阵：Rust + GPUI 极速桌面端 + 纯血 Apple 移动端 + 零安装 Web 端 (`crates/coda-desktop` & `apple/` & `web/`)**
   - **`crates/coda-desktop` (Rust + GPUI 0.2.2)**：采用 Zed 同款 **Rust + GPUI** 纯 GPU 加速框架（macOS `Metal`、Windows `DirectX 12`、Linux `Vulkan`/`Blade`），**零 FFI 跨语言损耗**直接复用 `coda-protocol`、`coda-herdr` 与 `coda-daemon`（含 Direct P2P 直连引擎），一套 Rust 代码同时覆盖 **macOS、Windows、Linux 桌面端**，实现 120Hz/240Hz 丝滑 OLED 终端工作台。
   - **`apple/` (Swift 6 / SwiftUI Multiplatform)**：专为 **iPhone、iPhone Duo（5.4" 外屏单手终端态 ↔ 7.6" 展开双栏工作台态瞬时字符重排）、iPadOS** 打造，配备双排专属终端键盘（`esc | tab | ⌨/🎙 | ↑ | ⌫` / `^C | ^D | / | ← | ↓ | →` + 右侧跨两排高回车键 `↵`）与右侧垂直拇指轨。
   - **`web/` (TypeScript + `@noble/ciphers` + `fzstd`)**：浏览器端零知识加密终端控制台，1:1 还原纯黑 OLED 终端界面。
   - **`deploy/`**：提供 [完整生产环境极简部署指南 (`deploy/DEPLOY.md`)](deploy/DEPLOY.md)、`install-server.sh`、`install-relay-cn.sh`、`install-daemon.sh`（支持 `curl -fsSL https://<server>/install.sh | sh -s -- --server ... --code ...` 一键安装配置 `launchd` / `systemd`）、`Dockerfile`、`docker-compose.yml` 与国内免备案中继 `coda-relay-cn.service`。

---

## 本地开发与测试快速启动

### 1. 运行全套单元与端到端集成测试（PostgreSQL 18 + Herdr Core + GPUI 桌面端 + Swift 原生）

```bash
# Rust 全工作区测试（含多租户隔离、免备案裸 TCP 隧道、CDN WSS 无缝换轨、Direct P2P 直连、GPUI 桌面端与真实 hc 编排）
cargo test --workspace

# 启动 Rust + GPUI 120Hz 原生桌面客户端（支持 macOS / Windows / Linux）
cargo run -p coda-desktop

# Apple Swift 6 原生 CryptoKit 互通与 iPhone Duo 折叠屏动态重排自检
cd apple && swift run CodaSelfTest
```

### 2. 启动服务端与 Web 控制台

```bash
# 构建前端静态产物
cd web && bun install && bun run build && cd ..

# 启动 Coda 控制面 (:8080) + 免备案裸 TCP 中继 (:24443)
DATABASE_URL="postgres://vampire@localhost:5432/coda_dev" cargo run -p coda-server
```

浏览器打开 `http://127.0.0.1:8080` 注册首个管理员账户，点击 **「+ 配对新机器」** 获取 6 位配对码后，在任意 Mac 或 Linux 服务器上执行：

```bash
cargo run -p coda-daemon -- pair --server http://127.0.0.1:8080 --code 839-204
cargo run -p coda-daemon -- run
```

### 3. 国内阿里云/腾讯云免备案轻量中继独立部署（无需连接数据库）

```bash
CODA_RELAY_SECRET="与控制面一致的密钥" coda-server --relay-only --raw-tcp-bind 0.0.0.0:24443
```
