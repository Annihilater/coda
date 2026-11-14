#!/usr/bin/env sh
# Coda Host Daemon One-Click Installer & Authorization Script (macOS & Linux: openEuler/CentOS/Debian/Ubuntu)
# Default (Browser Device Flow - recommended):
#   curl -fsSL https://<coda-server>/install.sh | sh
# Non-interactive Linux/CI enrollment:
#   sh install-daemon.sh --server http://<coda-server>:8080 --email you@example.com --password '***' --non-interactive
# Legacy 6-digit code pairing:
#   curl -fsSL https://<coda-server>/install.sh | sh -s -- --server https://<coda-server> --code 839-204 [--name "My Server"]

set -eu

REPO="Annihilater/coda"
VERSION="${CODA_VERSION:-latest}"
DEFAULT_SERVER_URL="__CODA_DEFAULT_SERVER_URL__"
case "${DEFAULT_SERVER_URL}" in
  __*__) DEFAULT_SERVER_URL="https://codahub.dev" ;;
esac
SERVER_URL="${CODA_SERVER_URL:-${DEFAULT_SERVER_URL}}"
PAIR_CODE=""
DEVICE_NAME=""
DEVICE_ROLE="${CODA_DEVICE_ROLE:-daemon}"
LOCAL_BIN="${CODA_LOCAL_BIN:-}"
LOCAL_HC_BIN="${CODA_LOCAL_HC_BIN:-}"
LOGIN_EMAIL="${CODA_LOGIN_EMAIL:-}"
LOGIN_PASSWORD="${CODA_LOGIN_PASSWORD:-}"
CONFIG_ARG=""
P2P_BIND="${CODA_P2P_BIND:-}"
NON_INTERACTIVE=0
INSTALL_DIR="${CODA_INSTALL_DIR:-${HOME}/.local/bin}"
LAUNCH_PWD="$(pwd)"

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      VERSION="$2"
      shift 2
      ;;
    --server)
      SERVER_URL="$2"
      shift 2
      ;;
    --code|--pair)
      PAIR_CODE="$2"
      shift 2
      ;;
    --name)
      DEVICE_NAME="$2"
      shift 2
      ;;
    --role)
      DEVICE_ROLE="$2"
      shift 2
      ;;
    --local-bin)
      LOCAL_BIN="$2"
      shift 2
      ;;
    --local-hc-bin)
      LOCAL_HC_BIN="$2"
      shift 2
      ;;
    --email)
      LOGIN_EMAIL="$2"
      shift 2
      ;;
    --password)
      LOGIN_PASSWORD="$2"
      shift 2
      ;;
    --config)
      CONFIG_ARG="$2"
      shift 2
      ;;
    --p2p-bind)
      P2P_BIND="$2"
      shift 2
      ;;
    --non-interactive)
      NON_INTERACTIVE=1
      shift 1
      ;;
    *)
      shift 1
      ;;
  esac
done

CFG_PATH="${CONFIG_ARG:-${CODA_DAEMON_CONFIG:-${HOME}/.config/coda/daemon.json}}"
export CODA_DAEMON_CONFIG="${CFG_PATH}"
STATE_DIR="$(dirname "${CFG_PATH}")"
mkdir -p "${INSTALL_DIR}" "${STATE_DIR}"
export PATH="${INSTALL_DIR}:${HOME}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:${PATH}"

run_as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    "$@"
  fi
}

