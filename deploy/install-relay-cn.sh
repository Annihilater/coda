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
  echo "==> Loading base configuration from ${ENV_SRC}..."
  set -a
  # shellcheck disable=SC1090
  . "${ENV_SRC}"
  set +a
fi

RELAY_SECRET="${ARG_RELAY_SECRET:-${CODA_RELAY_SECRET:-coda-default-relay-secret-change-me}}"
if [ -n "${ARG_PORT}" ]; then
  PORT="${ARG_PORT}"
elif [ -n "${CODA_RAW_TCP_BIND:-}" ]; then
  PORT="${CODA_RAW_TCP_BIND##*:}"
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
  *) echo "Unsupported architecture: ${ARCH}"; exit 1 ;;
esac
PKG="${DIR}-${VERSION}.tar.gz"

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

echo "==> [2/4] Downloading Coda Relay ${VERSION} (${PKG})..."
TMP_DIR="$(mktemp -d)"
curl -fsSL "https://github.com/${REPO}/releases/download/${VERSION}/${PKG}" -o "${TMP_DIR}/${PKG}"
tar -C "${TMP_DIR}" -xzf "${TMP_DIR}/${PKG}"
mkdir -p /opt/coda /etc/coda
cp "${TMP_DIR}/${DIR}/coda-server" /usr/local/bin/coda-server
chmod +x /usr/local/bin/coda-server
rm -rf "${TMP_DIR}"

echo "==> [3/4] Writing /etc/coda/.env (0600) & systemd unit /etc/systemd/system/coda-relay-cn.service..."
cat > /etc/coda/.env <<EOF
CODA_RELAY_SECRET=${RELAY_SECRET}
CODA_RAW_TCP_BIND=0.0.0.0:${PORT}
CODA_REGION_LABEL=${REGION}
EOF
chmod 600 /etc/coda/.env

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
