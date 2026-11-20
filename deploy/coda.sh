#!/usr/bin/env sh
# Coda Production Lifecycle Manager
# Supports Docker Compose (default) and Systemd (--systemd / DEPLOY_RUNTIME=systemd).
# Commands: pull | start | stop | restart | status | logs | update

set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
cd "${SCRIPT_DIR}"

REPO="Annihilater/coda"
RUNTIME="${DEPLOY_RUNTIME:-auto}"
SYSTEMD_FLAG=0

usage() {
  cat <<'EOF'
Coda 运维管理脚本

用法:
  ./coda.sh <命令> [选项]
  或直接运行快捷脚本: ./pull.sh | ./start.sh | ./stop.sh | ./restart.sh | ./status.sh | ./logs.sh | ./update.sh

命令:
  pull               预拉取所需容器镜像（postgres:18-alpine 与 coda 镜像；GHCR 不可达时自动回落离线包）
  start              首次检查 .env、拉取/加载预编译镜像并启动服务（按 .env 中的 DEPLOY_MODE）
  stop [-v]          执行 docker compose down 停止并移除容器（默认保留数据卷 coda_pgdata；加 -v 同时删除数据卷）
  restart            重新读取 .env 并重建 Coda 容器以使新配置生效
  status             查看运行状态、端口监听情况与健康检查结果
  logs [service...]  实时跟踪查看日志（可用 Ctrl+C 退出；可传服务名如 coda-coordinator）
  update             拉取最新镜像并平滑重启服务

常用选项:
  --relay-cn         临时以国内免备案纯中继模式运行（覆盖 .env 的 DEPLOY_MODE）
  --systemd          强制使用 systemd 管理（适用于裸机 install-server.sh / install-relay-cn.sh 部署）
  -h, --help         显示本帮助信息
EOF
}

CMD="${1:-}"
if [ -z "${CMD}" ] || [ "${CMD}" = "-h" ] || [ "${CMD}" = "--help" ]; then
  usage
  exit 0
fi
shift 1

OVERRIDE_MODE=""
PASSTHROUGH_ARGS=""
DOWN_FLAG=0
DOWN_VOLUMES_FLAG=0
KEEP_TAG_FLAG=0
FORCE_LATEST_FLAG=0

for arg in "$@"; do
  case "${arg}" in
    --systemd)
      RUNTIME="systemd"
      SYSTEMD_FLAG=1
      ;;
    --relay-cn)
      OVERRIDE_MODE="relay-cn"
      ;;
    --latest)
      FORCE_LATEST_FLAG=1
      ;;
    --keep-tag)
      KEEP_TAG_FLAG=1
      ;;
    --down)
      DOWN_FLAG=1
      ;;
    -v|--volumes)
      DOWN_FLAG=1
      DOWN_VOLUMES_FLAG=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      # Preserve extra args safely using newline delimiter is avoided; quote on pass
      PASSTHROUGH_ARGS="${PASSTHROUGH_ARGS:+${PASSTHROUGH_ARGS} }${arg}"
      ;;
  esac
done

detect_compose() {
  if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
    COMPOSE_KIND="v2"
    return 0
  fi
  if command -v docker-compose >/dev/null 2>&1; then
    COMPOSE_KIND="v1"
    return 0
  fi
  return 1
}

run_compose() {
  if [ "${COMPOSE_KIND}" = "v2" ]; then
    docker compose "$@"
  else
    docker-compose "$@"
  fi
}

resolve_runtime() {
  if [ "${RUNTIME}" = "systemd" ]; then
    return 0
  fi
  if detect_compose; then
    RUNTIME="docker"
    return 0
  fi
  if command -v systemctl >/dev/null 2>&1 && { [ -f /etc/systemd/system/coda-server.service ] || [ -f /etc/systemd/system/coda-relay-cn.service ]; }; then
    RUNTIME="systemd"
    SYSTEMD_FLAG=1
    return 0
  fi
  echo "[ERROR] 未检测到 docker compose / docker-compose，也未检测到已安装的 systemd 服务。" >&2
  echo "        请先安装 Docker Compose，或使用 ./install-server.sh / ./install-relay-cn.sh 进行裸机安装。" >&2
  exit 1
}