OS_NAME="$(uname -s)"
if [ "${OS_NAME}" = "Linux" ]; then
  MISSING_PKGS=""
  command -v curl >/dev/null 2>&1 || MISSING_PKGS="${MISSING_PKGS} curl"
  command -v tar >/dev/null 2>&1 || MISSING_PKGS="${MISSING_PKGS} tar"
  command -v git >/dev/null 2>&1 || MISSING_PKGS="${MISSING_PKGS} git"
  if [ -n "${MISSING_PKGS}" ]; then
    DISTRO_ID=""
    DISTRO_LIKE=""
    if [ -r /etc/os-release ]; then
      # shellcheck disable=SC1091
      . /etc/os-release || true
      DISTRO_ID="${ID:-}"
      DISTRO_LIKE="${ID_LIKE:-}"
    fi
    echo "==> [0/4] Installing base Linux prerequisites (${MISSING_PKGS}) for ${DISTRO_ID:-linux}..."
    case " ${DISTRO_ID} ${DISTRO_LIKE} " in
      *debian*|*ubuntu*)
        run_as_root env DEBIAN_FRONTEND=noninteractive apt-get update -y >/dev/null
        # shellcheck disable=SC2086
        run_as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates ${MISSING_PKGS}
        ;;
      *openeuler*|*centos*|*rhel*|*rocky*|*almalinux*|*fedora*)
        if command -v dnf >/dev/null 2>&1; then
          # shellcheck disable=SC2086
          run_as_root dnf install -y ca-certificates ${MISSING_PKGS}
        elif command -v yum >/dev/null 2>&1; then
          # shellcheck disable=SC2086
          run_as_root yum install -y ca-certificates ${MISSING_PKGS}
        fi
        ;;
      *)
        if command -v dnf >/dev/null 2>&1; then
          # shellcheck disable=SC2086
          run_as_root dnf install -y ca-certificates ${MISSING_PKGS}
        elif command -v apt-get >/dev/null 2>&1; then
          run_as_root env DEBIAN_FRONTEND=noninteractive apt-get update -y >/dev/null
          # shellcheck disable=SC2086
          run_as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates ${MISSING_PKGS}
        fi
        ;;
    esac
  fi
fi

echo "==> [1/4] Checking herdr-core (herdr-core / hc) runtime..."
if [ -n "${LOCAL_HC_BIN}" ] && [ -x "${LOCAL_HC_BIN}" ]; then
  cp "${LOCAL_HC_BIN}" "${INSTALL_DIR}/hc"
  chmod +x "${INSTALL_DIR}/hc"
  echo "    Installed local herdr-core binary to ${INSTALL_DIR}/hc"
elif command -v herdr-core >/dev/null 2>&1; then
  echo "    Found herdr-core: $(command -v herdr-core)"
elif command -v hc >/dev/null 2>&1; then
  echo "    Found hc: $(command -v hc)"
else
  echo "    Installing herdr-core (hc)..."
  curl -fsSL https://releases.herdr.dev/core/install.sh | sh
fi

echo "==> [2/4] Checking coda-daemon binary..."
if [ -n "${LOCAL_BIN}" ] && [ -x "${LOCAL_BIN}" ]; then
  cp "${LOCAL_BIN}" "${INSTALL_DIR}/coda-daemon"
  chmod +x "${INSTALL_DIR}/coda-daemon"
  echo "    Installed local coda-daemon binary to ${INSTALL_DIR}/coda-daemon"
elif ! command -v coda-daemon >/dev/null 2>&1; then
  if [ -x "./target/release/coda-daemon" ]; then
    cp "./target/release/coda-daemon" "${INSTALL_DIR}/coda-daemon"
  elif [ -x "./target/debug/coda-daemon" ]; then
    cp "./target/debug/coda-daemon" "${INSTALL_DIR}/coda-daemon"
  else
    ARCH_NAME="$(uname -m)"
    PKG_NAME=""
    DIR_NAME=""
    if [ "${OS_NAME}" = "Darwin" ]; then
      DIR_NAME="coda-daemon-macos-aarch64"
    elif [ "${OS_NAME}" = "Linux" ]; then
      case "${ARCH_NAME}" in
        x86_64|amd64) DIR_NAME="coda-daemon-linux-x86_64" ;;
        aarch64|arm64) DIR_NAME="coda-daemon-linux-aarch64" ;;
      esac
    fi

    if [ -n "${DIR_NAME}" ]; then
      if [ "${VERSION}" = "latest" ]; then
        EFFECTIVE_URL="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest" 2>/dev/null || true)"
        if [ -n "${EFFECTIVE_URL}" ]; then
          VERSION="${EFFECTIVE_URL##*/}"
        fi
      fi
      PKG_NAME="${DIR_NAME}-${VERSION}.tar.gz"
      echo "    Downloading prebuilt ${PKG_NAME} from GitHub Releases..."
      TMP_D="$(mktemp -d)"
      if [ "${VERSION}" != "latest" ] && curl -fsSL "https://github.com/${REPO}/releases/download/${VERSION}/${PKG_NAME}" -o "${TMP_D}/${PKG_NAME}"; then
        tar -C "${TMP_D}" -xzf "${TMP_D}/${PKG_NAME}"
        cp "${TMP_D}/${DIR_NAME}/coda-daemon" "${INSTALL_DIR}/coda-daemon"
      else
        echo "    Release download unavailable; building coda-daemon from source via cargo..."
        cargo build --release -p coda-daemon
        cp "./target/release/coda-daemon" "${INSTALL_DIR}/coda-daemon"
      fi
      rm -rf "${TMP_D}"
    else
      cargo build --release -p coda-daemon
      cp "./target/release/coda-daemon" "${INSTALL_DIR}/coda-daemon"
    fi
  fi
  chmod +x "${INSTALL_DIR}/coda-daemon"
