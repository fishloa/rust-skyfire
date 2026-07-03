#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLCHAIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
export PATH="$TOOLCHAIN:$PATH"
# Bundler target for npm consumers (webpack/vite/rollup resolve the wasm).
wasm-pack build "$ROOT/crates/skyfire-wasm" --target bundler --release \
  --out-dir "$ROOT/packages/core/pkg"
# wasm-pack writes pkg/.gitignore = "*", which makes `npm pack`/`npm publish`
# EXCLUDE the whole pkg dir (the wasm!) from the tarball. Remove it so the
# published package actually ships the wasm. (The root .gitignore still keeps
# pkg/ out of git.)
rm -f "$ROOT/packages/core/pkg/.gitignore"
echo "built packages/core/pkg (pkg/.gitignore removed for publish)"
