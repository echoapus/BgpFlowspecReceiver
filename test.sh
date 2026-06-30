#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
export PYTHONDONTWRITEBYTECODE=1

cleanup() {
  rm -rf "${TMP}"
}
trap cleanup EXIT

echo "== Python fallback =="
python3 -m pytest -q

echo "== Python syntax =="
PYTHONPYCACHEPREFIX="${TMP}/pycache" \
  python3 -m py_compile "${ROOT}"/bgpx/*.py "${ROOT}"/bgpx/message/*.py

echo "== Deploy/uninstall shell syntax =="
bash -n "${ROOT}/deploy.sh" "${ROOT}/uninstall.sh"

if command -v node >/dev/null 2>&1; then
  echo "== Web UI JavaScript syntax =="
  sed -n '/<script>/,/<\/script>/p' "${ROOT}/bgpx/web/ui.html" |
    sed '1d;$d' |
    node --check
fi

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required for Rust parser tests" >&2
  exit 1
}

cp -a "${ROOT}/bgpx_rust" "${TMP}/bgpx_rust"
MANIFEST="${TMP}/bgpx_rust/Cargo.toml"
export CARGO_TARGET_DIR="${TMP}/target"

echo "== Rust format, lint, and unit tests =="
cargo fmt --manifest-path "${ROOT}/bgpx_rust/Cargo.toml" -- --check
cargo clippy --offline --manifest-path "${MANIFEST}" -- -D warnings
cargo test --offline --manifest-path "${MANIFEST}"

echo "== Rust release parser through Python (PyO3) =="
cargo build --offline --release --manifest-path "${MANIFEST}"
command -v maturin >/dev/null 2>&1 || {
  echo "maturin is required for PyO3 parser tests" >&2
  exit 1
}
maturin build --release \
  --manifest-path "${TMP}/bgpx_rust/Cargo.toml" \
  --interpreter python3 \
  --out "${TMP}/wheels"
python3 -m pip install --quiet --root-user-action ignore --no-index --find-links "${TMP}/wheels" --target "${TMP}/site" bgpx_rust
PYTHONPATH="${TMP}/site:${ROOT}${PYTHONPATH:+:${PYTHONPATH}}" \
  python3 -c 'from bgpx.message import parser; assert parser._rust is not None'
PYTHONPATH="${TMP}/site:${ROOT}${PYTHONPATH:+:${PYTHONPATH}}" \
  python3 -m pytest -q

echo "All tests passed."