fi

if [ -n "${PAIR_CODE}" ]; then
  echo "==> [3/4] Pairing host with Coda Coordinator (${SERVER_URL}) using code ${PAIR_CODE}..."
  if [ -n "${DEVICE_NAME}" ]; then
    coda-daemon pair --server "${SERVER_URL}" --code "${PAIR_CODE}" --name "${DEVICE_NAME}"
  else
    coda-daemon pair --server "${SERVER_URL}" --code "${PAIR_CODE}"
  fi
elif [ -n "${LOGIN_EMAIL}" ] && [ -n "${LOGIN_PASSWORD}" ]; then
  echo "==> [3/4] Enrolling host non-interactively with Coda Coordinator (${SERVER_URL})..."
  if [ -n "${DEVICE_NAME}" ]; then
    coda-daemon login --server "${SERVER_URL}" --email "${LOGIN_EMAIL}" --password "${LOGIN_PASSWORD}" --role "${DEVICE_ROLE}" --name "${DEVICE_NAME}"
  else
    coda-daemon login --server "${SERVER_URL}" --email "${LOGIN_EMAIL}" --password "${LOGIN_PASSWORD}" --role "${DEVICE_ROLE}"
  fi
elif [ ! -f "${CFG_PATH}" ]; then
  if [ "${NON_INTERACTIVE}" -eq 1 ]; then
    echo "error: --non-interactive requires --email/--password, --code, or an existing ${CFG_PATH}" >&2
    exit 1
  fi
  echo "==> [3/4] Authorizing host with Coda Coordinator (${SERVER_URL}) via browser device login..."
  if [ -n "${DEVICE_NAME}" ]; then
    coda-daemon login --server "${SERVER_URL}" --role "${DEVICE_ROLE}" --name "${DEVICE_NAME}"
  else
    coda-daemon login --server "${SERVER_URL}" --role "${DEVICE_ROLE}"
  fi
else
  echo "==> [3/4] Existing daemon config detected at ${CFG_PATH} (re-run \`coda-daemon login --server ${SERVER_URL}\` to re-authorize)."
fi

start_nohup_daemon() {
  PID_FILE="${STATE_DIR}/coda-daemon.pid"
  LOG_FILE="${STATE_DIR}/coda-daemon.log"
  if [ -f "${PID_FILE}" ]; then
    OLD_PID="$(cat "${PID_FILE}" 2>/dev/null || true)"
    if [ -n "${OLD_PID}" ] && kill -0 "${OLD_PID}" 2>/dev/null; then
      kill "${OLD_PID}" 2>/dev/null || true
      sleep 1
    fi
  fi
  if [ -n "${P2P_BIND}" ]; then
    nohup "${INSTALL_DIR}/coda-daemon" --config "${CFG_PATH}" run --p2p-bind "${P2P_BIND}" >"${LOG_FILE}" 2>&1 </dev/null &
  else
    nohup "${INSTALL_DIR}/coda-daemon" --config "${CFG_PATH}" run >"${LOG_FILE}" 2>&1 </dev/null &
  fi
  DAEMON_PID=$!
  echo "${DAEMON_PID}" >"${PID_FILE}"
  sleep 1
  if ! kill -0 "${DAEMON_PID}" 2>/dev/null; then
    echo "error: coda-daemon exited prematurely; tail of ${LOG_FILE}:" >&2
    tail -n 40 "${LOG_FILE}" >&2 || true
    exit 1
  fi
  echo "    Background coda-daemon running (pid=${DAEMON_PID}, log=${LOG_FILE})"
}

