# Coda 生产环境极简部署指南 (Deployment Guide)

Coda 每次发布 Tag (`v*`) 时，GitHub Actions 流水线会自动构建并发布全平台制品包（位于 **GitHub Releases**）与多架构公开容器镜像（`ghcr.io/annihilater/coda:<version>` 与 `:latest`，同时支持 `linux/amd64` 与 `linux/arm64`）。

> **目录、镜像与配置规范说明**：
> - **产物命名**：所有发布压缩包文件名均携带版本号（例如 `-v0.2.3.tar.gz`），而 **解压后顶层目录名称均不带版本号**（例如 `coda-deploy-bundle/`、`coda-macos-aarch64/`），方便脚本与升级复用。
> - **预构建镜像（部署包无需 Dockerfile）**：`coda-deploy-bundle/` 直接引用公网镜像仓库 `ghcr.io/annihilater/coda:<version>`；同时 `start.sh` / `update.sh` 内置自动回落逻辑——若服务器无法访问 GHCR，会自动从 GitHub Releases 下载离线镜像包 `coda-image-linux-{amd64,arm64}-<version>.tar.gz` 并 `docker load` 导入。
> - **统一 `.env` 密钥管理**：`./start.sh`、`docker-compose.yml`、`coda-server.service`、`coda-relay-cn.service` 与直接运行 `coda-server` 二进制均**统一从 `.env`（或 `/etc/coda/.env`，`0600` 权限）读取 `CODA_RELAY_SECRET` 等环境变量**，`.service` 单元文件中不存放任何明文密钥。
> - **完整变量与安全规范**：有关 `.env` 全部配置项、必填密钥校验、`SHA256SUMS` 完整性校验、E2EE v2 前向安全及滚动升级说明，请查阅同目录下的 `CONFIG_AND_SECURITY.md`。

| 制品 / 镜像名称 | 解压后目录 / 平台 | 说明 |
| :--- | :--- | :--- |
| **`ghcr.io/annihilater/coda:<version>`** | `linux/amd64` / `linux/arm64` | **预构建多架构服务端容器镜像**（内置 `coda-server`、`coda-daemon` 与前端资源） |
| **`coda-deploy-bundle-<version>.tar.gz`** | `coda-deploy-bundle/` | **极简部署运维包**（含 `pull.sh`/`start.sh`/`stop.sh`/`restart.sh`/`status.sh`/`logs.sh`/`update.sh`、`docker-compose.yml`、`env.example`/`.env.example`、`*.service`、安装脚本、`DEPLOY.md` 与 `CONFIG_AND_SECURITY.md`，**无 `Dockerfile`**） |
| **`coda-image-linux-{amd64,arm64}-<version>.tar.gz`** | 离线容器镜像包 | 供内网或 GHCR 不可达环境执行 `docker load` 离线导入使用 |
| **`coda-server-linux-{x86_64,aarch64}-<version>.tar.gz`** | `coda-server-linux-*/` | Linux 服务端裸机包（含 `coda-server` + `web-dist` + `migrations` + `deploy`） |
| **`coda-daemon-linux-{x86_64,aarch64}-<version>.tar.gz`** | `coda-daemon-linux-*/` | Linux 受控开发机守护进程 (`coda-daemon`) |
| **`coda-macos-aarch64-<version>.tar.gz`** | `coda-macos-aarch64/` | macOS Apple Silicon 全家桶（含 **`coda-desktop` [Rust+GPUI 120Hz 桌面端]**、`CodaMacApp` [SwiftUI]、`libcoda_apple_ffi.dylib`、`coda-daemon`、`coda-server`） |
| **`Coda-Desktop-macos-aarch64-<version>.dmg`** | macOS App 安装镜像 | 拖入 `Applications` 即可运行的 `Coda.app`（原生 GPUI 桌面端，支持双向被控显式开关与独立 `coda-daemon` 平滑共存） |
| **`coda-daemon-macos-aarch64-<version>.tar.gz`** | `coda-daemon-macos-aarch64/` | macOS Apple Silicon 受控开发机守护进程 (`coda-daemon`) |

---

## Cloudflare Tunnel 域名接入（日本主控服务器 `codahub.dev` 推荐配置）

主控面板官方域名已预设为 **`https://codahub.dev`**（桌面端 `Coda.app`、iOS 客户端、`coda-daemon` 均默认指向此地址；同时 `.dev` 域名受浏览器 HSTS 强制 HTTPS 约束，Cloudflare Tunnel 自动终结 TLS 并升级 `wss://`）：

