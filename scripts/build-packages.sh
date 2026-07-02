#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLCHAIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
export PATH="$TOOLCHAIN:$PATH"
# Bundler target for npm consumers (webpack/vite/rollup resolve the wasm).
wasm-pack build "$ROOT/crates/skyfire-wasm" --target bundler --release \
  --out-dir "$ROOT/packages/core/pkg"
echo "built packages/core/pkg"
