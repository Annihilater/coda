# Coda 生产环境极简部署指南 (Deployment Guide)

Coda 每次发布 Tag (`v*`) 时，GitHub Actions 流水线会自动构建并发布全平台制品包（位于 **GitHub Releases**）。

> **目录与配置规范说明**：
> - **产物命名**：所有发布压缩包文件名均携带版本号（例如 `-v0.2.2.tar.gz`），而 **解压后顶层目录名称均不带版本号**（例如 `coda-deploy-bundle/`、`coda-macos-aarch64/`），方便脚本与升级复用。
> - **统一 `.env` 密钥管理**：`Docker Compose`、`coda-server.service`、`coda-relay-cn.service` 与直接运行 `coda-server` 二进制均**统一从 `.env`（或 `/etc/coda/.env`，`0600` 权限）读取 `CODA_RELAY_SECRET` 等环境变量**，`.service` 单元文件中不存放任何明文密钥！

| 制品压缩包文件名 | 解压后目录 | 适用平台与内容 |
| :--- | :--- | :--- |
| **`coda-deploy-bundle-<version>.tar.gz`** | `coda-deploy-bundle/` | **完整自包含部署包**（含预编译多架构二进制、前端静态资源、`docker-compose.yml`、`.env.example`、`coda-server.service`、`coda-relay-cn.service`、一键脚本与本指南） |
| **`coda-server-linux-x86_64-<version>.tar.gz`** | `coda-server-linux-x86_64/` | Linux x86_64 服务端（含 `coda-server` + 预构建 `web-dist` 前端 + `migrations` + `deploy`） |
| **`coda-server-linux-aarch64-<version>.tar.gz`** | `coda-server-linux-aarch64/` | Linux ARM64 服务端（同上） |
| **`coda-daemon-linux-x86_64-<version>.tar.gz`** | `coda-daemon-linux-x86_64/` | Linux x86_64 受控开发机守护进程 (`coda-daemon`) |
| **`coda-daemon-linux-aarch64-<version>.tar.gz`** | `coda-daemon-linux-aarch64/` | Linux ARM64 受控开发机守护进程 (`coda-daemon`) |
| **`coda-macos-aarch64-<version>.tar.gz`** | `coda-macos-aarch64/` | macOS Apple Silicon 全家桶（含 **`coda-desktop` [Rust+GPUI 120Hz 桌面端]**、`CodaMacApp` [SwiftUI]、`coda-daemon`、`coda-server`） |
| **`coda-daemon-macos-aarch64-<version>.tar.gz`** | `coda-daemon-macos-aarch64/` | macOS Apple Silicon 受控开发机守护进程 (`coda-daemon`) |

---

## 一、服务端部署（二选一：Docker 一键启动 或 裸机一键脚本）

### 方式 A：Docker Compose 一键部署（推荐，自带 PostgreSQL 18）

在你的服务器上下载最新带版本号的 `coda-deploy-bundle-<version>.tar.gz` 并启动（解压后目录为干净的 `coda-deploy-bundle/`）：

```bash
# 1. 自动获取最新版本号并下载解压（也可手动指定如 VER=v0.2.1）
VER="$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/Annihilater/coda/releases/latest | awk -F/ '{print $NF}')"
curl -fsSL "https://github.com/Annihilater/coda/releases/download/${VER}/coda-deploy-bundle-${VER}.tar.gz" | tar -xz
cd coda-deploy-bundle

# 2. 复制环境变量并修改密码与密钥
cp .env.example .env
# 编辑 .env 设置 POSTGRES_PASSWORD、CODA_RELAY_SECRET 和 CODA_PUBLIC_URL (例如 http://你的公网IP:8080 或 https://你的域名)

# 3. 启动控制面 (:8080) + 免备案裸 TCP 中继 (:24443)
docker compose up -d
```

> **防火墙/安全组放行端口**：
> - `8080/tcp`（Web 终端控制台、注册登录 API、海外 CDN WebSocket `/v1/tunnel/ws`）
> - `24443/tcp`（国内免备案纯二进制裸 TCP 隧道，零 HTTP/零 SNI）

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

如果你将主控制面放在海外，同时有一台国内阿里云/腾讯云机器（**不绑域名、不备案、不开 80/443 端口、不装数据库**），只需在安全组放行 **`24443/tcp`**。

