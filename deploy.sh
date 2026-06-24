#!/usr/bin/env bash
set -Eeuo pipefail

APP_NAME="bgpx"
INSTALL_DIR="${INSTALL_DIR:-/opt/bgpx}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
WEB_PORT="${WEB_PORT:-}"
CREATE_SERVICE=""
SET_BIND_CAP=0
SERVICE_FILE="/etc/systemd/system/${APP_NAME}.service"
USE_RUST=""

usage() {
  cat <<'EOF'
Usage: ./deploy.sh [options]

Install bgpx under /opt/bgpx by default.

Options:
  --install-dir DIR          Install location (default: /opt/bgpx)
  --python PATH             Python interpreter to use (default: python3)
  --web-port PORT           Web UI bind port (default prompt: 8080)
  --service                 Install and enable a systemd service
  --cap-net-bind-service    Allow the venv Python to bind privileged ports like 179
  --rust                    Compile and use the Rust-optimized parser
  -h, --help                Show this help

Environment:
  INSTALL_DIR=/opt/bgpx
  PYTHON_BIN=python3
  WEB_PORT=8080
EOF
}

is_valid_port() {
  [[ "$1" =~ ^[0-9]+$ ]] && [[ "$1" -ge 1 ]] && [[ "$1" -le 65535 ]]
}

prompt_web_port() {
  local value

  if [[ -n "${WEB_PORT}" ]]; then
    if ! is_valid_port "${WEB_PORT}"; then
      echo "Invalid WEB_PORT: ${WEB_PORT}. Use a port from 1 to 65535." >&2
      exit 1
    fi
    return
  fi

  if [[ -t 0 ]]; then
    while true; do
      read -r -p "Web UI bind port [8080]: " value
      value="${value:-8080}"
      if is_valid_port "${value}"; then
        WEB_PORT="${value}"
        return
      fi
      echo "Invalid port. Use a port from 1 to 65535." >&2
    done
  fi

  WEB_PORT=8080
  echo "No interactive input detected; using default Web UI port ${WEB_PORT}."
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
    --web-port)
      WEB_PORT="${2:?missing value for --web-port}"
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
    --rust)
      USE_RUST=1
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

prompt_service() {
  # ponytail: prompt for service installation if running interactively as root and not explicitly set.
  [[ -n "${CREATE_SERVICE}" ]] && return
  if [[ "${EUID}" -eq 0 && -t 0 ]]; then
    read -r -p "Install and enable systemd service? [y/N]: " ans
    [[ "${ans}" =~ ^[Yy] ]] && CREATE_SERVICE=1 || CREATE_SERVICE=0
  else
    CREATE_SERVICE=0
  fi
}

prompt_web_port
prompt_service

prompt_rust() {
  [[ -n "${USE_RUST}" ]] && return
  if [[ -t 0 ]]; then
    read -r -p "Use Rust-optimized parser? [y/N]: " ans
    [[ "${ans}" =~ ^[Yy] ]] && USE_RUST=1 || USE_RUST=0
  else
    USE_RUST=0
  fi
}
prompt_rust

SRC_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${USE_RUST}" -eq 1 ]]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust (cargo) not found."
    if [[ -t 0 ]]; then
      read -r -p "Install Rust automatically via rustup? [Y/n]: " ans
      if [[ ! "${ans}" =~ ^[Nn] ]]; then
        echo "Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        if [[ -f "$HOME/.cargo/env" ]]; then
          source "$HOME/.cargo/env"
        elif [[ -f "/root/.cargo/env" ]]; then
          source "/root/.cargo/env"
        fi
      else
        echo "Rust is required to compile. Exiting." >&2
        exit 1
      fi
    else
      echo "Non-interactive shell and Rust not found. Exiting." >&2
      exit 1
    fi
  fi

  echo "Compiling Rust BGP parser extension..."
  cargo build --release --manifest-path "${SRC_DIR}/bgpx_rust/Cargo.toml"
fi

VENV_DIR="${INSTALL_DIR}/venv"
APP_DIR="${INSTALL_DIR}/app"
BIN_LINK="/usr/local/bin/${APP_NAME}"

echo "Installing ${APP_NAME} from ${SRC_DIR} to ${INSTALL_DIR}"
echo "Web UI will bind to 0.0.0.0:${WEB_PORT}"

