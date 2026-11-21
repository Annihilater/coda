# Coda 生产环境配置参考、安全基线与运维手册

本文档随 `coda-deploy-bundle` 一并打包分发，补充说明 `.env` 环境变量、密钥与校验规则、E2EE v2 安全基线、W3C VAPID Web Push 与好友免密重置机制以及日常升级回滚流程；主安装步骤请同步参阅同目录的 `DEPLOY.md`，全端产品交互规范请参阅 [`../docs/PRODUCT_PROTOTYPE.md`](../docs/PRODUCT_PROTOTYPE.md)。

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
| `CODA_PUBLIC_URL` | 留空（示例 `https://coda.example.com`） | `coordinator` | 对外公开访问地址；默认留空按请求 `Host`/代理自动推导，生产上线建议显式配置 |
| `CODA_TRUST_PROXY_HEADERS` | `auto`（可选 `true`/`false`） | `coordinator` | 反向代理真实 IP 信任策略（默认仅信任回环/私网代理的 `CF-Connecting-IP`/`X-Real-IP`） |
| `CODA_ALLOWED_ORIGINS` | 逗号分隔白名单（可选） | `coordinator` | 额外允许访问 CORS 与 WebSocket 的浏览器 `Origin` 白名单（默认严格同源 + `CODA_PUBLIC_URL`） |
| `CODA_REGION_LABEL` | `CN-BGP-NoICP` | 全部 | 中继节点展示标签 |
| `CODA_OPEN_REGISTRATION` | `false` | `coordinator` | 是否允许公开自助注册；`false` 时仅首号免邀，后续需管理员签发邀请码 |
| `CODA_RELAY_SECRET` | **必填**（32B hex） | **全部** | 控制面签发中继令牌与中继节点验签的共享 HMAC 密钥，主控与全部中继必须一致 |
| `POSTGRES_PASSWORD` | **必填** | `coordinator` | PostgreSQL 18 `coda` 用户密码（经 `PGPASSWORD` 传入，原生支持 `@`/`#`/`/`/`%` 等任意特殊字符） |
| `DATABASE_URL` | 外部连接串（可选，支持 `sslmode=require` TLS） | 裸机 / 外部 DB | 覆盖 Docker Compose 内置 PostgreSQL 连接串；支持 `DOCKER_DATABASE_URL` 与 `rustls` TLS |
| `CODA_GITHUB_REPO` | `Annihilater/coda` | `coordinator` | 下载中心与 `/install.sh` 注入指向的 GitHub 仓库 |
| `CODA_RELEASE_VERSION` | `vX.Y.Z`（可选） | `coordinator` | 固定下载中心与 `/install.sh` 使用的版本标签；留空则自动跟随最新 Release（带 5 分钟失败负缓存） |
| `CODA_RAW_TCP_PUBLIC_ENDPOINT` | 例如 `cn-relay.codahub.dev:24443` | `coordinator` | 若海外主控配合独立公网 IP/国内中继暴露裸 TCP 端口，可显式声明公网端点（`coda-daemon` 会与主控并发双路保活） |
| `CODA_IOS_BETA_URL` | 可选 TestFlight URL | `coordinator` | 在 Web 下载中心展示 iOS TestFlight 测试入口 |
| `CODA_SKIP_HC_INSTALL` | `0`（设为 `1` 跳过） | `install-daemon.sh` | 跳过远端 `herdr-core` 脚本安装，直接回落 `coda-daemon` 内置原生 Unix PTY Shell |
| `CODA_HC_INSTALL_SHA256` | 可选 64 位 SHA-256 | `install-daemon.sh` | 为 `herdr-core` 安装脚本提供强制摘要校验 |
| `GITHUB_PROXY` | 可选加速前缀 | 脚本 | 如国内下载离线容器包缓慢，可设加速前缀（如 `https://ghfast.top/`） |

---

## 二、安全基线与校验机制

### 1. 必填密钥与 Fail-Closed 拦截
- `deploy/docker-compose.yml` 与 `coda.sh` 均已启用 Fail-Closed 校验：**未设置 `CODA_RELAY_SECRET` 或 `POSTGRES_PASSWORD`，或仍为 `change_me` 等示例占位符时将直接拒绝启动**。
- 首次在 `coda-deploy-bundle/` 下运行 `./pull.sh` 或 `./start.sh` 且当前目录不存在 `.env` 时，脚本会以 `umask 077` 自动使用 CSPRNG（`openssl rand -hex 24` 或 `/dev/urandom`）生成高熵随机密钥，写入权限 `0600` 的 `.env`，并将 `CODA_OPEN_REGISTRATION` 默认设为 `false`。