gen_secret_hex() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 24
  else
    od -An -N24 -tx1 /dev/urandom | tr -d ' \n'
  fi
}

load_env_file() {
  env_path="${1:-}"
  [ -n "${env_path}" ] && [ -f "${env_path}" ] || return 0
  while IFS= read -r raw_line || [ -n "${raw_line}" ]; do
    line="$(printf '%s' "${raw_line}" | tr -d '\r')"
    line="${line#"${line%%[![:space:]]*}"}"
    case "${line}" in
      ""|\#*) continue ;;
      export[[:space:]]*)
        line="${line#export}"
        line="${line#"${line%%[![:space:]]*}"}"
        ;;
    esac
    case "${line}" in
      *=*) ;;
      *) continue ;;
    esac
    env_key="${line%%=*}"
    env_val="${line#*=}"
    env_key="${env_key%"${env_key##*[![:space:]]}"}"
    case "${env_key}" in
      ''|[0-9]*|*[!A-Za-z0-9_]*) continue ;;
    esac
    case "${env_val}" in
      \'*\')
        env_val="${env_val#\'}"
        env_val="${env_val%\'}"
        ;;
      \"*\")
        env_val="${env_val#\"}"
        env_val="${env_val%\"}"
        env_val="$(printf '%s' "${env_val}" | sed 's/\\"/"/g; s/\\\\/\\/g')"
        ;;
    esac
    export "${env_key}=${env_val}"
  done < "${env_path}"
}

compute_sha256() {
  target_file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${target_file}" | awk '{print tolower($1)}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${target_file}" | awk '{print tolower($1)}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "${target_file}" | awk '{print tolower($NF)}'
  else
    echo "[ERROR] 缺少 sha256sum / shasum / openssl，无法校验离线包摘要。" >&2
    return 1
  fi
}

verify_release_sha256() {
  base_url="$1"
  ver_tag="$2"
  asset_name="$3"
  local_file="$4"
  if [ "${CODA_SKIP_CHECKSUM:-0}" = "1" ]; then
    return 0
  fi
  sums_file="$(mktemp)"
  if ! curl -fsSL "${base_url}/SHA256SUMS-${ver_tag}.txt" -o "${sums_file}" 2>/dev/null \
    && ! curl -fsSL "${base_url}/SHA256SUMS.txt" -o "${sums_file}" 2>/dev/null; then
    rm -f "${sums_file}"
    echo "[ERROR] 无法下载校验清单 SHA256SUMS-${ver_tag}.txt（或 SHA256SUMS.txt），拒绝导入未校验镜像。" >&2
    return 1
  fi
  expected_hash="$(awk -v target="${asset_name}" '{
    fname = $2;
    sub(/^\*/, "", fname);
    sub(/^.*\//, "", fname);
    if (fname == target) { print tolower($1); exit; }
  }' "${sums_file}")"
  rm -f "${sums_file}"
  if [ -z "${expected_hash}" ]; then
    echo "[ERROR] 校验清单中缺少 ${asset_name} 的 SHA256 记录。" >&2
    return 1
  fi
  actual_hash="$(compute_sha256 "${local_file}")" || return 1
  if [ "${actual_hash}" != "${expected_hash}" ]; then
    echo "[ERROR] ${asset_name} SHA256 校验失败 (expected=${expected_hash}, actual=${actual_hash})。" >&2
    return 1
  fi
  echo "==> [校验] SHA256 通过: ${asset_name} (${actual_hash})"
}

