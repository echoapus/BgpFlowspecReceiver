#!/usr/bin/env bash
set -Eeuo pipefail

INSTALL_DIR="${INSTALL_DIR:-/opt/bgpx}"
WEB_PORT="${WEB_PORT:-}"
CREATE_SERVICE=""
SET_BIND_CAP=0
BINARY=""
CREATE_LINK=1
RUST_BOOTSTRAP=0
SERVICE_MANAGER=""
SERVICE_FILE=""
BUILD_USER=""
BUILD_CARGO=""
BUILD_DIR_TEMP=""
STAGED=""

cleanup() {
  [[ -z "${STAGED}" ]] || rm -f -- "${STAGED}"
  [[ -z "${BUILD_DIR_TEMP}" ]] || rm -rf -- "${BUILD_DIR_TEMP}"
}
trap cleanup EXIT

usage() {
  cat <<'EOF'
Usage: ./deploy.sh [options]
  --install-dir DIR       Install directory (default: /opt/bgpx)
  --binary FILE           Install a prebuilt bgpx executable without Cargo
  --web-port PORT         Web UI port (default: 8080)
  --service               Install and start a bgpx service (systemd or OpenRC)
  --no-service            Do not install a service or prompt
  --no-link               Do not create /usr/local/bin/bgpx
  --cap-net-bind-service  Allow the binary to bind TCP port 179
  --rust                  Update build packages and compile from source
  -h, --help              Show this help
EOF
}

require_coreutils() {
  if realpath -m -- / >/dev/null 2>&1 && mv --help 2>&1 | grep -- '-T' >/dev/null; then
    return
  fi
  if command -v apk >/dev/null && [[ "${EUID}" -eq 0 ]]; then
    apk add --no-cache coreutils
    realpath -m -- / >/dev/null 2>&1 && mv --help 2>&1 | grep -- '-T' >/dev/null && return
  fi
  echo "GNU coreutils is required (Alpine: apk add coreutils)" >&2
  exit 1
}

detect_service_manager() {
  if command -v systemctl >/dev/null && [[ -d /run/systemd/system ]]; then
    SERVICE_MANAGER=systemd
    SERVICE_FILE=/etc/systemd/system/bgpx.service
  elif command -v rc-service >/dev/null && command -v rc-update >/dev/null; then
    SERVICE_MANAGER=openrc
    SERVICE_FILE=/etc/init.d/bgpx
  elif command -v systemctl >/dev/null; then
    SERVICE_MANAGER=systemd
    SERVICE_FILE=/etc/systemd/system/bgpx.service
  else
    echo "--service requires systemd or OpenRC" >&2
    exit 1
  fi
}

shell_quote() {
  printf "'%s'" "${1//\'/\'\"\'\"\'}"
}

install_rust_build_packages() {
  [[ "${EUID}" -eq 0 ]] || { echo "--rust updates system packages; re-run with sudo or install build dependencies manually" >&2; exit 1; }
  if command -v apk >/dev/null; then
    apk update
    apk add --no-cache build-base ca-certificates curl
  elif command -v apt-get >/dev/null; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y build-essential ca-certificates curl
  elif command -v dnf >/dev/null; then
    dnf -y makecache
    dnf install -y gcc gcc-c++ make ca-certificates curl
  elif command -v yum >/dev/null; then
    yum -y makecache
    yum install -y gcc gcc-c++ make ca-certificates curl
  else
    echo "--rust supports apk, apt-get, dnf, or yum package managers" >&2
    exit 1
  fi
}

prepare_rust_build_user() {
  [[ "${EUID}" -eq 0 && -n "${SUDO_USER:-}" && "${SUDO_USER}" != root ]] || {
    echo "Source builds under sudo must be run from a non-root build user" >&2
    exit 1
  }
  command -v sudo >/dev/null || { echo "sudo is required for --rust" >&2; exit 1; }
  BUILD_USER="${SUDO_USER}"
}

find_cargo() {
  if [[ -n "${BUILD_USER}" ]]; then
    sudo -H -u "${BUILD_USER}" sh -c 'if [ -x "$HOME/.cargo/bin/cargo" ]; then printf "%s\n" "$HOME/.cargo/bin/cargo"; else command -v cargo; fi' || true
  else
    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
      printf '%s\n' "${HOME}/.cargo/bin/cargo"
    else
      command -v cargo || true
    fi
  fi
}

install_rustup() {
  if [[ -n "${BUILD_USER}" ]]; then
    sudo -H -u "${BUILD_USER}" sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable"
  else
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
  fi
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
    --rust) RUST_BOOTSTRAP=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

require_coreutils
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
  read -r -p "Install and enable bgpx service? [y/N]: " answer
  [[ "${answer}" =~ ^[Yy] ]] && CREATE_SERVICE=1 || CREATE_SERVICE=0
fi
CREATE_SERVICE="${CREATE_SERVICE:-0}"
if [[ "${CREATE_SERVICE}" -eq 1 || "${SET_BIND_CAP}" -eq 1 ]]; then
  [[ "${EUID}" -eq 0 ]] || { echo "Service/capability installation requires root" >&2; exit 1; }
