#!/usr/bin/env bash
# Build the Dioxus SPA and assemble dist/ (NOT committed; the flake builds it
# — see machines/services1/services/filestore.nix — and embeds it into the
# filestore binary via build.rs).
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release --target wasm32-unknown-unknown

BIN="target/wasm32-unknown-unknown/release/filestore_web.wasm"

if ! wasm-bindgen --version >/dev/null 2>&1; then
    echo "wasm-bindgen-cli is required (devshell installs 0.2.128)" >&2
    exit 1
fi
# The generated glue is schema-versioned: the CLI must match the
# wasm-bindgen crate in Cargo.lock exactly (0.2.128). Warn, don't fail —
# the devshell installs the right one.
WANTED="wasm-bindgen 0.2.128"
HAVE="$(wasm-bindgen --version)"
if [ "$HAVE" != "$WANTED" ]; then
    echo "warning: wasm-bindgen-cli is '$HAVE', expected '$WANTED'" >&2
fi

rm -rf dist
wasm-bindgen --target web --out-dir dist --no-typescript "$BIN"
cp index.html dist/index.html

echo "dist/ ready:"
du -sh dist
