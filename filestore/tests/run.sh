#!/usr/bin/env bash
# Headless-browser test runner for the filestore web UI.
#
# Builds the binary, starts it with --no-auth (loopback only) against a
# scratch fixture, and drives it with headless Chromium.  Run via
# `just filestore-test`, which puts cargo/rustc on PATH through the devshell.

set -euo pipefail

cd "$(dirname "$0")"          # filestore/tests
cd ..                          # filestore

# Everything this script writes lives under one scratch directory, so two runs
# (or a leftover from a crashed one) cannot clobber each other's logs, fixture
# or browser choice.  Override the base with FS_TEST_TMPDIR.
WORKDIR="${FS_TEST_TMPDIR:-$(mktemp -d "${TMPDIR:-/tmp}/filestore-test.XXXXXX")}"
mkdir -p "$WORKDIR"
LOG_FRONTEND="$WORKDIR/frontend.log"
LOG_SERVER="$WORKDIR/server.log"

cleanup() {
  [ -n "${FS_TEST_KEEP:-}" ] && return
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

PORT="${FS_TEST_PORT:-18097}"
# The fixture is inside WORKDIR by default; an explicit FS_TEST_ROOT (e.g. on a
# filesystem with room for the big fixture) still wins.
ROOT="${FS_TEST_ROOT:-$WORKDIR/root}"
BIN="target/release/filestore"

# 1. build the SPA bundle and the binary (offline; build.rs embeds the bundle)
(cd frontend && ./build.sh) > "$LOG_FRONTEND" 2>&1 || {
  echo "frontend build failed:" >&2
  cat "$LOG_FRONTEND" >&2
  exit 1
}
cargo build --release --offline
FS_TEST_BIN="$(readlink -f "$BIN")"

# 2. resolve a headless browser.  Playwright's own download is unusable on
#    NixOS (the unwrapped binary cannot find libglib), so prefer nixpkgs'
#    wrapped browsers; override with FS_TEST_BROWSER.
if [ -z "${FS_TEST_BROWSER:-}" ] && command -v nix > /dev/null; then
  B="$(nix build --no-link --print-out-paths 'nixpkgs#playwright.browsers-chromium' 2>/dev/null || true)"
  if [ -n "$B" ]; then
    FS_TEST_BROWSER="$(echo "$B"/chromium-*/chrome-linux64/chrome)"
  fi
fi

# 3. start the server (the suite rebuilds this fixture before every test)
mkdir -p "$ROOT"
# The thumbnail cache is pointed at the scratch directory so the suite can look
# inside it; on the real box it is /run/filestore-thumbs (tmpfs).
THUMB_DIR="$WORKDIR/thumbs"
mkdir -p "$THUMB_DIR"
if ss -ltn 2>/dev/null | grep -q ":${PORT}[[:space:]]"; then
  echo "port ${PORT} is already in use — another filestore is running (kill it first)" >&2
  exit 1
fi
"$BIN" --root "$ROOT" --no-auth --port "$PORT" --thumb-cache "$THUMB_DIR" > "$LOG_SERVER" 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null || true; cleanup' EXIT

for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:${PORT}/api/whoami" > /dev/null && break
  sleep 0.25
done
if ! curl -sf "http://127.0.0.1:${PORT}/api/whoami" > /dev/null; then
  echo "server did not come up on ${PORT}:" >&2
  cat "$LOG_SERVER" >&2
  exit 1
fi

# 4. run the suite
cd tests
export FS_TEST_BIN FS_TEST_ROOT="$ROOT" FS_TEST_PORT="$PORT" FS_TEST_WORKDIR="$WORKDIR" FS_TEST_THUMB_CACHE="$THUMB_DIR"
[ -d node_modules ] || npm install --no-audit --no-fund
FS_TEST_BROWSER="${FS_TEST_BROWSER:-}" node suite.mjs
