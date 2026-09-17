#!/usr/bin/env bash
set -Eeuo pipefail

INSTALL_DIR="${INSTALL_DIR:-/opt/bgpx}"
WEB_PORT="${WEB_PORT:-}"
CREATE_SERVICE=""
SET_BIND_CAP=0
BINARY=""
CREATE_LINK=1
SERVICE_FILE=/etc/systemd/system/bgpx.service

usage() {
  cat <<'EOF'
Usage: ./deploy.sh [options]
  --install-dir DIR       Install directory (default: /opt/bgpx)
  --binary FILE           Install a prebuilt bgpx executable without Cargo
  --web-port PORT         Web UI port (default: 8080)
  --service               Install and start bgpx.service
  --no-service            Do not install a service or prompt
  --no-link               Do not create /usr/local/bin/bgpx
  --cap-net-bind-service  Allow the binary to bind TCP port 179
  --rust                  Accepted for compatibility; Rust is always used
  -h, --help              Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir) INSTALL_DIR="${2:?missing directory}"; shift 2 ;;
    --binary) BINARY="${2:?missing binary}"; shift 2 ;;
    --web-port) WEB_PORT="${2:?missing port}"; shift 2 ;;
    --service) CREATE_SERVICE=1; shift ;;
    --no-service) CREATE_SERVICE=0; shift ;;
    --no-link) CREATE_LINK=0; shift ;;
    --cap-net-bind-service) SET_BIND_CAP=1; shift ;;
    --rust) shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "${INSTALL_DIR}" ]] || { echo "Install directory cannot be empty" >&2; exit 1; }
INSTALL_DIR="$(realpath -m -- "${INSTALL_DIR}")"
case "${INSTALL_DIR}" in
  /|/opt|/usr|/usr/bin|/usr/sbin|/usr/lib|/usr/lib64|/usr/local|/usr/local/bin|/var|/home|/root|/tmp|/etc|/bin|/sbin|/lib|/lib64|/boot|/dev|/proc|/sys|/run)
    echo "Refusing unsafe install directory: ${INSTALL_DIR}" >&2; exit 1 ;;
esac
[[ "${INSTALL_DIR}" != *$'\n'* && "${INSTALL_DIR}" != *$'\r'* ]] || { echo "Invalid install directory" >&2; exit 1; }
if [[ -z "${WEB_PORT}" && -t 0 ]]; then
  read -r -p "Web UI bind port [8080]: " WEB_PORT
fi
WEB_PORT="${WEB_PORT:-8080}"
if [[ ! "${WEB_PORT}" =~ ^[0-9]{1,5}$ ]] || (( 10#${WEB_PORT} < 1 || 10#${WEB_PORT} > 65535 )); then
  echo "Invalid web port: ${WEB_PORT}" >&2; exit 1
fi
WEB_PORT="$((10#${WEB_PORT}))"
if [[ -z "${CREATE_SERVICE}" && "${EUID}" -eq 0 && -t 0 ]]; then
  read -r -p "Install and enable systemd service? [y/N]: " answer
  [[ "${answer}" =~ ^[Yy] ]] && CREATE_SERVICE=1 || CREATE_SERVICE=0
fi
CREATE_SERVICE="${CREATE_SERVICE:-0}"
if [[ "${CREATE_SERVICE}" -eq 1 || "${SET_BIND_CAP}" -eq 1 ]]; then
  [[ "${EUID}" -eq 0 ]] || { echo "Service/capability installation requires root" >&2; exit 1; }
fi
if [[ "${CREATE_SERVICE}" -eq 1 ]]; then
  command -v systemctl >/dev/null || { echo "systemctl is required" >&2; exit 1; }
  SERVICE_DIR="${INSTALL_DIR//%/%%}"
  SERVICE_DIR="${SERVICE_DIR//\\/\\\\}"
  SERVICE_DIR="${SERVICE_DIR//\"/\\\"}"
  if [[ -e "${SERVICE_FILE}" || -L "${SERVICE_FILE}" ]]; then
    OWNED=0
    while IFS= read -r line; do
      if [[ "${line}" == "# bgpx-install-dir: ${INSTALL_DIR}" || "${line}" == "ExecStart=${INSTALL_DIR}/venv/bin/bgpx "* || "${line}" == "ExecStart=\"${SERVICE_DIR}/bin/bgpx\" "* ]]; then OWNED=1; fi
    done <"${SERVICE_FILE}"
    [[ "${OWNED}" -eq 1 ]] || { echo "Existing bgpx.service belongs to another or unrecognized installation" >&2; exit 1; }
  fi
fi
if [[ "${SET_BIND_CAP}" -eq 1 ]]; then
  command -v setcap >/dev/null || { echo "Install libcap tools first" >&2; exit 1; }
fi

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "${ROOT}/Cargo.toml" && ( "${ROOT}" == "${INSTALL_DIR}" || "${ROOT}" == "${INSTALL_DIR}/"* ) ]]; then
  echo "Install directory must not contain the source checkout" >&2; exit 1
