#!/usr/bin/env sh
# Coda Coordinator & Web Console One-Click Production Installer (Linux x86_64 / ARM64)
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-server.sh | sudo sh -s -- \
#     --public-url http://YOUR_SERVER_IP:8080 \
#     --relay-secret YOUR_SHARED_SECRET

set -eu

REPO="Annihilater/coda"
PUBLIC_URL="http://127.0.0.1:8080"
RELAY_SECRET="coda-default-relay-secret-change-me"
DB_URL="postgres://coda:coda_prod_pass@127.0.0.1:5432/coda_prod"
HTTP_BIND="0.0.0.0:8080"
RAW_TCP_BIND="0.0.0.0:24443"

while [ $# -gt 0 ]; do
  case "$1" in
    --public-url) PUBLIC_URL="$2"; shift 2 ;;
    --relay-secret) RELAY_SECRET="$2"; shift 2 ;;
    --database-url) DB_URL="$2"; shift 2 ;;
    --http-bind) HTTP_BIND="$2"; shift 2 ;;
    --raw-tcp-bind) RAW_TCP_BIND="$2"; shift 2 ;;
    *) shift 1 ;;
  esac
done

ARCH="$(uname -m)"
case "${ARCH}" in
  x86_64|amd64) PKG="coda-server-linux-x86_64.tar.gz"; DIR="coda-server-linux-x86_64" ;;
  aarch64|arm64) PKG="coda-server-linux-aarch64.tar.gz"; DIR="coda-server-linux-aarch64" ;;
  *) echo "Unsupported architecture: ${ARCH}"; exit 1 ;;
esac

echo "==> [1/3] Downloading latest Coda Server release (${PKG})..."
TMP_DIR="$(mktemp -d)"
curl -fsSL "https://github.com/${REPO}/releases/latest/download/${PKG}" -o "${TMP_DIR}/${PKG}"
tar -C "${TMP_DIR}" -xzf "${TMP_DIR}/${PKG}"

mkdir -p /opt/coda
cp "${TMP_DIR}/${DIR}/coda-server" /usr/local/bin/coda-server
chmod +x /usr/local/bin/coda-server
rm -rf /opt/coda/web-dist
cp -r "${TMP_DIR}/${DIR}/web-dist" /opt/coda/web-dist
rm -rf "${TMP_DIR}"

echo "==> [2/3] Writing systemd unit /etc/systemd/system/coda-server.service..."
cat > /etc/systemd/system/coda-server.service <<EOF
[Unit]
Description=Coda Zero-Knowledge Coordinator & Web Console
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
Environment="DATABASE_URL=${DB_URL}"
Environment="CODA_RELAY_SECRET=${RELAY_SECRET}"
Environment="CODA_PUBLIC_URL=${PUBLIC_URL}"
ExecStart=/usr/local/bin/coda-server --http-bind ${HTTP_BIND} --raw-tcp-bind ${RAW_TCP_BIND} --web-dist /opt/coda/web-dist
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
