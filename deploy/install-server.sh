#!/usr/bin/env sh
# Coda Coordinator & Web Console One-Click Production Installer (Linux x86_64 / ARM64)
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-server.sh | sudo sh -s -- \
#     --public-url http://YOUR_SERVER_IP:8080 \
#     --relay-secret YOUR_SHARED_SECRET

set -eu

REPO="Annihilater/coda"
VERSION="${CODA_VERSION:-latest}"
ENV_SRC=""
ARG_PUBLIC_URL=""
ARG_RELAY_SECRET=""
ARG_DB_URL=""
ARG_HTTP_BIND=""
ARG_RAW_TCP_BIND=""

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --env-file) ENV_SRC="$2"; shift 2 ;;
    --public-url) ARG_PUBLIC_URL="$2"; shift 2 ;;
    --relay-secret) ARG_RELAY_SECRET="$2"; shift 2 ;;
    --database-url) ARG_DB_URL="$2"; shift 2 ;;
    --http-bind) ARG_HTTP_BIND="$2"; shift 2 ;;
    --raw-tcp-bind) ARG_RAW_TCP_BIND="$2"; shift 2 ;;
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

PUBLIC_URL="${ARG_PUBLIC_URL:-${CODA_PUBLIC_URL:-http://127.0.0.1:8080}}"
RELAY_SECRET="${ARG_RELAY_SECRET:-${CODA_RELAY_SECRET:-coda-default-relay-secret-change-me}}"
DB_URL="${ARG_DB_URL:-${DATABASE_URL:-postgres://coda:coda_prod_pass@127.0.0.1:5432/coda_prod}}"
HTTP_BIND="${ARG_HTTP_BIND:-${CODA_HTTP_BIND:-0.0.0.0:${CODA_HTTP_PORT:-8080}}}"
RAW_TCP_BIND="${ARG_RAW_TCP_BIND:-${CODA_RAW_TCP_BIND:-0.0.0.0:${CODA_RAW_TCP_PORT:-24443}}}"
REGION_LABEL="${CODA_REGION_LABEL:-CN-BGP-NoICP}"

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

echo "==> [1/3] Downloading Coda Server ${VERSION} (${PKG})..."
TMP_DIR="$(mktemp -d)"
curl -fsSL "https://github.com/${REPO}/releases/download/${VERSION}/${PKG}" -o "${TMP_DIR}/${PKG}"
tar -C "${TMP_DIR}" -xzf "${TMP_DIR}/${PKG}"

mkdir -p /opt/coda /etc/coda
cp "${TMP_DIR}/${DIR}/coda-server" /usr/local/bin/coda-server
chmod +x /usr/local/bin/coda-server
rm -rf /opt/coda/web-dist
cp -r "${TMP_DIR}/${DIR}/web-dist" /opt/coda/web-dist
rm -rf "${TMP_DIR}"

echo "==> [2/3] Writing unified /etc/coda/.env (0600) & systemd unit /etc/systemd/system/coda-server.service..."
cat > /etc/coda/.env <<EOF
CODA_RELAY_SECRET=${RELAY_SECRET}
CODA_PUBLIC_URL=${PUBLIC_URL}
CODA_HTTP_BIND=${HTTP_BIND}
CODA_RAW_TCP_BIND=${RAW_TCP_BIND}
CODA_REGION_LABEL=${REGION_LABEL}
DATABASE_URL=${DB_URL}
EOF
chmod 600 /etc/coda/.env

cat > /etc/systemd/system/coda-server.service <<'EOF'
[Unit]
Description=Coda Zero-Knowledge Coordinator & Web Console
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/coda
EnvironmentFile=/etc/coda/.env
ExecStart=/usr/local/bin/coda-server --web-dist /opt/coda/web-dist
Restart=always
RestartSec=2
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF

echo "==> [3/3] Starting coda-server service..."
systemctl daemon-reload
systemctl enable --now coda-server

echo "==> Coda Coordinator is running!"
echo "    Web Console & API : ${PUBLIC_URL}"
echo "    No-ICP Raw TCP    : ${RAW_TCP_BIND}"
