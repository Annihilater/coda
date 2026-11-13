#!/usr/bin/env sh
# Coda Host Daemon One-Click Installer & Browser Authorization Script (macOS & Linux)
# Default (Browser Device Flow - recommended):
#   curl -fsSL https://<coda-server>/install.sh | sh
# Legacy 6-digit code pairing:
#   curl -fsSL https://<coda-server>/install.sh | sh -s -- --server https://<coda-server> --code 839-204 [--name "My Server"]

set -eu

REPO="Annihilater/coda"
VERSION="${CODA_VERSION:-latest}"
DEFAULT_SERVER_URL="__CODA_DEFAULT_SERVER_URL__"
case "${DEFAULT_SERVER_URL}" in
  __*__) DEFAULT_SERVER_URL="http://127.0.0.1:8080" ;;
esac
SERVER_URL="${CODA_SERVER_URL:-${DEFAULT_SERVER_URL}}"
PAIR_CODE=""
DEVICE_NAME=""
DEVICE_ROLE="${CODA_DEVICE_ROLE:-daemon}"
INSTALL_DIR="${HOME}/.local/bin"

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
    *)
      shift 1
      ;;
  esac
done

mkdir -p "${INSTALL_DIR}"
export PATH="${INSTALL_DIR}:/opt/homebrew/bin:/usr/local/bin:${PATH}"

echo "==> [1/4] Checking herdr-core (hc) runtime..."
if ! command -v hc >/dev/null 2>&1; then
  echo "    Installing herdr-core (hc)..."
  curl -fsSL https://releases.herdr.dev/core/install.sh | sh
else
  echo "    Found hc: $(command -v hc)"
fi

echo "==> [2/4] Checking coda-daemon binary..."
if ! command -v coda-daemon >/dev/null 2>&1; then
  if [ -x "./target/release/coda-daemon" ]; then
    cp "./target/release/coda-daemon" "${INSTALL_DIR}/coda-daemon"
  elif [ -x "./target/debug/coda-daemon" ]; then
    cp "./target/debug/coda-daemon" "${INSTALL_DIR}/coda-daemon"
  else
    OS_NAME="$(uname -s)"
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
elif [ ! -f "${HOME}/.config/coda/daemon.json" ]; then
  echo "==> [3/4] Authorizing host with Coda Coordinator (${SERVER_URL}) via browser device login..."
  if [ -n "${DEVICE_NAME}" ]; then
    coda-daemon login --server "${SERVER_URL}" --role "${DEVICE_ROLE}" --name "${DEVICE_NAME}"
  else
    coda-daemon login --server "${SERVER_URL}" --role "${DEVICE_ROLE}"
  fi
else
  echo "==> [3/4] Existing daemon config detected at ${HOME}/.config/coda/daemon.json (re-run \`coda-daemon login --server ${SERVER_URL}\` to re-authorize)."
fi

echo "==> [4/4] Configuring & starting background service supervision..."
OS_NAME="$(uname -s)"
if [ "${OS_NAME}" = "Darwin" ]; then
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
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>${INSTALL_DIR}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
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
elif [ "${OS_NAME}" = "Linux" ]; then
  UNIT_DIR="${HOME}/.config/systemd/user"
  UNIT_PATH="${UNIT_DIR}/coda-daemon.service"
  mkdir -p "${UNIT_DIR}"
  cat > "${UNIT_PATH}" <<EOF
[Unit]
Description=Coda Zero-Knowledge Host Daemon (herdr-core bridge)
After=network-online.target

[Service]
Type=simple
Environment="PATH=${INSTALL_DIR}:/usr/local/bin:/usr/bin:/bin"
ExecStart=${INSTALL_DIR}/coda-daemon run
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
EOF
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload 2>/dev/null || true
    systemctl --user enable --now coda-daemon 2>/dev/null || true
  fi
  echo "    systemd user unit installed: ${UNIT_PATH}"
  echo "    Check status with: coda-daemon status"
fi

echo "==> Coda Host Daemon setup complete!"