echo "==> [4/4] Configuring & starting background service supervision..."
if [ "${OS_NAME}" = "Darwin" ]; then
  if [ "${NON_INTERACTIVE}" -eq 1 ] && [ -n "${CONFIG_ARG}" ]; then
    start_nohup_daemon
  else
    PLIST_DIR="${HOME}/Library/LaunchAgents"
    PLIST_PATH="${PLIST_DIR}/dev.coda.daemon.plist"
    mkdir -p "${PLIST_DIR}"
    cat > "${PLIST_PATH}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.coda.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>${INSTALL_DIR}/coda-daemon</string>
    <string>--config</string>
    <string>${CFG_PATH}</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>${INSTALL_DIR}:${HOME}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    <key>CODA_DAEMON_CONFIG</key>
    <string>${CFG_PATH}</string>
  </dict>
  <key>KeepAlive</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
EOF
    launchctl unload "${PLIST_PATH}" 2>/dev/null || true
    launchctl load -w "${PLIST_PATH}" 2>/dev/null || true
    echo "    macOS LaunchAgent active: ${PLIST_PATH}"
  fi
elif [ "${OS_NAME}" = "Linux" ]; then
  SERVICE_STARTED=0
  EXEC_CMD="${INSTALL_DIR}/coda-daemon --config ${CFG_PATH} run"
  if [ -n "${P2P_BIND}" ]; then
    EXEC_CMD="${EXEC_CMD} --p2p-bind ${P2P_BIND}"
  fi

  if [ "$(id -u)" -eq 0 ] && [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    SYS_UNIT="/etc/systemd/system/coda-daemon.service"
    cat > "${SYS_UNIT}" <<EOF
[Unit]
Description=Coda Zero-Knowledge Host Daemon (herdr-core bridge)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${LAUNCH_PWD}
Environment="HOME=${HOME}"
Environment="PATH=${INSTALL_DIR}:/usr/local/bin:/usr/bin:/bin"
Environment="CODA_DAEMON_CONFIG=${CFG_PATH}"
ExecStart=${EXEC_CMD}
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
    if systemctl daemon-reload 2>/dev/null && systemctl enable --now coda-daemon 2>/dev/null; then
      sleep 1
      if systemctl is-active --quiet coda-daemon; then
        SERVICE_STARTED=1
        echo "    systemd system service active: ${SYS_UNIT}"
      fi
    fi
  elif [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1 && systemctl --user status >/dev/null 2>&1; then
    UNIT_DIR="${HOME}/.config/systemd/user"
    UNIT_PATH="${UNIT_DIR}/coda-daemon.service"
    mkdir -p "${UNIT_DIR}"
    cat > "${UNIT_PATH}" <<EOF
[Unit]
Description=Coda Zero-Knowledge Host Daemon (herdr-core bridge)
After=network-online.target

[Service]
Type=simple
WorkingDirectory=${LAUNCH_PWD}
Environment="PATH=${INSTALL_DIR}:/usr/local/bin:/usr/bin:/bin"
Environment="CODA_DAEMON_CONFIG=${CFG_PATH}"
ExecStart=${EXEC_CMD}
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
EOF
    if systemctl --user daemon-reload 2>/dev/null && systemctl --user enable --now coda-daemon 2>/dev/null; then
      sleep 1
      if systemctl --user is-active --quiet coda-daemon; then
        SERVICE_STARTED=1
        echo "    systemd user service active: ${UNIT_PATH}"
      fi
    fi
  fi

  if [ "${SERVICE_STARTED}" -eq 0 ]; then
    echo "    systemd unavailable in this environment; starting supervised background coda-daemon..."
    start_nohup_daemon
  fi
fi

echo "==> Coda Host Daemon setup complete!"
