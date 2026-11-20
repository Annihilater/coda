#!/usr/bin/env sh
# Coda Coordinator & Web Console One-Click Production Installer (Linux x86_64 / ARM64)
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Annihilater/coda/main/deploy/install-server.sh | sudo sh -s -- \
#     --public-url https://codahub.dev \
#     --relay-secret YOUR_SHARED_SECRET \
#     --database-url postgres://coda:YOUR_STRONG_PASSWORD@127.0.0.1:5432/coda_prod

set -eu

REPO="Annihilater/coda"
VERSION="${CODA_VERSION:-latest}"
ENV_SRC=""
ARG_PUBLIC_URL=""
ARG_RELAY_SECRET=""
ARG_DB_URL=""
ARG_HTTP_BIND=""
ARG_RAW_TCP_BIND=""
ARG_OPEN_REG=""

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
    --public-url) ARG_PUBLIC_URL="$2"; shift 2 ;;
    --relay-secret) ARG_RELAY_SECRET="$2"; shift 2 ;;
    --database-url) ARG_DB_URL="$2"; shift 2 ;;
    --http-bind) ARG_HTTP_BIND="$2"; shift 2 ;;
    --raw-tcp-bind) ARG_RAW_TCP_BIND="$2"; shift 2 ;;
    --open-registration) ARG_OPEN_REG="$2"; shift 2 ;;
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

PUBLIC_URL="${ARG_PUBLIC_URL:-${CODA_PUBLIC_URL:-}}"
RELAY_SECRET="${ARG_RELAY_SECRET:-${CODA_RELAY_SECRET:-}}"
DB_URL="${ARG_DB_URL:-${DATABASE_URL:-}}"
HTTP_BIND="${ARG_HTTP_BIND:-${CODA_HTTP_BIND:-0.0.0.0:${CODA_HTTP_PORT:-8080}}}"
RAW_TCP_BIND="${ARG_RAW_TCP_BIND:-${CODA_RAW_TCP_BIND:-0.0.0.0:${CODA_RAW_TCP_PORT:-24443}}}"
REGION_LABEL="${CODA_REGION_LABEL:-CN-BGP-NoICP}"
OPEN_REGISTRATION="${ARG_OPEN_REG:-${CODA_OPEN_REGISTRATION:-false}}"
GITHUB_REPO="${CODA_GITHUB_REPO:-Annihilater/coda}"
IOS_BETA_URL="${CODA_IOS_BETA_URL:-}"
RAW_TCP_PUBLIC_ENDPOINT="${CODA_RAW_TCP_PUBLIC_ENDPOINT:-}"
TRUST_PROXY_HEADERS="${CODA_TRUST_PROXY_HEADERS:-auto}"
ALLOWED_ORIGINS="${CODA_ALLOWED_ORIGINS:-}"
RELEASE_VERSION="${CODA_RELEASE_VERSION:-}"

ensure_coda_user() {
  if ! getent group coda >/dev/null 2>&1; then
    if command -v groupadd >/dev/null 2>&1; then
      groupadd --system coda 2>/dev/null || true
    elif command -v addgroup >/dev/null 2>&1; then
      addgroup --system coda 2>/dev/null || true
    fi
  fi
  if ! id -u coda >/dev/null 2>&1; then
    if command -v useradd >/dev/null 2>&1; then
      useradd --system --gid coda --home-dir /opt/coda --no-create-home --shell /usr/sbin/nologin coda 2>/dev/null || true
    elif command -v adduser >/dev/null 2>&1; then
      adduser --system --ingroup coda --home /opt/coda --no-create-home --shell /usr/sbin/nologin coda 2>/dev/null || true
    fi
  fi
}

atomic_install_bin() {
  src="$1"
  dst="$2"
  dst_dir="$(dirname "${dst}")"
  mkdir -p "${dst_dir}"
  tmp_bin="${dst}.tmp.$$"
  cp "${src}" "${tmp_bin}"
  chmod 0755 "${tmp_bin}"
  mv -f "${tmp_bin}" "${dst}"
}