install -d "${APP_DIR}"

tar \
  --exclude='.git' \
  --exclude='.agents' \
  --exclude='.codex' \
  --exclude='.vscode' \
  --exclude='.idea' \
  --exclude='.pytest_cache' \
  --exclude='__pycache__' \
  --exclude='*.pyc' \
  --exclude='*.pyo' \
  --exclude='deploy.sh' \
  --exclude='uninstall.sh' \
  -C "${SRC_DIR}" \
  -cf - . | tar -C "${APP_DIR}" -xf -

"${PYTHON_BIN}" -m venv "${VENV_DIR}"
"${VENV_DIR}/bin/python" -m pip install --upgrade pip
"${VENV_DIR}/bin/pip" install "${APP_DIR}"

if [[ "${USE_RUST}" -eq 1 ]]; then
  SITEPACKAGES=$("${VENV_DIR}/bin/python" -c "import sysconfig; print(sysconfig.get_path('purelib'))")
  mkdir -p "${SITEPACKAGES}/bgpx/message"
  cp "${SRC_DIR}/bgpx_rust/target/release/libbgpx_rust.so" "${SITEPACKAGES}/bgpx/message/libbgpx_rust.so"
  echo "Installed Rust parser extension to ${SITEPACKAGES}/bgpx/message/libbgpx_rust.so"
fi

"${VENV_DIR}/bin/python" - <<'PY'
from importlib import resources

ui = resources.files("bgpx").joinpath("web", "ui.html")
if not ui.is_file():
    raise SystemExit("Installed package is missing bgpx/web/ui.html")
text = ui.read_text(encoding="utf-8")
if "<!DOCTYPE html>" not in text:
    raise SystemExit("Installed bgpx/web/ui.html does not look like the Web UI")
print("Verified installed package data: bgpx/web/ui.html")
PY

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

  cat >"${SERVICE_FILE}" <<EOF
[Unit]
Description=BGP Flowspec Receiver
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${APP_DIR}
ExecStart=${VENV_DIR}/bin/bgpx --host 0.0.0.0 --port ${WEB_PORT}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

  systemctl daemon-reload
  systemctl enable --now "${APP_NAME}.service"
  echo "Installed and started systemd service: ${SERVICE_FILE}"
fi

echo
echo "Install complete"
echo "=============="
echo "Application:       ${APP_NAME}"
echo "Install directory: ${INSTALL_DIR}"
echo "Source copy:       ${APP_DIR}"
echo "Virtualenv:        ${VENV_DIR}"
echo "Executable:        ${VENV_DIR}/bin/bgpx"
echo "Web UI bind:       0.0.0.0:${WEB_PORT}"
echo "Web UI URL:        http://localhost:${WEB_PORT}"
if [[ "${USE_RUST}" -eq 1 ]]; then
  echo "Parser Engine:     Rust-optimized (libbgpx_rust.so)"
else
  echo "Parser Engine:     Pure Python"
fi
if [[ "${EUID}" -eq 0 ]]; then
  echo "Command link:      ${BIN_LINK}"
else
  echo "Command link:      not created (run as root to link ${BIN_LINK})"
fi
if [[ "${SET_BIND_CAP}" -eq 1 ]]; then
  echo "BGP port 179:      cap_net_bind_service granted to $(readlink -f "${VENV_DIR}/bin/python")"
else
  echo "BGP port 179:      run as root or redeploy with --cap-net-bind-service"
fi
if [[ "${CREATE_SERVICE}" -eq 1 ]]; then
  echo "Systemd unit:      ${SERVICE_FILE}"
  echo
  echo "Next commands:"
  echo "  sudo systemctl status ${APP_NAME}"
  echo "  sudo journalctl -u ${APP_NAME} -f"
else
  echo "Systemd unit:      not installed"
  echo
  echo "Start manually:"
  echo "  ${VENV_DIR}/bin/bgpx --host 0.0.0.0 --port ${WEB_PORT}"
fi
echo
echo "Uninstall:"
if [[ -x "${SRC_DIR}/uninstall.sh" ]]; then
  echo "  sudo ${SRC_DIR}/uninstall.sh"
else
  echo "  sudo rm -rf ${INSTALL_DIR} ${BIN_LINK} ${SERVICE_FILE}"
fi
