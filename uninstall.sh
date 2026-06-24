#!/usr/bin/env bash
set -Eeuo pipefail

APP_NAME="bgpx"
INSTALL_DIR="${INSTALL_DIR:-/opt/bgpx}"
BIN_LINK="/usr/local/bin/${APP_NAME}"
SERVICE_FILE="/etc/systemd/system/${APP_NAME}.service"
FORCE=0
KEEP_DATA=0
SRC_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<'EOF'
Usage: ./uninstall.sh [options]

Remove a bgpx installation created by deploy.sh.

Options:
  --install-dir DIR   Install location to remove (default: /opt/bgpx)
  --force             Do not prompt for confirmation
  --keep-data         Keep the install directory; remove service and command link only
  -h, --help          Show this help

Environment:
  INSTALL_DIR=/opt/bgpx
EOF
}

confirm() {
  local answer
  if [[ "${FORCE}" -eq 1 ]]; then
    return
  fi
  if [[ ! -t 0 ]]; then
    echo "Refusing to uninstall without confirmation on noninteractive input. Use --force." >&2
    exit 1
  fi
  read -r -p "Remove bgpx installation? Type 'yes' to continue: " answer
  if [[ "${answer}" != "yes" ]]; then
    echo "Uninstall cancelled."
    exit 0
  fi
}

remove_service() {
  if [[ ! -f "${SERVICE_FILE}" ]]; then
    echo "Systemd unit:      not found"
    return
  fi

  if command -v systemctl >/dev/null 2>&1; then
    systemctl stop "${APP_NAME}.service" >/dev/null 2>&1 || true
    systemctl disable "${APP_NAME}.service" >/dev/null 2>&1 || true
  fi

  rm -f "${SERVICE_FILE}"

  if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl reset-failed "${APP_NAME}.service" >/dev/null 2>&1 || true
  fi

  echo "Systemd unit:      removed ${SERVICE_FILE}"
}

remove_bin_link() {
  if [[ ! -L "${BIN_LINK}" ]]; then
    echo "Command link:      not found"
    return
  fi

  local target
  target="$(readlink -f "${BIN_LINK}")"
  case "${target}" in
    "${INSTALL_DIR}"/*)
      rm -f "${BIN_LINK}"
      echo "Command link:      removed ${BIN_LINK}"
      ;;
    *)
      echo "Command link:      kept ${BIN_LINK} -> ${target} (outside ${INSTALL_DIR})"
      ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)
      INSTALL_DIR="${2:?missing value for --install-dir}"
      shift 2
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --keep-data)
      KEEP_DATA=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "${EUID}" -ne 0 && ( -f "${SERVICE_FILE}" || "${INSTALL_DIR}" == /opt* || -e "${BIN_LINK}" ) ]]; then
  echo "Uninstalling system paths requires root. Re-run with sudo or choose --install-dir." >&2
  exit 1
fi

echo "Uninstall plan"
echo "=============="
echo "Application:       ${APP_NAME}"
echo "Install directory: ${INSTALL_DIR}"
echo "Command link:      ${BIN_LINK}"
echo "Systemd unit:      ${SERVICE_FILE}"
if [[ "${KEEP_DATA}" -eq 1 ]]; then
  echo "Data removal:      skipped by --keep-data"
else
  echo "Data removal:      ${INSTALL_DIR}"
fi
echo

confirm

remove_service
remove_bin_link

if [[ "${KEEP_DATA}" -eq 1 ]]; then
  echo "Install directory: kept ${INSTALL_DIR}"
elif [[ -e "${INSTALL_DIR}" ]]; then
  rm -rf "${INSTALL_DIR}"
  echo "Install directory: removed ${INSTALL_DIR}"
else
  echo "Install directory: not found"
fi

# ponytail: clean up Rust build artifacts and local shared library if present
if [[ -f "${SRC_DIR}/bgpx_rust/Cargo.toml" ]] && command -v cargo >/dev/null 2>&1; then
  echo "Cleaning Rust build artifacts..."
  cargo clean --manifest-path "${SRC_DIR}/bgpx_rust/Cargo.toml" >/dev/null 2>&1 || true
fi
rm -f "${SRC_DIR}/bgpx/message/libbgpx_rust.so"

echo
echo "Uninstall complete."