### 2. 最小权限运行时与权限隔离
- **容器与裸机均非 root**：`ghcr.io/annihilater/coda` 容器以及裸机 `coda-server.service` / `coda-relay-cn.service` 均以非特权系统用户 `coda` 执行，裸机 Systemd 单元启用 `NoNewPrivileges`、`ProtectSystem=strict`、`ProtectHome=true`、`PrivateTmp=true` 等沙箱限制。
- **角色最小权限与设备吊销**：`role = "daemon"` 受控机无法反向控制同账号其他设备；删除设备时级联吊销关联会话并立即踢出活跃中继与 Direct P2P 连接；`coda-daemon` 支持 30 天中继票据提前 7 天 Ed25519 签名自动续签。
- **凭据最小暴露**：macOS `Coda.app` 会话 Token 优先存入 macOS Keychain（CLI/测试回落 `0600` 文件）；`install-daemon.sh` 防止 `sudo` 污染普通用户 `$HOME` 权限，且日志强制 `0600` 与 10 MiB 截断。

### 3. 全制品 SHA-256 完整性校验与客户端应用内自更新
- 每次发版均会生成 `SHA256SUMS-<version>.txt` 与 `SHA256SUMS.txt`；`coda.sh` 离线镜像包回落、`install-server.sh`、`install-relay-cn.sh`、`install-daemon.sh`、`coda-daemon upgrade` 以及 **`Coda.app` 应用内一键自更新**均内置强制 SHA-256 摘要比对，摘要不匹配或缺少校验清单时立即中止替换。
- 如需手工校验已下载到本地的发布制品：
  ```bash
  sha256sum -c SHA256SUMS-<version>.txt --ignore-missing
  ```

### 4. 管理员好友免密重置令牌与 W3C VAPID Web Push 安全基线
- **单次哈希重置令牌 (`password_reset_tokens`)**：管理员接口 `/api/v1/admin/users` 与 `/api/v1/admin/users/{id}/reset-link` 严格校验 `scope = 'full'` 且 `is_admin = true`；签发的 `rst_<32+ urlsafe chars>` 令牌仅以 `SHA-256` 摘要落库（24 小时 TTL），`/api/v1/auth/reset-password` 按客户端 IP 限流并通过 `UPDATE ... WHERE consumed_at IS NULL AND expires_at > NOW()` 单次原子核销，成功后仅清理该用户 `scope = 'full'` 会话并实时通知事件连接断开，不中断已绑定的 `coda-daemon` 节点。
- **零配置 W3C VAPID Web Push (`server_kv` & `web_push_subscriptions`)**：主控启动时在 PostgreSQL `server_kv` 原子初始化 P-256 PKCS#8 VAPID 密钥对（`--relay-only` 无数据库模式自动关闭）；推送载荷使用 `RFC 8292 ES256` 鉴权与 `RFC 8291 aes128gcm` 加密（仅含零知识元数据），所有订阅 `endpoint` 强制 `https://`、单用户上限 16 条、投递时通过 DNS 解析校验加 `resolve_to_addrs` IP 钉扎阻断 SSRF 重绑定，并在收到 `404`/`410` 时自动清理失效订阅；静态服务对 `/sw.js` 固定返回 `Service-Worker-Allowed: /` 与 `Cache-Control: no-cache`。
- **跨端提示词片段边界 (`user_prompt_snippets`)**：`/api/v1/me/snippets` 强制校验单用户最多 24 条、标题 `<= 60` 字符、内容 `<= 4000` 字符且全端账号严格租户隔离。

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

# 自动检测并拉取最新 Release 镜像、同步更新 .env 中 CODA_IMAGE 并平滑重建容器（保留 coda_pgdata）
# 若希望保持当前固定版本不自动升 tag，可加 --keep-tag；裸机模式传 --systemd 自动升级二进制
./update.sh
```
> 裸机 Systemd 模式下 `./update.sh --systemd` 会调用同目录 `install-server.sh` / `install-relay-cn.sh --version latest` 通过临时文件原子替换二进制（避免 `ETXTBSY`）并平滑重启服务。

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
