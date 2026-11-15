# Coda 生产环境配置参考、安全基线与运维手册

本文档随 `coda-deploy-bundle` 一并打包分发，补充说明 `.env` 环境变量、密钥与校验规则、E2EE v2 安全基线以及日常升级回滚流程；主安装步骤请同步参阅同目录的 `DEPLOY.md`。

---

## 一、`.env` 环境变量全量参考

所有部署模式（Docker Compose `./start.sh`、裸机 Systemd `/etc/coda/.env`、直接启动 `coda-server`）统一读取以下环境变量：

| 变量名 | 默认值 / 示例 | 适用模式 | 说明 |
| :--- | :--- | :--- | :--- |
| `DEPLOY_MODE` | `coordinator` | 脚本 | `coordinator`（完整主控 + PG18 + Web + 中继）或 `relay-cn`（纯无状态裸 TCP 中继） |
| `CODA_IMAGE` | `ghcr.io/annihilater/coda:<tag>` | Docker | 预构建多架构容器镜像；GHCR 不可达时自动回落 Releases 离线包 |
| `CODA_HOST_BIND_IP` | `127.0.0.1` | Docker (`coordinator`) | 宿主机端口绑定地址；默认仅绑定回环供 Cloudflare Tunnel 转发，直连公网改 `0.0.0.0` |
| `CODA_RELAY_BIND_IP` | `0.0.0.0` | Docker (`relay-cn`) | 国内无域名中继节点的宿主机绑定地址 |
| `CODA_HTTP_PORT` | `8080` | Docker (`coordinator`) | 宿主机映射的 HTTP / WebSocket 控制面端口 |
| `CODA_RAW_TCP_PORT` | `24443` | 全部 | 裸 TCP 加密隧道中继端口 |
| `CODA_HTTP_BIND` | `0.0.0.0:8080` | 裸机 / 二进制 | `coda-server` HTTP/WS 监听地址（Docker 内固定监听 `0.0.0.0:8080`） |
| `CODA_RAW_TCP_BIND` | `0.0.0.0:24443` | 裸机 / 二进制 | `coda-server` 裸 TCP 中继监听地址 |
| `CODA_PUBLIC_URL` | `https://codahub.dev` | `coordinator` | 对外公开访问地址（决定 WebAuthn/设备流 URL 与默认 WSS 隧道节点） |
| `CODA_REGION_LABEL` | `CN-BGP-NoICP` | 全部 | 中继节点展示标签 |
| `CODA_OPEN_REGISTRATION` | `false` | `coordinator` | 是否允许公开自助注册；`false` 时仅首号免邀，后续需管理员签发邀请码 |
| `CODA_RELAY_SECRET` | **必填**（32B hex） | **全部** | 控制面签发中继令牌与中继节点验签的共享 HMAC 密钥，主控与全部中继必须一致 |
| `POSTGRES_PASSWORD` | **必填** | `coordinator` | PostgreSQL 18 `coda` 用户密码（`relay-cn` 模式无需配置） |
| `DATABASE_URL` | 外部连接串（可选） | 裸机 / 外部 DB | 覆盖 Docker Compose 内置 PostgreSQL 连接串；支持 `DOCKER_DATABASE_URL` |
| `CODA_GITHUB_REPO` | `Annihilater/coda` | `coordinator` | 下载中心链接指向的 GitHub 仓库 |
| `CODA_RELEASE_VERSION` | `vX.Y.Z`（可选） | `coordinator` | 固定下载中心展示的版本标签；留空则由服务端自动跟随构建版本或 `latest` |
| `CODA_RAW_TCP_PUBLIC_ENDPOINT` | 例如 `cn-relay.codahub.dev:24443` | `coordinator` | 若海外主控配合独立公网 IP/国内中继暴露裸 TCP 端口，可显式声明公网端点 |
| `CODA_IOS_BETA_URL` | 可选 TestFlight URL | `coordinator` | 在 Web 下载中心展示 iOS TestFlight 测试入口 |
| `GITHUB_PROXY` | 可选加速前缀 | 脚本 | 如国内下载离线容器包缓慢，可设加速前缀（如 `https://ghfast.top/`） |