load_env() {
  require_configured="${1:-0}"

  if [ ! -f "${SCRIPT_DIR}/.env" ]; then
    template_env=""
    if [ -f "${SCRIPT_DIR}/.env.example" ]; then
      template_env="${SCRIPT_DIR}/.env.example"
    elif [ -f "${SCRIPT_DIR}/env.example" ]; then
      template_env="${SCRIPT_DIR}/env.example"
    fi

    if [ "${RUNTIME}" != "systemd" ] && { [ "${require_configured}" = "1" ] || [ "${CMD}" = "pull" ]; }; then
      rand_relay_secret="$(gen_secret_hex)"
      rand_pg_pass="$(gen_secret_hex)"
      (
        umask 077
        if [ -n "${template_env}" ]; then
          sed \
            -e "s|coda_shared_hmac_relay_secret_change_me_2026|${rand_relay_secret}|g" \
            -e "s|coda_prod_change_me_2026|${rand_pg_pass}|g" \
            -e "s|^CODA_OPEN_REGISTRATION=true|CODA_OPEN_REGISTRATION=false|g" \
            "${template_env}" > "${SCRIPT_DIR}/.env"
        else
          cat > "${SCRIPT_DIR}/.env" <<EOF
DEPLOY_MODE=coordinator
CODA_IMAGE=ghcr.io/annihilater/coda:latest
CODA_HOST_BIND_IP=127.0.0.1
CODA_HTTP_PORT=8080
CODA_RAW_TCP_PORT=24443
# CODA_PUBLIC_URL=https://coda.example.com
CODA_PUBLIC_URL=
CODA_REGION_LABEL=CN-BGP-NoICP
CODA_OPEN_REGISTRATION=false
CODA_GITHUB_REPO=Annihilater/coda
CODA_RELAY_SECRET=${rand_relay_secret}
POSTGRES_PASSWORD=${rand_pg_pass}
EOF
        fi
        chmod 600 "${SCRIPT_DIR}/.env" 2>/dev/null || true
      )
      echo "[INFO] 已安全创建配置文件 ${SCRIPT_DIR}/.env (mode 0600，默认仅监听 127.0.0.1，已生成随机密钥与密码，默认关闭开放注册 CODA_OPEN_REGISTRATION=false)。"
    elif [ -f "/etc/coda/.env" ]; then
      load_env_file "/etc/coda/.env"
    fi
  fi

  if [ -f "${SCRIPT_DIR}/.env" ]; then
    load_env_file "${SCRIPT_DIR}/.env"
  fi

  DEPLOY_MODE="${OVERRIDE_MODE:-${DEPLOY_MODE:-coordinator}}"
  HOST_BIND_IP="${CODA_HOST_BIND_IP:-127.0.0.1}"
  HTTP_PORT="${CODA_HTTP_PORT:-8080}"
  RAW_TCP_PORT="${CODA_RAW_TCP_PORT:-24443}"
  IMAGE="${CODA_IMAGE:-ghcr.io/annihilater/coda:latest}"
  PUBLIC_URL="${CODA_PUBLIC_URL:-}"

  if [ "${DEPLOY_MODE}" = "relay-cn" ] && [ -z "${POSTGRES_PASSWORD:-}" ]; then
    export POSTGRES_PASSWORD="unused_relay_cn_placeholder"
  fi

  if [ "${require_configured}" = "1" ]; then
    case "${CODA_RELAY_SECRET:-}" in
      ""|*change_me*|*change-me*)
        echo "[ERROR] CODA_RELAY_SECRET 缺失或仍为示例占位值，请修改 .env 后重试！" >&2
        exit 1
        ;;
    esac
    if [ "${DEPLOY_MODE}" != "relay-cn" ] && [ -z "${DOCKER_DATABASE_URL:-}" ]; then
      case "${POSTGRES_PASSWORD:-}" in
        ""|*change_me*|*change-me*|coda_prod_secret|coda_prod_pass)
          echo "[ERROR] POSTGRES_PASSWORD 缺失或仍为示例占位值，请修改 .env 后重试！" >&2
          exit 1
          ;;
      esac
    fi
  else
    export CODA_RELAY_SECRET="${CODA_RELAY_SECRET:-placeholder_not_for_runtime}"
    export POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-placeholder_not_for_runtime}"
  fi
}

