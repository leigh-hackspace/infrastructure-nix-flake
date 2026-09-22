#!/usr/bin/env bash
# Build the Dioxus SPA and assemble dist/ (committed; embedded into the
# frigate-monitor binary by its build.rs).
#
# Prerequisites (see README.md):
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.128
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release --target wasm32-unknown-unknown

BIN="target/wasm32-unknown-unknown/release/frigate_monitor_web.wasm"

if ! wasm-bindgen --version >/dev/null 2>&1; then
    echo "wasm-bindgen-cli is required: cargo install wasm-bindgen-cli --version 0.2.128" >&2
    exit 1
fi
WANTED="wasm-bindgen 0.2.128"
HAVE="$(wasm-bindgen --version)"
if [ "$HAVE" != "$WANTED" ]; then
    echo "wasm-bindgen-cli version mismatch: want '$WANTED', have '$HAVE'" >&2
    echo "install the matching one with: cargo install wasm-bindgen-cli --version 0.2.128" >&2
    exit 1
fi

rm -rf dist
wasm-bindgen --target web --out-dir dist --no-typescript "$BIN"
cp index.html dist/index.html

echo "dist/ ready:"
du -sh dist