1. 在 Cloudflare Zero Trust Dashboard 的 **Cloudflare Tunnel (`cloudflared`) -> Published applications -> Add published application** 中按如下填写：
   - **Subdomain**：留空不填（即直接绑定根域名 `codahub.dev`）
   - **Domain / Zone**：选择 `codahub.dev`
   - **Path**：留空不填（转发所有页面、API 与 `/v1/tunnel/ws` WebSocket 隧道）
   - **Service Type / URL**：`HTTP` → **`http://127.0.0.1:8080`**（勿填 `https://`；建议写 `127.0.0.1` 避免 `localhost` 解析为 IPv6 `[::1]`）
   - 在 Cloudflare 域名 **Network** 设置中确认 **WebSockets** 为开启状态（用于 `/v1/tunnel/ws` 隧道转发）。
2. 检查服务端 `.env`（自建默认留空 `CODA_PUBLIC_URL=`，控制面会按请求 `Host` 与反向代理自动推导；生产环境建议显式配置公网域名）：
   ```dotenv
   CODA_PUBLIC_URL=https://codahub.dev
   CODA_OPEN_REGISTRATION=false   # 如需向公网开放免邀请码自助注册可设为 true
   ```
   > 只要 `CODA_PUBLIC_URL` 是 `https://` 域名且未配置 `CODA_RAW_TCP_PUBLIC_ENDPOINT`，控制面会自动将 `global-cdn-wss`（`wss://codahub.dev/v1/tunnel/ws`）设为首选，并屏蔽不可达的内网 `127.0.0.1:24443` 地址；若同时配置了独立国内免备案中继，`coda-daemon` 会并发保活主控面与国内 `:24443` 中继，确保 Web 端和移动端均保持极速连通。
3. 修改配置后执行 **`./restart.sh`** 让 `.env` 生效。
4. 访问 **`https://codahub.dev`** 即可打开 **Coda 官网与免登录全端下载中心**，访客无需登录即可注册或下载 Linux Server / macOS 桌面端 / deploy-bundle；后期如加自选 CDN，直接把 CDN 回源指向该域名即可。

---

## 一、服务端部署（极简运维脚本，无需本地构建）

### 方式 A：Docker Compose + 一键启停脚本（推荐，自带 PostgreSQL 18）

在你的服务器上下载最新带版本号的 `coda-deploy-bundle-<version>.tar.gz`，修改 `.env` 后直接执行 `./start.sh` 即可：

```bash
# 1. 自动获取最新版本号并下载解压（解压后目录固定为 coda-deploy-bundle/）
VER="$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/Annihilater/coda/releases/latest | awk -F/ '{print $NF}')"
curl -fsSL "https://github.com/Annihilater/coda/releases/download/${VER}/coda-deploy-bundle-${VER}.tar.gz" | tar -xz
cd coda-deploy-bundle

# 2. 预拉取镜像并一键启动（若无 .env，脚本会自动根据 env.example 生成随机密钥的 .env，并预设 https://codahub.dev）
./pull.sh
./start.sh

# （可选）如需自定义端口、中继密钥或关闭开放注册，查看 env.example 并编辑 .env 后执行 ./restart.sh
vim .env
./restart.sh
```

#### 日常运维脚本一览（改完 `.env` 后直接执行即可）

| 脚本 | 作用说明 |
| :--- | :--- |
| **`./pull.sh`** | 预拉取所需容器镜像（`postgres:18-alpine` 与 `coda` 镜像；GHCR 不可达时自动回落 GitHub Releases 离线镜像包） |
| **`./start.sh`** | 首次检查 `.env`，拉取/加载预构建镜像并启动服务（根据 `.env` 中的 `DEPLOY_MODE` 启动主控或国内中继） |
| **`./stop.sh`** | 执行 `docker compose down` 停止并移除容器（默认保留 `coda_pgdata` 数据库卷；传 `-v` 可同时清理数据卷） |
| **`./restart.sh`** | **修改 `.env` 后推荐执行**：执行 `docker compose down && up -d` 重新加载 `.env` 并重建服务 |
| **`./status.sh`** | 查看容器运行状态、端口监听（`:8080` / `:24443`）与 `/api/v1/health` 健康检查结果 |
| **`./logs.sh`** | 实时跟踪查看日志（`Ctrl+C` 退出；支持传服务名如 `./logs.sh coda-coordinator`） |
| **`./update.sh`** | 拉取最新版本容器镜像并平滑重启 |

