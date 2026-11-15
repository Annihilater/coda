#!/usr/bin/env sh
# Coda Domestic Cloud (Aliyun / Tencent Cloud) No-ICP Raw TCP Relay One-Click Installer
# Zero Database, Zero HTTP, Zero TLS SNI — Binds ONLY Port 24443
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-relay-cn.sh | sudo sh -s -- \
#     --relay-secret YOUR_SHARED_SECRET \
#     --port 24443

set -eu

REPO="Annihilater/coda"
VERSION="${CODA_VERSION:-latest}"
ENV_SRC=""
ARG_RELAY_SECRET=""
ARG_PORT=""
ARG_REGION=""

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

quote_env_val() {
  esc="$(printf '%s' "${1:-}" | tr -d '\r\n' | sed 's/\\/\\\\/g; s/"/\\"/g')"
  printf '"%s"' "${esc}"
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
    echo "[ERROR] Missing sha256sum / shasum / openssl for checksum verification" >&2
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
    echo "[ERROR] Failed downloading SHA256SUMS-${ver_tag}.txt (or SHA256SUMS.txt); aborting unverified install." >&2
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
    echo "[ERROR] SHA256SUMS does not contain an entry for ${asset_name}." >&2
    return 1
  fi
  actual_hash="$(compute_sha256 "${local_file}")" || return 1
  if [ "${actual_hash}" != "${expected_hash}" ]; then
    echo "[ERROR] SHA256 mismatch for ${asset_name}: expected=${expected_hash} actual=${actual_hash}" >&2
    return 1
  fi
  echo "    [OK] SHA256 verified (${asset_name}: ${actual_hash})"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --env-file) ENV_SRC="$2"; shift 2 ;;
    --relay-secret) ARG_RELAY_SECRET="$2"; shift 2 ;;
    --port) ARG_PORT="$2"; shift 2 ;;
    --region) ARG_REGION="$2"; shift 2 ;;
    *) shift 1 ;;
  esac
done

if [ -z "${ENV_SRC}" ] && [ -f "./.env" ]; then
  ENV_SRC="./.env"
elif [ -z "${ENV_SRC}" ] && [ -f "/etc/coda/.env" ]; then
  ENV_SRC="/etc/coda/.env"
fi

if [ -n "${ENV_SRC}" ] && [ -f "${ENV_SRC}" ]; then
  echo "==> Loading base configuration safely from ${ENV_SRC}..."
  load_env_file "${ENV_SRC}"
fi

RELAY_SECRET="${ARG_RELAY_SECRET:-${CODA_RELAY_SECRET:-}}"
case "${RELAY_SECRET}" in
  ""|*change-me*|*change_me*)
    echo "[ERROR] Refusing to install relay without a real --relay-secret / CODA_RELAY_SECRET." >&2
    exit 1
    ;;
esac

if [ -n "${ARG_PORT}" ]; then
  PORT="${ARG_PORT}"
elif [ -n "${CODA_RAW_TCP_BIND:-}" ]; then
  PORT="${CODA_RAW_TCP_BIND##*:}"
elif [ -n "${CODA_RAW_TCP_PORT:-}" ]; then
  PORT="${CODA_RAW_TCP_PORT}"
else
  PORT="24443"
fi
REGION="${ARG_REGION:-${CODA_REGION_LABEL:-CN-BGP-NoICP}}"

if [ "${VERSION}" = "latest" ]; then
  EFFECTIVE_URL="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")"
  VERSION="${EFFECTIVE_URL##*/}"
fi

ARCH="$(uname -m)"
case "${ARCH}" in
  x86_64|amd64) DIR="coda-server-linux-x86_64" ;;
  aarch64|arm64) DIR="coda-server-linux-aarch64" ;;
  *) echo "Unsupported architecture: ${ARCH}" >&2; exit 1 ;;
esac
PKG="${DIR}-${VERSION}.tar.gz"
REL_BASE="https://github.com/${REPO}/releases/download/${VERSION}"

echo "==> [1/4] Optimizing Linux Kernel TCP BBR & Low-Latency Socket Queue..."
if [ -d /etc/sysctl.d ]; then
  cat > /etc/sysctl.d/99-coda-relay.conf <<EOF
net.core.default_qdisc = fq
net.ipv4.tcp_congestion_control = bbr
net.ipv4.tcp_fastopen = 3
net.ipv4.tcp_notsent_lowat = 16384
EOF
  sysctl --system >/dev/null 2>&1 || true
fi

echo "==> [2/4] Downloading & verifying Coda Relay ${VERSION} (${PKG})..."
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT HUP INT TERM
curl -fsSL "${REL_BASE}/${PKG}" -o "${TMP_DIR}/${PKG}"
verify_release_sha256 "${REL_BASE}" "${VERSION}" "${PKG}" "${TMP_DIR}/${PKG}"
tar -C "${TMP_DIR}" -xzf "${TMP_DIR}/${PKG}"
mkdir -p /opt/coda /etc/coda
chmod 700 /etc/coda
cp "${TMP_DIR}/${DIR}/coda-server" /usr/local/bin/coda-server
chmod +x /usr/local/bin/coda-server

echo "==> [3/4] Writing /etc/coda/.env (0600) & systemd unit /etc/systemd/system/coda-relay-cn.service..."
(
  umask 077
  cat > /etc/coda/.env <<EOF
CODA_RELAY_SECRET=$(quote_env_val "${RELAY_SECRET}")
CODA_RAW_TCP_BIND=$(quote_env_val "0.0.0.0:${PORT}")
CODA_REGION_LABEL=$(quote_env_val "${REGION}")
EOF
  chmod 600 /etc/coda/.env
)

cat > /etc/systemd/system/coda-relay-cn.service <<'EOF'
[Unit]
Description=Coda Stateless Domestic Cloud No-ICP Relay Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/coda
EnvironmentFile=/etc/coda/.env
ExecStart=/usr/local/bin/coda-server --relay-only
Restart=always
RestartSec=2
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF

echo "==> [4/4] Starting coda-relay-cn service..."
systemctl daemon-reload
systemctl enable --now coda-relay-cn

echo "==> Domestic No-ICP Raw TCP Relay active on 0.0.0.0:${PORT} (${REGION})!"
