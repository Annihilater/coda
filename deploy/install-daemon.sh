#!/usr/bin/env sh
# Coda Host Daemon One-Click Installer & Authorization Script (macOS & Linux: openEuler/CentOS/Debian/Ubuntu)
# Default (Browser Device Flow - recommended):
#   curl -fsSL https://<coda-server>/install.sh | sh
# Non-interactive Linux/CI enrollment:
#   sh install-daemon.sh --server http://<coda-server>:8080 --email you@example.com --password '***' --non-interactive
# Legacy 6-digit code pairing:
#   curl -fsSL https://<coda-server>/install.sh | sh -s -- --server https://<coda-server> --code 839-204 [--name "My Server"]

set -eu

REPO="__CODA_GITHUB_REPO__"
case "${REPO}" in
  ""|*__*) REPO="Annihilater/coda" ;;
esac
DEFAULT_RELEASE_VERSION="__CODA_RELEASE_VERSION__"
case "${DEFAULT_RELEASE_VERSION}" in
  ""|*__*) DEFAULT_RELEASE_VERSION="latest" ;;
esac
VERSION="${CODA_VERSION:-${DEFAULT_RELEASE_VERSION}}"
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
FORCE_REINSTALL="${CODA_FORCE_REINSTALL:-0}"
INSTALL_DIR="${CODA_INSTALL_DIR:-${HOME}/.local/bin}"

# Guard against `sudo sh` polluting a non-root user's $HOME with root-owned files.
SUDO_FIXUP_USER=""
TARGET_HOME="${HOME:-/root}"
if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
  SUDO_USER_HOME="$(eval printf '%s' "~${SUDO_USER}" 2>/dev/null || true)"
  if [ -n "${SUDO_USER_HOME}" ] && [ "${HOME}" = "${SUDO_USER_HOME}" ]; then
    echo "[WARN] Detected sudo preserving ${SUDO_USER}'s HOME (${HOME}); will restore ownership and run service as ${SUDO_USER}." >&2
    SUDO_FIXUP_USER="${SUDO_USER}"
    TARGET_HOME="${SUDO_USER_HOME}"
  fi
fi

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

atomic_install_bin() {
  src="$1"
  dst="$2"
  mkdir -p "$(dirname "${dst}")"
  tmp_bin="${dst}.tmp.$$"
  cp "${src}" "${tmp_bin}"
  chmod 0755 "${tmp_bin}"
  mv -f "${tmp_bin}" "${dst}"
}

is_real_herdr_core() {
  candidate="$1"
  [ -x "${candidate}" ] || return 1
  ver_out="$("${candidate}" --version 2>&1 || true)"
  case "$(printf '%s' "${ver_out}" | tr '[:upper:]' '[:lower:]')" in
    *herbstclient*|*herbstluftwm*) return 1 ;;
  esac
  return 0
}

echo "==> [1/4] Checking herdr-core (herdr-core / hc) runtime..."
if [ -n "${LOCAL_HC_BIN}" ] && [ -x "${LOCAL_HC_BIN}" ]; then
  atomic_install_bin "${LOCAL_HC_BIN}" "${INSTALL_DIR}/hc"
  echo "    Installed local herdr-core binary to ${INSTALL_DIR}/hc"
elif [ "${CODA_SKIP_HC_INSTALL:-0}" = "1" ]; then
  echo "    Skipping herdr-core install (CODA_SKIP_HC_INSTALL=1); coda-daemon will use built-in native PTY shell fallback if hc is absent."
elif command -v herdr-core >/dev/null 2>&1 && is_real_herdr_core "$(command -v herdr-core)"; then
  echo "    Found herdr-core: $(command -v herdr-core)"
elif command -v hc >/dev/null 2>&1 && is_real_herdr_core "$(command -v hc)"; then
  echo "    Found hc: $(command -v hc)"