> 以上脚本同样支持 `--systemd`（或 `DEPLOY_RUNTIME=systemd`）管理方式 B 安装的系统服务。
>
> **端口安全说明**：
> - 主控端（`coordinator`）默认设置了 `CODA_HOST_BIND_IP=127.0.0.1`，仅在本机回环暴露 `127.0.0.1:8080` 与 `127.0.0.1:24443`，**不向公网暴露 `0.0.0.0` / `[::]` 端口**，配合 Cloudflare Tunnel 使用无需在云安全组放行任何入站端口。
> - 如需无隧道公网直连部署，在 `.env` 将 `CODA_HOST_BIND_IP=0.0.0.0` 并执行 `./restart.sh`，再按需放行 `8080/tcp` 与 `24443/tcp`（纯国内中继 `DEPLOY_MODE=relay-cn` 默认监听 `0.0.0.0:24443`）。

---

### 方式 B：Linux 裸机一行命令安装（使用预编译 Release 二进制 + Systemd + `/etc/coda/.env`）

脚本会自动生成权限为 `0600` 的 `/etc/coda/.env`，`.service` 仅通过 `EnvironmentFile=/etc/coda/.env` 引用，不包含明文密钥。如果你在解压后的 `coda-deploy-bundle/` 目录中先执行 `cp .env.example .env` 改好密钥，也可以直接运行 `sudo ./install-server.sh`（自动读取当前目录的 `.env` 写入 `/etc/coda/.env`）：

```bash
curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-server.sh | sudo sh -s -- \
  --public-url http://你的服务器IP:8080 \
  --database-url "postgres://postgres:你的数据库密码@127.0.0.1:5432/coda_prod" \
  --relay-secret "你的共享HMAC密钥"
```

---

## 二、（可选）国内阿里云/腾讯云「免备案」中继节点一键部署

如果你将主控制面放在海外，同时有一台国内阿里云/腾讯云机器（**不绑域名、不备案、不开 80/443 端口、不装数据库**），只需在安全组放行 **`24443/tcp`**：

- **方式 1（Docker + 统一脚本，推荐）**：解压 `coda-deploy-bundle-<version>.tar.gz`，在 `.env` 中设置 `DEPLOY_MODE=relay-cn` 及与主控相同的 `CODA_RELAY_SECRET`，然后执行 **`./start.sh`**（如国内拉取 GHCR 较慢，可在 `.env` 设置 `GITHUB_PROXY=https://ghfast.top/` 自动走离线镜像包回落）。
- **方式 2（裸机 Systemd 一键安装）**：将控制面上的 `.env` 复制到本机当前目录或 `/etc/coda/.env`，然后运行 `sudo ./install-relay-cn.sh` 或：
  ```bash
  curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-relay-cn.sh | sudo sh -s -- \
    --relay-secret "与主控制面完全相同的CODA_RELAY_SECRET" \
    --port 24443
  ```

`coda-relay-cn.service` 与直接运行 `coda-server --relay-only` 均统一从 `.env` / `/etc/coda/.env` 读取密钥，脚本还会自动开启 Linux 内核 `tcp_bbr` 拥塞控制。

---

## 三、只登录一次，全端浏览器授权接入开发机 (Server / Mac / iOS)

1. **注册账号**：打开浏览器访问 **`https://codahub.dev`**（或 `http://你的服务器IP:8080`）：
   - 当 `.env` 设置了 `CODA_OPEN_REGISTRATION=true` 时，任何人都可**免邀请码自由注册**并直接进入控制台（第一个注册的用户同时拥有管理员权限）；
   - 如需封闭内测，设 `CODA_OPEN_REGISTRATION=false`，则除首位管理员外需使用管理员签发的 `CODA-XXXXXX` 邀请码。
2. **Linux / macOS 服务器一键接入（推荐浏览器设备码授权）**：
   直接在目标开发机或云服务器上执行官网首页的一键命令（脚本与 `coda-daemon login` 均已默认指向 `https://codahub.dev`，无需额外传 `--server`）：

```bash
curl -fsSL https://codahub.dev/install.sh | sh
```

