#!/usr/bin/env sh
# Coda Host Daemon One-Click Installer & Pairing Script (macOS & Linux)
# Usage:
#   curl -fsSL https://<coda-server>/install.sh | sh -s -- --server https://<coda-server> --code 839-204 [--name "Mac Studio"]

set -eu

REPO="Annihilater/coda"
SERVER_URL="http://127.0.0.1:8080"
PAIR_CODE=""
DEVICE_NAME=""
INSTALL_DIR="${HOME}/.local/bin"

while [ $# -gt 0 ]; do
  case "$1" in
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
      PKG_NAME="coda-daemon-macos-aarch64.tar.gz"
      DIR_NAME="coda-daemon-macos-aarch64"
    elif [ "${OS_NAME}" = "Linux" ]; then
      case "${ARCH_NAME}" in
        x86_64|amd64) PKG_NAME="coda-daemon-linux-x86_64.tar.gz"; DIR_NAME="coda-daemon-linux-x86_64" ;;
        aarch64|arm64) PKG_NAME="coda-daemon-linux-aarch64.tar.gz"; DIR_NAME="coda-daemon-linux-aarch64" ;;
      esac
    fi

    if [ -n "${PKG_NAME}" ]; then
      echo "    Downloading prebuilt ${PKG_NAME} from GitHub Releases..."
      TMP_D="$(mktemp -d)"
      if curl -fsSL "https://github.com/${REPO}/releases/latest/download/${PKG_NAME}" -o "${TMP_D}/${PKG_NAME}"; then
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
else
  echo "==> [3/4] No --code supplied; skipping pairing step (run: coda-daemon pair --server ${SERVER_URL} --code XXX-XXX)"
fi

echo "==> [4/4] Configuring background service supervision..."
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
  echo "    Created macOS LaunchAgent at ${PLIST_PATH}"
  echo "    Load with: launchctl load -w ${PLIST_PATH}"
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
  echo "    Created systemd user unit at ${UNIT_PATH}"
  echo "    Enable with: systemctl --user daemon-reload && systemctl --user enable --now coda-daemon"
fi

echo "==> Coda Host Daemon setup complete!"