fi
if [[ "${CREATE_LINK}" -eq 1 && "${EUID}" -eq 0 && ( -e /usr/local/bin/bgpx || -L /usr/local/bin/bgpx ) ]]; then
  LINK_TARGET="$(realpath -m -- /usr/local/bin/bgpx)"
  if [[ ! -L /usr/local/bin/bgpx || ( "${LINK_TARGET}" != "${INSTALL_DIR}/bin/bgpx" && "${LINK_TARGET}" != "${INSTALL_DIR}/venv/bin/bgpx" ) ]]; then
    echo "Existing /usr/local/bin/bgpx belongs to another installation; use --no-link" >&2; exit 1
  fi
fi
if [[ -z "${BINARY}" ]]; then
  if ! command -v cargo >/dev/null; then
    command -v curl >/dev/null || { echo "Install curl (to fetch Rust) or pass --binary FILE" >&2; exit 1; }
    echo "Rust/Cargo not found; installing via rustup..." >&2
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
    # shellcheck disable=SC1091
    source "${HOME}/.cargo/env"
  fi
  command -v cargo >/dev/null || { echo "Rust installation failed; install manually or pass --binary FILE" >&2; exit 1; }
  BUILD_DIR="$(realpath -m -- "${CARGO_TARGET_DIR:-${ROOT}/target}")"
  cargo build --locked --release --manifest-path "${ROOT}/Cargo.toml" --target-dir "${BUILD_DIR}"
  BINARY="${BUILD_DIR}/release/bgpx"
fi
[[ -x "${BINARY}" ]] || { echo "Executable not found: ${BINARY}" >&2; exit 1; }
"${BINARY}" --version
install -d "${INSTALL_DIR}/bin"
STAGED="$(mktemp "${INSTALL_DIR}/bin/.bgpx.XXXXXX")"
trap 'rm -f -- "${STAGED}"' EXIT
install -m 755 "${BINARY}" "${STAGED}"
if [[ "${SET_BIND_CAP}" -eq 1 ]]; then
  setcap cap_net_bind_service+ep "${STAGED}"
fi
mv -fT -- "${STAGED}" "${INSTALL_DIR}/bin/bgpx"
install -m 755 "${ROOT}/uninstall.sh" "${INSTALL_DIR}/uninstall.sh"
printf '%s\n' 'bgpx-native-v1' >"${INSTALL_DIR}/.bgpx-install"
if [[ "${EUID}" -eq 0 && "${CREATE_LINK}" -eq 1 ]]; then
  ln -sfn "${INSTALL_DIR}/bin/bgpx" /usr/local/bin/bgpx
fi
if [[ "${CREATE_SERVICE}" -eq 1 ]]; then
  cat >"${SERVICE_FILE}" <<EOF
# bgpx-install-dir: ${INSTALL_DIR}
[Unit]
Description=BGP Unicast and FlowSpec Receiver
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${INSTALL_DIR//%/%%}
ExecStart="${SERVICE_DIR}/bin/bgpx" --host 0.0.0.0 --port ${WEB_PORT}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable bgpx.service
  systemctl restart bgpx.service
  systemctl --no-pager --full --lines=20 status bgpx.service
fi
echo "Installed: ${INSTALL_DIR}/bin/bgpx"
echo "Web UI: http://localhost:${WEB_PORT}"
echo "Manual start: ${INSTALL_DIR}/bin/bgpx --port ${WEB_PORT}"
printf 'Uninstall: %q --install-dir %q\n' "${INSTALL_DIR}/uninstall.sh" "${INSTALL_DIR}"