ensure_image() {
  force_pull="${1:-0}"
  if [ "${force_pull}" != "1" ] && docker image inspect "${IMAGE}" >/dev/null 2>&1; then
    return 0
  fi

  echo "==> [镜像] 正在拉取预构建镜像 ${IMAGE} ..."
  if docker pull "${IMAGE}"; then
    return 0
  fi

  echo "[WARN] 直接从 GHCR 拉取 ${IMAGE} 失败，尝试从 GitHub Releases 下载预构建离线镜像包回落加载..." >&2
  if ! command -v curl >/dev/null 2>&1; then
    echo "[ERROR] 缺少 curl，无法下载 GitHub Releases 离线镜像包。" >&2
    exit 1
  fi

  arch="$(uname -m)"
  case "${arch}" in
    x86_64|amd64) img_arch="amd64" ;;
    aarch64|arm64) img_arch="arm64" ;;
    *)
      echo "[ERROR] 不支持的 CPU 架构: ${arch}" >&2
      exit 1
      ;;
  esac

  case "${IMAGE}" in
    *:*) tag="${IMAGE##*:}" ;;
    *) tag="latest" ;;
  esac

  proxy="${GITHUB_PROXY:-}"
  if [ "${tag}" = "latest" ]; then
    effective_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")"
    tag="${effective_url##*/}"
  fi

  asset="coda-image-linux-${img_arch}-${tag}.tar.gz"
  release_base="${proxy}https://github.com/${REPO}/releases/download/${tag}"
  download_url="${release_base}/${asset}"
  tmp_tar="$(mktemp)"
  echo "==> [镜像] 下载 ${download_url} ..."
  if ! curl -fL --retry 2 "${download_url}" -o "${tmp_tar}"; then
    rm -f "${tmp_tar}"
    echo "[ERROR] 无法下载离线镜像包 ${download_url}。请检查网络或配置 .env 中的 GITHUB_PROXY。" >&2
    exit 1
  fi
  if ! verify_release_sha256 "${release_base}" "${tag}" "${asset}" "${tmp_tar}"; then
    rm -f "${tmp_tar}"
    exit 1
  fi

  echo "==> [镜像] 正在导入镜像到 Docker..."
  load_out="$(gzip -dc "${tmp_tar}" | docker load)"
  rm -f "${tmp_tar}"
  printf '%s\n' "${load_out}"

  loaded_ref="$(printf '%s\n' "${load_out}" | sed -n 's/^Loaded image: //p' | tail -n 1)"
  if [ -n "${loaded_ref}" ] && [ "${loaded_ref}" != "${IMAGE}" ]; then
    docker tag "${loaded_ref}" "${IMAGE}"
  fi
}

wait_coordinator_health() {
  if ! command -v curl >/dev/null 2>&1; then
    return 0
  fi
  i=0
  while [ "${i}" -lt 20 ]; do
    if curl -fsS "http://127.0.0.1:${HTTP_PORT}/api/v1/health" >/dev/null 2>&1; then
      return 0
    fi
    i=$((i + 1))
    sleep 1
  done
  return 1
}

systemd_unit_for_mode() {
  if [ "${DEPLOY_MODE}" = "relay-cn" ] || { [ "${DEPLOY_MODE}" = "coordinator" ] && ! [ -f /etc/systemd/system/coda-server.service ] && [ -f /etc/systemd/system/coda-relay-cn.service ]; }; then
    printf '%s' "coda-relay-cn"
  else
    printf '%s' "coda-server"
  fi
}

resolve_runtime

