#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "${ROOT}"
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bash -n deploy.sh uninstall.sh
if command -v node >/dev/null 2>&1; then
  sed -n '/<script>/,/<\/script>/p' web/ui.html | sed '1d;$d' | node --check
fi
if command -v python3 >/dev/null 2>&1; then
  cargo build --locked --bin bgpx
  PYTHONDONTWRITEBYTECODE=1 python3 tests/install_smoke.py "${CARGO_TARGET_DIR:-target}/debug/bgpx"
  PYTHONDONTWRITEBYTECODE=1 python3 tests/http_smoke.py "${CARGO_TARGET_DIR:-target}/debug/bgpx"
fi
echo "All tests passed."