执行后终端会打印激活链接与 8 位设备码（例如 **`https://codahub.dev/activate?code=ABCD-2345`**）。在任意浏览器打开并登录确认批准，服务器即自动绑定到你的账号并后台启动 `coda-daemon`。（同时兼容传统 6 位 `--code 839-204` 配对方式。）

```bash
# 查看当前绑定的设备状态、进程/锁健康、hc 可用性与票据有效期
~/.local/bin/coda-daemon status

# 一键拉取最新 GitHub Release（含 SHA256SUMS 强校验）并向运行中守护进程发送 SIGUSR1 零中断热升级
~/.local/bin/coda-daemon upgrade

# 常用运维子命令（支持停服、重启、登出解绑与完整卸载；亦可在 Web / GPUI / iOS 端一键远程 E2EE OTA 升级）
~/.local/bin/coda-daemon restart
~/.local/bin/coda-daemon stop
~/.local/bin/coda-daemon logout
~/.local/bin/coda-daemon uninstall
```

> 若目标主机未安装或不便安装 `herdr-core` (`hc`)，可在安装时传入 `CODA_SKIP_HC_INSTALL=1`，`coda-daemon` 会自动回落内置原生 Unix PTY Shell 会话管理。

---

## 四、使用各端客户端连接开发（Windows/Mac/iOS 互控）

1. **Web 终端 & iOS Safari PWA（零上架零安装）**：
   用电脑浏览器或 iPhone Safari 访问 `https://codahub.dev`（支持终端原始键盘输入、多标签与双栏分屏会话、`⌘F` 搜索与 `.md` 导出、E2EE 远端文件树浏览与沙箱化 `localhost` 端口预览、Whisper 语音转写输入、主机健康遥测与一键 OTA 升级）；在 iPhone Safari 点击「分享 → 添加到主屏幕」即可全屏当原生 App 使用；如有 **TestFlight** 链接，可在 `.env` 中配置 `CODA_IOS_BETA_URL`。
2. **macOS 桌面端（`.dmg` 双击安装，`Hybrid` 双向互控 + 显式被控开关）**：
   在官网下载中心直接点击下载 **`Coda-Desktop-macos-aarch64-<version>.dmg`** 并拖入 `Applications`；首次打开若提示未公证开发者，在终端执行一次：
   ```bash
   xattr -cr /Applications/Coda.app
   ```
   打开 `Coda.app` 点击左侧「🔑 浏览器登录」即可绑定同一账号——支持在侧边抽屉与设置页一键开关 **「允许远端控制本机（Allow Remote Control）」**，配备 Dock `Needs-You` 待审批角标、原生麦克风 Whisper 听写、`⌘F` 搜索 / `⌘+Click` 链接跳转 / `.md` 导出、E2EE 远端文件树与 `localhost` 端口预览，并与独立安装的 `coda-daemon` 自动共存互不冲突。
3. **iPhone / iPhone Duo / iPad 原生 SwiftUI 客户端 (`apple/`)**：
   使用 Xcode 打开 `apple/Package.swift`（或 `apple/project.yml`）编译到真机运行测试版，支持浏览器设备码授权、低延迟 PTY 实时输入、终端内联搜索与 `.md` 一键分享导出、E2EE 远端文件树与 `localhost` 端口预览、Whisper 语音录音转写与远程主机健康/OTA 管理。

---

## 五、内网 Wi-Fi / IPv6 自动直连协商机制（免配置）

只要控制端与 Mac / Linux 主机处于同一局域网 Wi-Fi（或均具备电信/联通/移动原生公网 IPv6），系统会自动跳过云端中继完成点对点加密直连：

- **自动发现与协议自适应**：主机（`coda-daemon` 或打开即生效的 Mac `coda-desktop`）在 `:24444` 同时监听 **Raw TCP（iOS / GPUI 客户端）**、**局域网 WebSocket（Web 浏览器）** 与 **UDP Wi-Fi 广播信标**；每次连网或切换 Wi-Fi 时自动上报最新内网地址。
- **零感知热切换与容灾回落**：
  - 同处 Wi-Fi 下自动竞速切换至 **⚡ 局域网直连（通常 < 1–3ms，不消耗服务器中继流量）**；
  - 当手机离开 Wi-Fi 切换至 **5G 蜂窝网络** 时，独立 E2EE 密钥会话会零中断回落至 **`:24443` 免备案裸 TCP** 或 **`/v1/tunnel/ws` CDN WSS**，确保远程终端与 AI Agent 会话不断线。
