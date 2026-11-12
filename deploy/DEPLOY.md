# Coda 生产环境极简部署指南 (Deployment Guide)

Coda 每次发布 Tag (`v*`) 时，GitHub Actions 流水线会自动构建并发布全平台制品包（位于 **GitHub Releases**）：

| 制品文件名 | 适用平台与内容 |
| :--- | :--- |
| **`coda-deploy-bundle.tar.gz`** | **完整部署目录包**（含 `docker-compose.yml`、`.env.example`、一键安装脚本与本指南） |
| **`coda-server-linux-x86_64.tar.gz`** | Linux x86_64 服务端（含 `coda-server` + 预构建 `web-dist` 前端 + `migrations` + `deploy`） |
| **`coda-server-linux-aarch64.tar.gz`** | Linux ARM64 服务端（同上） |
| **`coda-daemon-linux-x86_64.tar.gz`** | Linux x86_64 受控开发机守护进程 (`coda-daemon`) |
| **`coda-daemon-linux-aarch64.tar.gz`** | Linux ARM64 受控开发机守护进程 (`coda-daemon`) |
| **`coda-macos-aarch64.tar.gz`** | macOS Apple Silicon 全家桶（含 **`coda-desktop` [Rust+GPUI 120Hz 桌面端]**、`CodaMacApp` [SwiftUI]、`coda-daemon`、`coda-server`） |

---

## 一、服务端部署（二选一：Docker 一键启动 或 裸机一键脚本）

### 方式 A：Docker Compose 一键部署（推荐，自带 PostgreSQL 18）

在你的服务器上下载 `coda-deploy-bundle.tar.gz` 并启动：

```bash
# 1. 下载并解压部署包
curl -fsSL https://github.com/Annihilater/coda/releases/latest/download/coda-deploy-bundle.tar.gz | tar -xz
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

### 方式 B：Linux 裸机一行命令安装（使用预编译 Release 二进制 + Systemd）

如果服务器已安装 PostgreSQL，直接运行：

```bash
curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-server.sh | sudo sh -s -- \
  --public-url http://你的服务器IP:8080 \
  --database-url "postgres://postgres:你的数据库密码@127.0.0.1:5432/coda_prod" \
  --relay-secret "你的共享HMAC密钥"
```

---

## 二、（可选）国内阿里云/腾讯云「免备案」中继节点一键部署

如果你将主控制面放在海外，同时有一台国内阿里云/腾讯云机器（**不绑域名、不备案、不开 80/443 端口、不装数据库**），只需在安全组放行 **`24443/tcp`**，然后运行：

```bash
curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-relay-cn.sh | sudo sh -s -- \
  --relay-secret "与主控制面完全相同的CODA_RELAY_SECRET" \
  --port 24443
```

脚本会自动开启 Linux 内核 `tcp_bbr` 拥塞控制，并以 `--relay-only` 无状态模式常驻运行。

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
   下载并解压 `coda-macos-aarch64.tar.gz`，直接运行：
   ```bash
   ./coda-macos-aarch64/coda-desktop
   ```
3. **iPhone / iPhone Duo / iPad 原生客户端 (`apple/`)**：
   使用 Xcode 打开 `apple/Package.swift` 选择你的真机编译运行。
