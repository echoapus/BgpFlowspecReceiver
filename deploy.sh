#!/usr/bin/env bash
set -Eeuo pipefail

APP_NAME="bgpx"
INSTALL_DIR="${INSTALL_DIR:-/opt/bgpx}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
CREATE_SERVICE=0
SET_BIND_CAP=0

usage() {
  cat <<'EOF'
Usage: ./deploy.sh [options]

Install bgpx under /opt/bgpx by default.

Options:
  --install-dir DIR          Install location (default: /opt/bgpx)
  --python PATH             Python interpreter to use (default: python3)
  --service                 Install and enable a systemd service
  --cap-net-bind-service    Allow the venv Python to bind privileged ports like 179
  -h, --help                Show this help

Environment:
  INSTALL_DIR=/opt/bgpx
  PYTHON_BIN=python3
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)
      INSTALL_DIR="${2:?missing value for --install-dir}"
      shift 2
      ;;
    --python)
      PYTHON_BIN="${2:?missing value for --python}"
      shift 2
      ;;
    --service)
      CREATE_SERVICE=1
      shift
      ;;
    --cap-net-bind-service)
      SET_BIND_CAP=1
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

if [[ "${EUID}" -ne 0 && "${INSTALL_DIR}" == /opt* ]]; then
  echo "Installing to ${INSTALL_DIR} requires root. Re-run with sudo or choose --install-dir." >&2
  exit 1
fi

if ! command -v "${PYTHON_BIN}" >/dev/null 2>&1; then
  echo "Python interpreter not found: ${PYTHON_BIN}" >&2
  exit 1
fi

SRC_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
VENV_DIR="${INSTALL_DIR}/venv"
APP_DIR="${INSTALL_DIR}/app"
BIN_LINK="/usr/local/bin/${APP_NAME}"

echo "Installing ${APP_NAME} from ${SRC_DIR} to ${INSTALL_DIR}"

install -d "${APP_DIR}"

tar \
  --exclude='.git' \
  --exclude='.agents' \
  --exclude='.codex' \
  --exclude='.pytest_cache' \
  --exclude='__pycache__' \
  --exclude='*.pyc' \
  --exclude='*.pyo' \
  --exclude='deploy.sh' \
  -C "${SRC_DIR}" \
  -cf - . | tar -C "${APP_DIR}" -xf -

"${PYTHON_BIN}" -m venv "${VENV_DIR}"
"${VENV_DIR}/bin/python" -m pip install --upgrade pip
"${VENV_DIR}/bin/pip" install "${APP_DIR}"

if [[ "${EUID}" -eq 0 ]]; then
  ln -sfn "${VENV_DIR}/bin/${APP_NAME}" "${BIN_LINK}"
  echo "Linked ${BIN_LINK} -> ${VENV_DIR}/bin/${APP_NAME}"
fi

if [[ "${SET_BIND_CAP}" -eq 1 ]]; then
  if ! command -v setcap >/dev/null 2>&1; then
    echo "setcap not found; install libcap tools or skip --cap-net-bind-service" >&2
    exit 1
  fi
  setcap cap_net_bind_service+ep "$(readlink -f "${VENV_DIR}/bin/python")"
  echo "Granted cap_net_bind_service to ${VENV_DIR}/bin/python"
fi

if [[ "${CREATE_SERVICE}" -eq 1 ]]; then
  if [[ "${EUID}" -ne 0 ]]; then
    echo "--service requires root" >&2
    exit 1
  fi

  cat >/etc/systemd/system/${APP_NAME}.service <<EOF
[Unit]
Description=BGP Flowspec Receiver
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${APP_DIR}
ExecStart=${VENV_DIR}/bin/bgpx --host 0.0.0.0 --port 8080
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

  systemctl daemon-reload
  systemctl enable "${APP_NAME}.service"
  echo "Installed systemd service: ${APP_NAME}.service"
fi

echo
echo "Install complete."
echo "Run: ${VENV_DIR}/bin/bgpx --host 0.0.0.0 --port 8080"
if [[ "${EUID}" -eq 0 ]]; then
  echo "Or:  ${APP_NAME} --host 0.0.0.0 --port 8080"
fi