fi
if [[ "${CREATE_SERVICE}" -eq 1 ]]; then
  detect_service_manager
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
  if [[ "${EUID}" -eq 0 && -n "${SUDO_USER:-}" && "${SUDO_USER}" != root ]]; then
    prepare_rust_build_user
  elif [[ "${RUST_BOOTSTRAP}" -eq 1 ]]; then
    prepare_rust_build_user
  fi
  # Cargo needs a C linker (and libgcc on Alpine); install them when missing.
  if [[ "${RUST_BOOTSTRAP}" -eq 1 ]] || { [[ "${EUID}" -eq 0 ]] && ! command -v cc >/dev/null; }; then
    install_rust_build_packages
  fi
  BUILD_CARGO="$(find_cargo)"
  if [[ -z "${BUILD_CARGO}" ]]; then
    command -v curl >/dev/null || { echo "Install curl (to fetch Rust) or pass --binary FILE" >&2; exit 1; }
    echo "Rust/Cargo not found; installing via rustup..." >&2
    install_rustup
    BUILD_CARGO="$(find_cargo)"
  fi
  [[ -n "${BUILD_CARGO}" ]] || { echo "Rust installation failed; install manually or pass --binary FILE" >&2; exit 1; }
  if [[ -n "${BUILD_USER}" && -z "${CARGO_TARGET_DIR:-}" ]]; then
    BUILD_DIR_TEMP="$(mktemp -d /tmp/bgpx-build.XXXXXX)"
    chown "${BUILD_USER}" "${BUILD_DIR_TEMP}"
    BUILD_DIR="${BUILD_DIR_TEMP}"
  else
    BUILD_DIR="$(realpath -m -- "${CARGO_TARGET_DIR:-${ROOT}/target}")"
  fi
  if [[ -n "${BUILD_USER}" ]]; then
    sudo -H -u "${BUILD_USER}" "${BUILD_CARGO}" build --locked --release --manifest-path "${ROOT}/Cargo.toml" --target-dir "${BUILD_DIR}"
  else
    "${BUILD_CARGO}" build --locked --release --manifest-path "${ROOT}/Cargo.toml" --target-dir "${BUILD_DIR}"
  fi
  BINARY="${BUILD_DIR}/release/bgpx"
fi
[[ -x "${BINARY}" ]] || { echo "Executable not found: ${BINARY}" >&2; exit 1; }
"${BINARY}" --version
install -d "${INSTALL_DIR}/bin"
STAGED="$(mktemp "${INSTALL_DIR}/bin/.bgpx.XXXXXX")"
install -m 755 "${BINARY}" "${STAGED}"
if [[ "${SET_BIND_CAP}" -eq 1 ]]; then
  setcap cap_net_bind_service+ep "${STAGED}"
fi
mv -fT -- "${STAGED}" "${INSTALL_DIR}/bin/bgpx"
if [[ -d "${ROOT}/web/locales" ]]; then
  # .bgpx-locales lists files shipped by the previous install: remove ones that
  # are no longer shipped, but never touch locale files the operator added.
  LOCALE_DIR="${INSTALL_DIR}/locales"
  LOCALE_MANIFEST="${INSTALL_DIR}/.bgpx-locales"
  install -d "${LOCALE_DIR}"
  shopt -s nullglob
  SHIPPED=()
  for f in "${ROOT}/web/locales/"*.json; do SHIPPED+=("$(basename -- "${f}")"); done
  shopt -u nullglob
  if [[ -f "${LOCALE_MANIFEST}" ]]; then
    while IFS= read -r old; do
      [[ "${old}" =~ ^[A-Za-z0-9_-]+\.json$ ]] || continue
      [[ " ${SHIPPED[*]} " == *" ${old} "* ]] || rm -f -- "${LOCALE_DIR}/${old}"
    done <"${LOCALE_MANIFEST}"
  fi
  if (( ${#SHIPPED[@]} )); then
    install -m 644 "${SHIPPED[@]/#/${ROOT}/web/locales/}" "${LOCALE_DIR}/"
  fi
  printf '%s\n' "${SHIPPED[@]}" >"${LOCALE_MANIFEST}"
fi
install -m 755 "${ROOT}/uninstall.sh" "${INSTALL_DIR}/uninstall.sh"
printf '%s\n' 'bgpx-native-v1' >"${INSTALL_DIR}/.bgpx-install"
if [[ "${EUID}" -eq 0 && "${CREATE_LINK}" -eq 1 ]]; then
  ln -sfn "${INSTALL_DIR}/bin/bgpx" /usr/local/bin/bgpx
fi
if [[ "${CREATE_SERVICE}" -eq 1 ]]; then
  if [[ "${SERVICE_MANAGER}" == systemd ]]; then
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
  else
    OPENRC_DIR="$(shell_quote "${INSTALL_DIR}")"
    cat >"${SERVICE_FILE}" <<EOF
#!/sbin/openrc-run
# bgpx-install-dir: ${INSTALL_DIR}
description="BGP Unicast and FlowSpec Receiver"
supervisor=supervise-daemon
command=${OPENRC_DIR}/bin/bgpx
command_args="--host 0.0.0.0 --port ${WEB_PORT}"
directory=${OPENRC_DIR}
respawn_delay=5
EOF
    chmod 755 "${SERVICE_FILE}"
    rc-update add bgpx default
    rc-service bgpx restart
    rc-service bgpx status
  fi
fi
echo "Installed: ${INSTALL_DIR}/bin/bgpx"
echo "Web UI: http://localhost:${WEB_PORT}"
echo "Manual start: ${INSTALL_DIR}/bin/bgpx --port ${WEB_PORT}"
printf 'Uninstall: %q --install-dir %q\n' "${INSTALL_DIR}/uninstall.sh" "${INSTALL_DIR}"