case "${RELAY_SECRET}" in
  ""|*change-me*|*change_me*)
    echo "[ERROR] Refusing to install without a real --relay-secret / CODA_RELAY_SECRET." >&2
    exit 1
    ;;
esac
case "${DB_URL}" in
  ""|*coda_prod_pass*|*change-me*|*change_me*)
    echo "[ERROR] Refusing to install without a real --database-url / DATABASE_URL." >&2
    exit 1
    ;;
esac

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

echo "==> [1/3] Downloading & verifying Coda Server ${VERSION} (${PKG})..."
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT HUP INT TERM
curl -fsSL "${REL_BASE}/${PKG}" -o "${TMP_DIR}/${PKG}"
verify_release_sha256 "${REL_BASE}" "${VERSION}" "${PKG}" "${TMP_DIR}/${PKG}"
tar -C "${TMP_DIR}" -xzf "${TMP_DIR}/${PKG}"

ensure_coda_user
mkdir -p /opt/coda /etc/coda /var/lib/coda
chmod 700 /etc/coda
atomic_install_bin "${TMP_DIR}/${DIR}/coda-server" /usr/local/bin/coda-server
rm -rf /opt/coda/web-dist /opt/coda/migrations
cp -r "${TMP_DIR}/${DIR}/web-dist" /opt/coda/web-dist
if [ -d "${TMP_DIR}/${DIR}/migrations" ]; then
  cp -r "${TMP_DIR}/${DIR}/migrations" /opt/coda/migrations
fi
chown -R coda:coda /opt/coda /var/lib/coda 2>/dev/null || true

echo "==> [2/3] Writing unified /etc/coda/.env (0600) & hardened systemd unit /etc/systemd/system/coda-server.service..."
(
  umask 077
  cat > /etc/coda/.env <<EOF
CODA_RELAY_SECRET=$(quote_env_val "${RELAY_SECRET}")
CODA_PUBLIC_URL=$(quote_env_val "${PUBLIC_URL}")
CODA_HTTP_BIND=$(quote_env_val "${HTTP_BIND}")
CODA_RAW_TCP_BIND=$(quote_env_val "${RAW_TCP_BIND}")
CODA_REGION_LABEL=$(quote_env_val "${REGION_LABEL}")
CODA_OPEN_REGISTRATION=$(quote_env_val "${OPEN_REGISTRATION}")
CODA_GITHUB_REPO=$(quote_env_val "${GITHUB_REPO}")
CODA_RELEASE_VERSION=$(quote_env_val "${RELEASE_VERSION}")
CODA_IOS_BETA_URL=$(quote_env_val "${IOS_BETA_URL}")
CODA_RAW_TCP_PUBLIC_ENDPOINT=$(quote_env_val "${RAW_TCP_PUBLIC_ENDPOINT}")
CODA_TRUST_PROXY_HEADERS=$(quote_env_val "${TRUST_PROXY_HEADERS}")
CODA_ALLOWED_ORIGINS=$(quote_env_val "${ALLOWED_ORIGINS}")
DATABASE_URL=$(quote_env_val "${DB_URL}")
EOF
  chmod 600 /etc/coda/.env
)

cat > /etc/systemd/system/coda-server.service <<'EOF'
[Unit]
Description=Coda Zero-Knowledge Coordinator & Web Console
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
Type=simple
User=coda
Group=coda
WorkingDirectory=/opt/coda
EnvironmentFile=/etc/coda/.env
ExecStart=/usr/local/bin/coda-server --web-dist /opt/coda/web-dist
Restart=always
RestartSec=2
LimitNOFILE=1048576
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectControlGroups=true
RestrictSUIDSGID=true
StateDirectory=coda
ReadWritePaths=/var/lib/coda

[Install]
WantedBy=multi-user.target
EOF

echo "==> [3/3] Starting coda-server service..."
systemctl daemon-reload
systemctl enable --now coda-server
systemctl restart coda-server

echo "==> Coda Coordinator is running!"
echo "    Web Console & API : ${PUBLIC_URL:-<auto-derived from Host / reverse proxy>}"
echo "    No-ICP Raw TCP    : ${RAW_TCP_BIND}"
