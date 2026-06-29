#!/usr/bin/env bash
set -Eeuo pipefail

APP_NAME="bgpx"
INSTALL_DIR="${INSTALL_DIR:-/opt/bgpx}"
BIN_LINK="/usr/local/bin/${APP_NAME}"
SERVICE_NAME="${APP_NAME}.service"
SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}"
FORCE=0
KEEP_DATA=0

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

require_safe_install_dir() {
  case "${INSTALL_DIR}" in
    ""|"/"|"/opt"|"/usr"|"/var"|"/home")
      echo "Refusing unsafe install dir: ${INSTALL_DIR:-<empty>}" >&2
      exit 1
      ;;
  esac
}

remove_service() {
  if [[ ! -f "${SERVICE_FILE}" ]]; then
    echo "Systemd unit:      not found"
    return
  fi

  if command -v systemctl >/dev/null 2>&1; then
    systemctl stop "${SERVICE_NAME}" >/dev/null 2>&1 || true
    systemctl disable "${SERVICE_NAME}" >/dev/null 2>&1 || true
  fi

  rm -f "${SERVICE_FILE}"

  if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl reset-failed "${SERVICE_NAME}" >/dev/null 2>&1 || true
  fi

  echo "Systemd unit:      removed ${SERVICE_FILE}"
}

remove_bin_link() {
  if [[ ! -e "${BIN_LINK}" ]]; then
    echo "Command link:      not found"
    return
  fi
  if [[ ! -L "${BIN_LINK}" ]]; then
    echo "Command link:      kept ${BIN_LINK} (not a symlink)"
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

require_safe_install_dir

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

echo
echo "Uninstall complete."