- **方式 1（先配置 `.env` 再一键安装，推荐）**：将控制面上的 `.env` 复制到本机当前目录或 `/etc/coda/.env`（或仅保留 `CODA_RELAY_SECRET`），然后运行：
  ```bash
  curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-relay-cn.sh | sudo sh
  ```
- **方式 2（命令行传参写入 `/etc/coda/.env`）**：
  ```bash
  curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-relay-cn.sh | sudo sh -s -- \
    --relay-secret "与主控制面完全相同的CODA_RELAY_SECRET" \
    --port 24443
  ```

`coda-relay-cn.service` 与直接运行 `coda-server --relay-only` 均统一从 `.env` / `/etc/coda/.env` 读取密钥，脚本还会自动开启 Linux 内核 `tcp_bbr` 拥塞控制。

---

## 三、注册管理员账号 & 一键接入开发机 (Mac / Linux Server)

1. **注册管理员**：打开浏览器访问 `http://你的服务器IP:8080`，点击 **「注册新账号」**。**第一个注册的用户会自动成为管理员（无需邀请码）**。
2. **生成配对码 & 邀请朋友**：
   - 点击左上角 **`[ ⚙ ]`** 按钮：
     - 点击 **「+ 签发好友邀请码」**，把生成的 `CODA-XXXXXX` 发给朋友即可让他们注册各自独立的账号。
     - 点击 **「+ 生成 6 位机器配对码」**，复制弹出的一键配对命令。
3. **在你的 Mac 或 Linux 开发机上运行一键配对命令**：

```bash
curl -fsSL http://你的服务器IP:8080/install.sh | sh -s -- \
  --server http://你的服务器IP:8080 \
  --code 839-204 \
  --name "Home-Mac-Studio"
```

该脚本会自动安装 `herdr-core` (`hc`)、下载对应系统的 `coda-daemon`、完成零知识密钥配对，并启动守护进程：

```bash
# 立即在前台或后台启动隧道守护进程（同时开启 :24444 局域网/IPv6 P2P 直连引擎）
~/.local/bin/coda-daemon run
```

---

## 四、使用各端客户端连接开发

1. **Web 终端控制台（零安装）**：直接用手机或电脑浏览器访问 `http://你的服务器IP:8080`。
2. **macOS 原生 120Hz GPUI 桌面客户端**：
   下载并解压带版本号的 `coda-macos-aarch64-<version>.tar.gz`（解压后目录固定为 `coda-macos-aarch64/`），直接运行：
   ```bash
   VER="$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/Annihilater/coda/releases/latest | awk -F/ '{print $NF}')"
   curl -fsSL "https://github.com/Annihilater/coda/releases/download/${VER}/coda-macos-aarch64-${VER}.tar.gz" | tar -xz
   ./coda-macos-aarch64/coda-desktop
   ```
3. **iPhone / iPhone Duo / iPad 原生客户端 (`apple/`)**：
   使用 Xcode 打开 `apple/Package.swift` 选择你的真机编译运行。

---

## 五、内网 Wi-Fi / IPv6 自动直连协商机制（免配置）

只要控制端与 Mac / Linux 主机处于同一局域网 Wi-Fi（或均具备电信/联通/移动原生公网 IPv6），系统会自动跳过云端中继完成点对点加密直连：

- **自动发现与协议自适应**：主机（`coda-daemon` 或打开即生效的 Mac `coda-desktop`）在 `:24444` 同时监听 **Raw TCP（iOS / GPUI 客户端）**、**局域网 WebSocket（Web 浏览器）** 与 **UDP Wi-Fi 广播信标**；每次连网或切换 Wi-Fi 时自动上报最新内网地址。
- **零感知热切换与容灾回落**：
  - 同处 Wi-Fi 下自动竞速切换至 **⚡ 局域网直连（通常 < 1–3ms，不消耗服务器中继流量）**；
  - 当手机离开 Wi-Fi 切换至 **5G 蜂窝网络** 时，独立 E2EE 密钥会话会零中断回落至 **`:24443` 免备案裸 TCP** 或 **`/v1/tunnel/ws` CDN WSS**，确保远程终端与 AI Agent 会话不断线。
