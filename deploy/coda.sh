#!/usr/bin/env sh
# Coda Production Lifecycle Manager
# Supports Docker Compose (default) and Systemd (--systemd / DEPLOY_RUNTIME=systemd).
# Commands: start | stop | restart | status | logs | update

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
  或直接运行快捷脚本: ./start.sh | ./stop.sh | ./restart.sh | ./status.sh | ./logs.sh | ./update.sh

命令:
  start              首次检查 .env、拉取/加载预编译镜像并启动服务（按 .env 中的 DEPLOY_MODE）
  stop [--down]      停止服务（加 --down 时同时移除容器，保留数据库卷 coda_pgdata）
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

for arg in "$@"; do
  case "${arg}" in
    --systemd)
      RUNTIME="systemd"
      SYSTEMD_FLAG=1
      ;;
    --relay-cn)
      OVERRIDE_MODE="relay-cn"
      ;;
    --down)
      DOWN_FLAG=1
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

load_env() {
  require_configured="${1:-0}"

  if [ ! -f "${SCRIPT_DIR}/.env" ]; then
    if [ "${require_configured}" = "1" ] && [ -f "${SCRIPT_DIR}/.env.example" ]; then
      cp "${SCRIPT_DIR}/.env.example" "${SCRIPT_DIR}/.env"
      chmod 600 "${SCRIPT_DIR}/.env" 2>/dev/null || true
      if [ "${CODA_AUTO_START:-0}" != "1" ]; then
        cat <<EOF
[INFO] 首次运行，已自动生成默认配置文件: ${SCRIPT_DIR}/.env
       请先修改其中的关键配置后再重新执行 ./start.sh ：
         1) DEPLOY_MODE       : coordinator (主控制面) 或 relay-cn (国内免备案裸TCP中继)
         2) CODA_PUBLIC_URL   : 例如 https://codahub.dev 或 http://你的公网IP:8080
         3) CODA_RELAY_SECRET : 主控制面与所有中继必须一致（可用 openssl rand -hex 32 生成）
         4) POSTGRES_PASSWORD : 数据库密码（仅 coordinator 模式需要）
       （如确定仅用默认值直接启动，可执行: CODA_AUTO_START=1 ./start.sh）
EOF
        exit 1
      fi
    elif [ -f "/etc/coda/.env" ]; then
      set -a
      # shellcheck disable=SC1091
      . "/etc/coda/.env"
      set +a
    fi
  fi

  if [ -f "${SCRIPT_DIR}/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    . "${SCRIPT_DIR}/.env"
    set +a
  fi

  DEPLOY_MODE="${OVERRIDE_MODE:-${DEPLOY_MODE:-coordinator}}"
  HTTP_PORT="${CODA_HTTP_PORT:-8080}"
  RAW_TCP_PORT="${CODA_RAW_TCP_PORT:-24443}"
  IMAGE="${CODA_IMAGE:-ghcr.io/annihilater/coda:latest}"
  PUBLIC_URL="${CODA_PUBLIC_URL:-http://127.0.0.1:${HTTP_PORT}}"

  if [ "${require_configured}" = "1" ]; then
    case "${CODA_RELAY_SECRET:-}" in
      ""|*change_me*|*change-me*)
        echo "[WARN] 当前 CODA_RELAY_SECRET 仍为默认占位值，生产环境强烈建议修改 .env 并执行 ./restart.sh ！" >&2
        ;;
    esac
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
  download_url="${proxy}https://github.com/${REPO}/releases/download/${tag}/${asset}"
  tmp_tar="$(mktemp)"
  echo "==> [镜像] 下载 ${download_url} ..."
  if ! curl -fL --retry 2 "${download_url}" -o "${tmp_tar}"; then
    rm -f "${tmp_tar}"
    echo "[ERROR] 无法下载离线镜像包 ${download_url}。请检查网络或配置 .env 中的 GITHUB_PROXY。" >&2
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
    start)
      systemctl start "${UNIT}"
      systemctl --no-pager -l status "${UNIT}" || true
      ;;
    stop)
      systemctl stop "${UNIT}"
      ;;
    restart|update)
      systemctl restart "${UNIT}"
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
      echo "    Web 控制台与 API : ${PUBLIC_URL} (本地端口 :${HTTP_PORT})"
      echo "    免备案裸 TCP 隧道 : :${RAW_TCP_PORT}"
    fi
    echo "    常用命令: ./status.sh | ./logs.sh | ./restart.sh | ./stop.sh"
    ;;
  stop)
    load_env 0
    if [ "${DOWN_FLAG}" = "1" ]; then
      echo "==> 正在停止并移除 Coda 容器（保留数据卷 coda_pgdata）..."
      run_compose --profile relay-cn down
    else
      echo "==> 正在停止 Coda 容器..."
      run_compose --profile relay-cn stop
    fi
    ;;
  restart)
    load_env 1
    ensure_image 0
    echo "==> 正在重载 .env 并重建服务 (DEPLOY_MODE=${DEPLOY_MODE})..."
    if [ "${DEPLOY_MODE}" = "relay-cn" ]; then
      run_compose --profile relay-cn up -d --force-recreate --no-deps coda-relay-cn
    else
      run_compose up -d postgres
      run_compose up -d --force-recreate --no-deps coda-coordinator
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
    ensure_image 1
    echo "==> 正在使用最新镜像重建服务 (DEPLOY_MODE=${DEPLOY_MODE})..."
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