else
  if command -v hc >/dev/null 2>&1 && ! is_real_herdr_core "$(command -v hc)"; then
    echo "    [WARN] Ignoring unrelated window-manager binary at $(command -v hc) (herbstclient)." >&2
  fi
  echo "    Downloading herdr-core installer (set CODA_SKIP_HC_INSTALL=1 to skip)..."
  HC_SCRIPT="$(mktemp)"
  if curl -fsSL https://releases.herdr.dev/core/install.sh -o "${HC_SCRIPT}"; then
    if [ -n "${CODA_HC_INSTALL_SHA256:-}" ]; then
      HC_ACTUAL="$(sha256sum "${HC_SCRIPT}" 2>/dev/null | awk '{print tolower($1)}' || shasum -a 256 "${HC_SCRIPT}" 2>/dev/null | awk '{print tolower($1)}' || true)"
      HC_EXPECTED="$(printf '%s' "${CODA_HC_INSTALL_SHA256}" | tr '[:upper:]' '[:lower:]')"
      if [ "${HC_ACTUAL}" != "${HC_EXPECTED}" ]; then
        rm -f "${HC_SCRIPT}"
        echo "error: herdr-core installer SHA256 mismatch (expected=${HC_EXPECTED}, actual=${HC_ACTUAL})" >&2
        exit 1
      fi
    fi
    sh "${HC_SCRIPT}" || echo "    [WARN] herdr-core installer failed; coda-daemon will fall back to built-in native PTY shell." >&2
  else
    echo "    [WARN] Failed to download herdr-core installer; coda-daemon will fall back to built-in native PTY shell." >&2
  fi
  rm -f "${HC_SCRIPT}"
fi

compute_sha256() {
  target_file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${target_file}" | awk '{print tolower($1)}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${target_file}" | awk '{print tolower($1)}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "${target_file}" | awk '{print tolower($NF)}'
  else
    echo "error: missing sha256sum / shasum / openssl for checksum verification" >&2
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
    echo "error: failed downloading SHA256SUMS-${ver_tag}.txt (or SHA256SUMS.txt)" >&2
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
    echo "error: SHA256SUMS missing entry for ${asset_name}" >&2
    return 1
  fi
  actual_hash="$(compute_sha256 "${local_file}")" || return 1
  if [ "${actual_hash}" != "${expected_hash}" ]; then
    echo "error: SHA256 mismatch for ${asset_name} (expected=${expected_hash}, actual=${actual_hash})" >&2
    return 1
  fi
  echo "    [OK] SHA256 verified (${asset_name}: ${actual_hash})"
}

xml_escape() {
  printf '%s' "${1:-}" | sed \
    -e 's/&/\&amp;/g' \
    -e 's/</\&lt;/g' \
    -e 's/>/\&gt;/g' \
    -e 's/"/\&quot;/g' \
    -e "s/'/\&apos;/g"
}

systemd_quote() {
  esc="$(printf '%s' "${1:-}" | tr -d '\r\n' | sed 's/\\/\\\\/g; s/"/\\"/g')"
  printf '"%s"' "${esc}"
}

echo "==> [2/4] Installing / upgrading coda-daemon binary..."
if [ -n "${LOCAL_BIN}" ] && [ -x "${LOCAL_BIN}" ]; then
  atomic_install_bin "${LOCAL_BIN}" "${INSTALL_DIR}/coda-daemon"
  echo "    Installed local coda-daemon binary to ${INSTALL_DIR}/coda-daemon"
elif [ "${CODA_ALLOW_LOCAL_TARGET:-0}" = "1" ] && [ -x "./target/release/coda-daemon" ]; then
  atomic_install_bin "./target/release/coda-daemon" "${INSTALL_DIR}/coda-daemon"
