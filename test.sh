#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUST_SO="${ROOT}/bgpx/message/libbgpx_rust.so"
TMP="$(mktemp -d)"
export PYTHONDONTWRITEBYTECODE=1

cleanup() {
  rm -f "${RUST_SO}"
  if [[ -f "${TMP}/libbgpx_rust.so.original" ]]; then
    cp "${TMP}/libbgpx_rust.so.original" "${RUST_SO}"
  fi
  rm -rf "${TMP}"
}
trap cleanup EXIT

if [[ -f "${RUST_SO}" ]]; then
  cp "${RUST_SO}" "${TMP}/libbgpx_rust.so.original"
fi
rm -f "${RUST_SO}"

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

echo "== Rust release parser through Python FFI =="
cargo build --offline --release --manifest-path "${MANIFEST}"
cp "${CARGO_TARGET_DIR}/release/libbgpx_rust.so" "${RUST_SO}"
python3 -c 'from bgpx.message import parser; assert parser._lib is not None'
python3 -m pytest -q

echo "All tests passed."
