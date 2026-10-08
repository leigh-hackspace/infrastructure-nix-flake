#!/usr/bin/env bash
# Shared build recipe for the Dioxus SPAs in this repo (filestore,
# gocardless-dashboard, frigate-monitor).  Each frontend keeps a three-line
# build.sh that calls this with its cargo package name, so there is one recipe
# to keep honest.
#
# The flake builds the bundle itself (CRANE.wasmSpa in common/crane.nix) and
# embeds it via build.rs; this script is the local-development equivalent and
# must produce exactly the same dist/.  Nothing here is committed.
#
# Usage (run from inside a frontend/ directory):
#   ../../common/frontend-build-spa.sh <wasm-package-name>
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: $0 <wasm-package-name>" >&2
    exit 2
fi
WASM_NAME="$1"

if [ ! -f Cargo.toml ] || [ ! -f Cargo.lock ]; then
    echo "not a cargo frontend (run me from inside a frontend/ directory)" >&2
    exit 2
fi

cargo build --release --target wasm32-unknown-unknown

BIN="target/wasm32-unknown-unknown/release/${WASM_NAME}.wasm"
if [ ! -f "$BIN" ]; then
    echo "cargo produced no $BIN (is the package name '$WASM_NAME' right?)" >&2
    exit 1
fi

if ! command -v wasm-bindgen >/dev/null 2>&1; then
    echo "wasm-bindgen-cli is required; 'nix develop' installs the pinned one" >&2
    exit 1
fi

# The generated JS glue is schema-versioned: the CLI must match the
# wasm-bindgen *crate* this SPA compiles against, which is the version in this
# directory's Cargo.lock.  wasm-bindgen itself dies on a mismatch ("schema
# version"), so failing here only turns that into a readable error.  Read the
# wanted version out of the lock file rather than hardcoding it: the lock is
# what the flake pins too.
WANTED="$(awk '/^name = "wasm-bindgen"$/{getline; sub(/^version = /, ""); gsub(/"/, ""); print; exit}' Cargo.lock)"
HAVE="$(wasm-bindgen --version | awk '{print $NF}')"
if [ -z "$WANTED" ]; then
    echo "could not find a wasm-bindgen version in Cargo.lock" >&2
    exit 1
fi
if [ "$HAVE" != "$WANTED" ]; then
    echo "wasm-bindgen-cli is '$HAVE' but this SPA compiles against '$WANTED'." >&2
    echo "The glue is schema-versioned, so the build would fail anyway." >&2
    echo "Use the pinned CLI: 'nix develop' installs $WANTED (see flake.nix)." >&2
    exit 1
fi

rm -rf dist
wasm-bindgen --target web --out-dir dist --no-typescript "$BIN"
cp index.html dist/index.html

echo "dist/ ready (wasm-bindgen $HAVE):"
du -sh dist
