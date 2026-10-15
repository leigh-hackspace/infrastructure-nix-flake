#!/usr/bin/env bash
# Build the status-page web app (Solid.js + TypeScript, Vite) and regenerate
# the Rust asset module the binary embeds (src/assets.rs). Run this before a
# plain `cargo build`; the Nix build (crates/whisper-ws/package.nix) does the
# same steps in-sandbox.
set -euo pipefail
cd "$(dirname "$0")"

if [ ! -d node_modules ]; then
  npm ci
fi
npm run build
./gen_assets.sh dist ../../src/assets.rs