---

## 二、安全基线与校验机制

### 1. 必填密钥与 Fail-Closed 拦截
- `deploy/docker-compose.yml` 与 `coda.sh` 均已启用 Fail-Closed 校验：**未设置 `CODA_RELAY_SECRET` 或 `POSTGRES_PASSWORD`，或仍为 `change_me` 等示例占位符时将直接拒绝启动**。
- 首次在 `coda-deploy-bundle/` 下运行 `./pull.sh` 或 `./start.sh` 且当前目录不存在 `.env` 时，脚本会以 `umask 077` 自动使用 CSPRNG（`openssl rand -hex 24` 或 `/dev/urandom`）生成高熵随机密钥，写入权限 `0600` 的 `.env`，并将 `CODA_OPEN_REGISTRATION` 默认设为 `false`。

### 2. 最小权限容器运行时
- `ghcr.io/annihilater/coda` 运行时以非特权系统用户 `coda`（`UID/GID 10001`）执行，不以 `root` 身份运行服务端或中继进程。

### 3. 全制品 SHA-256 完整性校验
- 每次发版均会生成 `SHA256SUMS-<version>.txt` 与 `SHA256SUMS.txt`；`coda.sh` 离线镜像包回落、`install-server.sh`、`install-relay-cn.sh` 与 `install-daemon.sh` 均已内置自动 SHA-256 摘要校验，摘要不匹配或缺少校验清单时拒绝导入或安装。
- 如需手工校验已下载到本地的发布制品：
  ```bash
  sha256sum -c SHA256SUMS-<version>.txt --ignore-missing
  ```

### 4. 非交互式受控机接入（避免密码暴露在进程列表）
- 在 CI 或自动化脚本中登录绑定受控机时，请通过标准输入传递密码而非命令行明文参数：
  ```bash
  printf '%s\n' "$CODA_PASS" | ~/.local/bin/coda-daemon login \
    --server https://codahub.dev \
    --email ops@example.com \
    --password-stdin
  ```

### 5. E2EE v2 端到端加密与前向安全特性
- **Direct P2P v2**：在保留 Ed25519/X25519 身份绑定的同时引入双方临时 X25519 密钥交换，提供每次直连会话的前向安全（PFS），并将设备 ID 与会话角色绑定进 HKDF-SHA256 密钥上下文。
- **Relay v2**：每次中继会话携带 CSPRNG 随机生成的 `session_nonce_b64` 盐值派生独立密钥流；64 帧抗重放滑动窗口仅在 ChaCha20-Poly1305 AEAD 验签通过后提交序号，防止伪造高序号包污染窗口。
- **压缩包防爆**：Zstd 解压强制限制 16 MiB 单帧上限。

---

## 三、升级、回滚与排障速查

### 1. 日常平滑升级
```bash
# 查看当前状态与健康检查 (/api/v1/health)
./status.sh

# 拉取新版本镜像并重建容器（保留 coda_pgdata 数据库卷与 .env 配置）
./update.sh
```
> 若已升级到指定固定 Tag（例如 `.env` 中 `CODA_IMAGE=ghcr.io/annihilater/coda:v0.4.0`），请先修改 `.env` 中的 `CODA_IMAGE` 为新版本号后再执行 `./update.sh`（或 `./restart.sh`）。

### 2. 手动离线镜像导入
若服务器无法直连 `ghcr.io`，可预先从 GitHub Releases 下载对应架构的 `coda-image-linux-{amd64,arm64}-<version>.tar.gz` 与 `SHA256SUMS-<version>.txt`，校验后直接导入：
```bash
sha256sum -c "SHA256SUMS-${VER}.txt" --ignore-missing
docker load -i "coda-image-linux-amd64-${VER}.tar.gz"
./start.sh
```

### 3. 日志查看与服务恢复
```bash
# 实时追踪主控或国内中继日志
./logs.sh coda-coordinator
./logs.sh coda-relay-cn

# 停止并移除容器（默认保留数据库卷 coda_pgdata）
./stop.sh

# 裸机 Systemd 模式附加 --systemd 标志
./status.sh --systemd
./restart.sh --systemd
./logs.sh --systemd
```
