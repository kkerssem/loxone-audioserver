#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "cargo-llvm-cov is required. Install: cargo install cargo-llvm-cov" >&2
  exit 1
fi

IGNORE='(tests/|main\.rs|remote/ws\.rs|cast/discovery\.rs|cast/session\.rs|tls\.rs|coordinator/transport\.rs|coordinator/cast\.rs|coordinator/browse\.rs|coordinator/config\.rs|coordinator/status\.rs|cast/manager\.rs|remote/mod\.rs|remote/handlers/|miniserver/mod\.rs|miniserver/ws\.rs|mdns_browse\.rs|discovery/mdns\.rs|radio/tunein_|radio/audiopath\.rs|radio/custom_streams\.rs)'

LCOV="${ROOT}/target/llvm-cov/lcov.info"
COBERTURA="${ROOT}/target/llvm-cov/cobertura.xml"
mkdir -p target/llvm-cov

cargo llvm-cov --features miniserver --lcov \
  --ignore-filename-regex "$IGNORE" \
  --output-path "$LCOV"

cargo llvm-cov report --cobertura \
  --ignore-filename-regex "$IGNORE" \
  --output-path "$COBERTURA"

"$ROOT/scripts/coverage-check-files.sh" "$LCOV"

echo "Coverage report: $LCOV"
echo "Cobertura report: $COBERTURA"