if [ "${RUNTIME}" = "systemd" ] || [ "${SYSTEMD_FLAG}" = "1" ]; then
  load_env 0
  UNIT="$(systemd_unit_for_mode)"
  case "${CMD}" in
    pull)
      echo "[INFO] 当前为 systemd 裸机模式，无需拉取 Docker 镜像（如需安装/更新二进制请使用 install-server.sh / install-relay-cn.sh）。"
      ;;
    start)
      systemctl start "${UNIT}"
      systemctl --no-pager -l status "${UNIT}" || true
      ;;
    stop)
      systemctl stop "${UNIT}"
      ;;
    restart)
      systemctl restart "${UNIT}"
      systemctl --no-pager -l status "${UNIT}" || true
      ;;
    update)
      if [ "${UNIT}" = "coda-relay-cn" ] && [ -x "${SCRIPT_DIR}/install-relay-cn.sh" ]; then
        echo "==> 正在使用 install-relay-cn.sh 更新裸机中继至最新版本..."
        sh "${SCRIPT_DIR}/install-relay-cn.sh" --version latest
      elif [ "${UNIT}" = "coda-server" ] && [ -x "${SCRIPT_DIR}/install-server.sh" ]; then
        echo "==> 正在使用 install-server.sh 更新裸机主控至最新版本..."
        sh "${SCRIPT_DIR}/install-server.sh" --version latest
      else
        echo "[WARN] 当前目录未找到安装脚本，仅执行 systemctl restart ${UNIT}。如需升级二进制请重新运行 install-server.sh / install-relay-cn.sh。" >&2
        systemctl restart "${UNIT}"
      fi
      systemctl --no-pager -l status "${UNIT}" || true
      ;;
    status)
      systemctl --no-pager -l status "${UNIT}" || true
      ;;
    logs)
      journalctl -u "${UNIT}" -f -n 100
      ;;
    *)
      usage
      exit 1
      ;;
  esac
  exit 0
fi