elif [ "${CODA_ALLOW_LOCAL_TARGET:-0}" = "1" ] && [ -x "./target/debug/coda-daemon" ]; then
  atomic_install_bin "./target/debug/coda-daemon" "${INSTALL_DIR}/coda-daemon"
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

    INSTALLED_VER=""
    if [ -x "${INSTALL_DIR}/coda-daemon" ]; then
      INSTALLED_VER="$("${INSTALL_DIR}/coda-daemon" --version 2>/dev/null | awk '{print $2}' || true)"
    fi
    TARGET_PLAIN_VER="${VERSION#v}"

    if [ "${FORCE_REINSTALL}" != "1" ] && [ -n "${INSTALLED_VER}" ] && [ "${INSTALLED_VER}" = "${TARGET_PLAIN_VER}" ]; then
      echo "    coda-daemon ${INSTALLED_VER} is already up to date at ${INSTALL_DIR}/coda-daemon"
    else
      PKG_NAME="${DIR_NAME}-${VERSION}.tar.gz"
      REL_BASE="https://github.com/${REPO}/releases/download/${VERSION}"
      echo "    Downloading & verifying prebuilt ${PKG_NAME} from GitHub Releases..."
      TMP_D="$(mktemp -d)"
      if [ "${VERSION}" != "latest" ] && curl -fsSL "${REL_BASE}/${PKG_NAME}" -o "${TMP_D}/${PKG_NAME}"; then
        if ! verify_release_sha256 "${REL_BASE}" "${VERSION}" "${PKG_NAME}" "${TMP_D}/${PKG_NAME}"; then
          rm -rf "${TMP_D}"
          exit 1
        fi
        tar -C "${TMP_D}" -xzf "${TMP_D}/${PKG_NAME}"
        atomic_install_bin "${TMP_D}/${DIR_NAME}/coda-daemon" "${INSTALL_DIR}/coda-daemon"
        echo "    Installed coda-daemon ${VERSION} to ${INSTALL_DIR}/coda-daemon"
      elif [ "${CODA_ALLOW_SOURCE_BUILD:-0}" = "1" ] && [ -f "./Cargo.toml" ]; then
        echo "    Release download unavailable; building coda-daemon from source via cargo..."
        cargo build --release -p coda-daemon
        atomic_install_bin "./target/release/coda-daemon" "${INSTALL_DIR}/coda-daemon"
      elif [ -x "${INSTALL_DIR}/coda-daemon" ]; then
        echo "    [WARN] Release download unavailable; keeping existing binary at ${INSTALL_DIR}/coda-daemon" >&2
      else
        rm -rf "${TMP_D}"
        echo "error: failed downloading ${PKG_NAME} from ${REL_BASE}" >&2
        exit 1
      fi
      rm -rf "${TMP_D}"
    fi
  else
    if [ "${CODA_ALLOW_SOURCE_BUILD:-0}" = "1" ] && [ -f "./Cargo.toml" ]; then
      cargo build --release -p coda-daemon
      atomic_install_bin "./target/release/coda-daemon" "${INSTALL_DIR}/coda-daemon"
    elif [ ! -x "${INSTALL_DIR}/coda-daemon" ]; then
      echo "error: unsupported OS/architecture (${OS_NAME}/${ARCH_NAME})" >&2
      exit 1
    fi
  fi
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
    printf '%s\n' "${LOGIN_PASSWORD}" | env -u CODA_LOGIN_PASSWORD coda-daemon login --server "${SERVER_URL}" --email "${LOGIN_EMAIL}" --password-stdin --role "${DEVICE_ROLE}" --name "${DEVICE_NAME}"
  else
    printf '%s\n' "${LOGIN_PASSWORD}" | env -u CODA_LOGIN_PASSWORD coda-daemon login --server "${SERVER_URL}" --email "${LOGIN_EMAIL}" --password-stdin --role "${DEVICE_ROLE}"
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

prepare_daemon_log() {
  log_path="$1"
  mkdir -p "$(dirname "${log_path}")"
  if [ -f "${log_path}" ]; then
    log_size="$(wc -c < "${log_path}" 2>/dev/null || echo 0)"
    if [ "${log_size}" -gt 10485760 ] 2>/dev/null; then
      tmp_log="${log_path}.trim.$$"
      (
        umask 077
        tail -c 2097152 "${log_path}" > "${tmp_log}" && mv -f "${tmp_log}" "${log_path}"
      ) || rm -f "${tmp_log}"
    fi
  else
    ( umask 077 && : > "${log_path}" )
  fi
  chmod 600 "${log_path}" 2>/dev/null || true
}

start_nohup_daemon() {
  PID_FILE="${STATE_DIR}/coda-daemon.pid"
  LOG_FILE="${STATE_DIR}/coda-daemon.log"
  prepare_daemon_log "${LOG_FILE}"
  if [ -f "${PID_FILE}" ]; then
    OLD_PID="$(cat "${PID_FILE}" 2>/dev/null || true)"
    if [ -n "${OLD_PID}" ] && kill -0 "${OLD_PID}" 2>/dev/null; then
      kill "${OLD_PID}" 2>/dev/null || true
      sleep 1
    fi
  fi
  if [ -n "${P2P_BIND}" ]; then
    nohup "${INSTALL_DIR}/coda-daemon" --config "${CFG_PATH}" run --p2p-bind "${P2P_BIND}" >>"${LOG_FILE}" 2>&1 </dev/null &
  else
    nohup "${INSTALL_DIR}/coda-daemon" --config "${CFG_PATH}" run >>"${LOG_FILE}" 2>&1 </dev/null &
  fi
  DAEMON_PID=$!
  ( umask 077 && echo "${DAEMON_PID}" >"${PID_FILE}" )
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
    LOG_FILE="${STATE_DIR}/coda-daemon.log"
    prepare_daemon_log "${LOG_FILE}"
    mkdir -p "${PLIST_DIR}"
    PLIST_P2P_XML=""
    if [ -n "${P2P_BIND}" ]; then
      PLIST_P2P_XML="    <string>--p2p-bind</string>
    <string>$(xml_escape "${P2P_BIND}")</string>"
    fi
    cat > "${PLIST_PATH}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.coda.daemon</string>
  <key>WorkingDirectory</key>
  <string>$(xml_escape "${HOME}")</string>
  <key>ProgramArguments</key>
  <array>
    <string>$(xml_escape "${INSTALL_DIR}/coda-daemon")</string>
    <string>--config</string>
    <string>$(xml_escape "${CFG_PATH}")</string>
    <string>run</string>
${PLIST_P2P_XML}
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>$(xml_escape "${INSTALL_DIR}:${HOME}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")</string>
    <key>CODA_DAEMON_CONFIG</key>
    <string>$(xml_escape "${CFG_PATH}")</string>
  </dict>
  <key>StandardOutPath</key>
  <string>$(xml_escape "${LOG_FILE}")</string>
  <key>StandardErrorPath</key>
  <string>$(xml_escape "${LOG_FILE}")</string>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>30</integer>
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
  SAFE_WORK_DIR="${TARGET_HOME:-/}"
  if [ ! -d "${SAFE_WORK_DIR}" ]; then
    SAFE_WORK_DIR="/"
  fi

  if [ -n "${SUDO_FIXUP_USER}" ]; then
    SUDO_FIXUP_GROUP="$(id -gn "${SUDO_FIXUP_USER}" 2>/dev/null || printf '%s' "${SUDO_FIXUP_USER}")"
    chown -R "${SUDO_FIXUP_USER}:${SUDO_FIXUP_GROUP}" "${INSTALL_DIR}" "${STATE_DIR}" 2>/dev/null || true
  fi

  EXEC_CMD="$(systemd_quote "${INSTALL_DIR}/coda-daemon") --config $(systemd_quote "${CFG_PATH}") run"
  if [ -n "${P2P_BIND}" ]; then
    EXEC_CMD="${EXEC_CMD} --p2p-bind $(systemd_quote "${P2P_BIND}")"
  fi

  if [ "$(id -u)" -eq 0 ] && [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    SYS_UNIT="/etc/systemd/system/coda-daemon.service"
    SYS_USER_DIRECTIVES=""
    if [ -n "${SUDO_FIXUP_USER}" ]; then
      SUDO_FIXUP_GROUP="$(id -gn "${SUDO_FIXUP_USER}" 2>/dev/null || printf '%s' "${SUDO_FIXUP_USER}")"
      SYS_USER_DIRECTIVES="User=${SUDO_FIXUP_USER}
Group=${SUDO_FIXUP_GROUP}"
    fi
    cat > "${SYS_UNIT}" <<EOF
[Unit]
Description=Coda Zero-Knowledge Host Daemon (herdr-core bridge)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
${SYS_USER_DIRECTIVES}
WorkingDirectory=$(systemd_quote "${SAFE_WORK_DIR}")
Environment="HOME=${TARGET_HOME}"
Environment=$(systemd_quote "PATH=${INSTALL_DIR}:/usr/local/bin:/usr/bin:/bin")
Environment=$(systemd_quote "CODA_DAEMON_CONFIG=${CFG_PATH}")
ExecStart=${EXEC_CMD}
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
    if systemctl daemon-reload 2>/dev/null && systemctl enable --now coda-daemon 2>/dev/null; then
      systemctl restart coda-daemon 2>/dev/null || true
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
WorkingDirectory=$(systemd_quote "${SAFE_WORK_DIR}")
Environment=$(systemd_quote "PATH=${INSTALL_DIR}:/usr/local/bin:/usr/bin:/bin")
Environment=$(systemd_quote "CODA_DAEMON_CONFIG=${CFG_PATH}")
ExecStart=${EXEC_CMD}
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
EOF
    if systemctl --user daemon-reload 2>/dev/null && systemctl --user enable --now coda-daemon 2>/dev/null; then
      systemctl --user restart coda-daemon 2>/dev/null || true
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

if [ -n "${SUDO_FIXUP_USER}" ]; then
  SUDO_FIXUP_GROUP="$(id -gn "${SUDO_FIXUP_USER}" 2>/dev/null || printf '%s' "${SUDO_FIXUP_USER}")"
  chown -R "${SUDO_FIXUP_USER}:${SUDO_FIXUP_GROUP}" "${INSTALL_DIR}" "${STATE_DIR}" 2>/dev/null || true
fi

echo "==> Coda Host Daemon setup complete!"