# Docker Compose mode
case "${CMD}" in
  pull)
    load_env 1
    ensure_image 1
    if [ "${DEPLOY_MODE}" != "relay-cn" ]; then
      echo "==> [镜像] 正在拉取数据库镜像 postgres:18-alpine ..."
      run_compose pull postgres
    fi
    echo "==> 镜像拉取完成！如需微调域名或密钥可编辑 .env，然后直接运行 ./start.sh 启动。"
    ;;
  start)
    load_env 1
    ensure_image 0
    if [ "${DEPLOY_MODE}" = "relay-cn" ]; then
      echo "==> 正在启动 Coda 国内免备案裸 TCP 中继节点 (DEPLOY_MODE=relay-cn, :${RAW_TCP_PORT})..."
      run_compose --profile relay-cn up -d --no-deps coda-relay-cn
      echo "==> 启动完成！开放安全组/防火墙端口: ${RAW_TCP_PORT}/tcp"
    else
      echo "==> 正在启动 Coda 主控制面与 PostgreSQL 18 (DEPLOY_MODE=coordinator)..."
      run_compose up -d postgres coda-coordinator
      if wait_coordinator_health; then
        echo "==> 服务就绪！健康检查通过。"
      else
        echo "[WARN] 容器已启动，等待健康检查响应超时，可执行 ./logs.sh 查看详细日志。" >&2
      fi
      echo "    公网访问入口     : ${PUBLIC_URL:-<自动按请求 Host / 反向代理推导>}"
      echo "    宿主机本地监听   : ${HOST_BIND_IP}:${HTTP_PORT} (HTTP/WSS) | ${HOST_BIND_IP}:${RAW_TCP_PORT}"
    fi
    echo "    常用命令: ./pull.sh | ./status.sh | ./logs.sh | ./restart.sh | ./stop.sh"
    ;;
  stop)
    load_env 0
    if [ "${DOWN_VOLUMES_FLAG}" = "1" ]; then
      echo "==> 正在执行 docker compose down -v（停止并移除容器及数据卷 coda_pgdata）..."
      run_compose --profile relay-cn down -v --remove-orphans
    else
      echo "==> 正在执行 docker compose down（停止并移除容器，保留数据卷 coda_pgdata）..."
      run_compose --profile relay-cn down --remove-orphans
    fi
    ;;
  restart)
    load_env 1
    ensure_image 0
    echo "==> 正在重启服务 (docker compose down && up -d, DEPLOY_MODE=${DEPLOY_MODE})..."
    run_compose --profile relay-cn down --remove-orphans
    if [ "${DEPLOY_MODE}" = "relay-cn" ]; then
      run_compose --profile relay-cn up -d --no-deps coda-relay-cn
    else
      run_compose up -d postgres coda-coordinator
      wait_coordinator_health || true
    fi
    run_compose --profile relay-cn ps
    ;;
  status)
    load_env 0
    echo "==> 部署模式: ${DEPLOY_MODE} | 镜像: ${IMAGE}"
    run_compose --profile relay-cn ps
    if [ "${DEPLOY_MODE}" != "relay-cn" ] && command -v curl >/dev/null 2>&1; then
      echo "==> 健康检查 (http://127.0.0.1:${HTTP_PORT}/api/v1/health):"
      if health_json="$(curl -fsS "http://127.0.0.1:${HTTP_PORT}/api/v1/health" 2>/dev/null)"; then
        printf '    OK: %s\n' "${health_json}"
      else
        echo "    [WARN] 控制面暂未就绪或 HTTP 端口未在 127.0.0.1:${HTTP_PORT} 响应。"
      fi
    fi
    if command -v ss >/dev/null 2>&1; then
      echo "==> 端口监听:"
      ss -tln | grep -E ":${HTTP_PORT}\b|:${RAW_TCP_PORT}\b" || true
    fi
    ;;
  logs)
    load_env 0
    if [ -n "${PASSTHROUGH_ARGS}" ]; then
      # shellcheck disable=SC2086
      run_compose --profile relay-cn logs -f --tail=100 ${PASSTHROUGH_ARGS}
    elif [ "${DEPLOY_MODE}" = "relay-cn" ]; then
      run_compose --profile relay-cn logs -f --tail=100 coda-relay-cn
    else
      run_compose logs -f --tail=100 coda-coordinator postgres
    fi
    ;;
  update)
    load_env 1
    if [ "${KEEP_TAG_FLAG}" != "1" ]; then
      case "${IMAGE}" in
        ghcr.io/annihilater/coda:v*|ghcr.io/annihilater/coda:latest)
          if [ "${FORCE_LATEST_FLAG}" = "1" ] || [ "${IMAGE}" != "ghcr.io/annihilater/coda:latest" ]; then
            if command -v curl >/dev/null 2>&1; then
              latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest" 2>/dev/null || true)"
              latest_tag="${latest_url##*/}"
              case "${latest_tag}" in
                v*)
                  next_image="ghcr.io/annihilater/coda:${latest_tag}"
                  if [ "${next_image}" != "${IMAGE}" ]; then
                    echo "==> [更新] 检测到最新版本 ${latest_tag}，升级镜像 ${IMAGE} -> ${next_image}"
                    IMAGE="${next_image}"
                    export CODA_IMAGE="${next_image}"
                    if [ -f "${SCRIPT_DIR}/.env" ]; then
                      tmp_env="$(mktemp)"
                      chmod 600 "${tmp_env}" 2>/dev/null || true
                      if grep -q '^CODA_IMAGE=' "${SCRIPT_DIR}/.env"; then
                        sed "s|^CODA_IMAGE=.*|CODA_IMAGE=${next_image}|" "${SCRIPT_DIR}/.env" > "${tmp_env}"
                      else
                        cat "${SCRIPT_DIR}/.env" > "${tmp_env}"
                        printf '\nCODA_IMAGE=%s\n' "${next_image}" >> "${tmp_env}"
                      fi
                      mv -f "${tmp_env}" "${SCRIPT_DIR}/.env"
                      chmod 600 "${SCRIPT_DIR}/.env" 2>/dev/null || true
                    fi
                  fi
                  ;;
              esac
            fi
          fi
          ;;
      esac
    fi
    ensure_image 1
    echo "==> 正在使用最新镜像 (${IMAGE}) 重建服务 (DEPLOY_MODE=${DEPLOY_MODE})..."
    if [ "${DEPLOY_MODE}" = "relay-cn" ]; then
      run_compose --profile relay-cn up -d --force-recreate --no-deps coda-relay-cn
    else
      run_compose up -d postgres
      run_compose up -d --force-recreate --no-deps coda-coordinator
      wait_coordinator_health || true
    fi
    run_compose --profile relay-cn ps
    ;;
  *)
    echo "[ERROR] 未知命令: ${CMD}" >&2
    usage
    exit 1
    ;;
esac
