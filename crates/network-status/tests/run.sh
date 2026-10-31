#!/usr/bin/env bash
# Headless-browser test runner for the network-status web UI (the Dioxus SPA behind
# network-info.int.leighhack.org).
#
# Builds the SPA and the binary, starts the binary on loopback with `--local` (so the probe
# measures *this* box rather than dialling the router), points the lease table at a fixture
# file instead of the router's, and drives the page with headless Chromium.  Run via
# `just network-status-test`, which puts cargo/rustc/node on PATH through the devshell.
#
# Why a fixture lease file rather than the router's, and why the connection table is left on:
# `/proc/net/nf_conntrack` and dnsmasq's lease file are group-read files on the router
# (`machines/gw/conntrack-reader.nix`, `machines/gw/lease-reader.nix`) and belong to a service
# account, so a suite that read them would report a result that depends on which box it ran on.
# The fixture is the same format, read through the same parser and the same endpoint, so the
# render path is the real one — and one of its rows carries a hostname that is markup, which
# is only worth checking through a path that parses it.
#
# The panels' error path is driven from the suite itself (one endpoint answered with a 503),
# because that is the thing the rule is about: a failed fetch keeps the last data and shows
# its reason in the panel that failed, while the other panels carry on.  The connection table
# stays on because the panel renders its controls only when it has data; here the read fails
# and the table comes back empty, which is the honest shape to drive a control against.
#
# Environment:
#   NS_TEST_PORT          web port            (default 18091)
#   NS_TEST_BROWSER       chromium executable (default: nixpkgs' wrapped one)
#   NS_TEST_KEEP=1        leave the scratch directory behind
#   NS_TEST_SKIP_BUILD=1  reuse the existing bundle and binary (iterate fast)
#   NS_TEST_RELEASE=1     build the binary in release mode

set -euo pipefail

cd "$(dirname "$0")"          # network-status/tests
cd ..                          # network-status

WORKDIR="${NS_TEST_TMPDIR:-$(mktemp -d "${TMPDIR:-/tmp}/network-status-test.XXXXXX")}"
mkdir -p "$WORKDIR"
LOG_FRONTEND="$WORKDIR/frontend.log"
LOG_SERVER="$WORKDIR/server.log"

PORT="${NS_TEST_PORT:-18091}"

if ss -ltn 2>/dev/null | grep -q ":${PORT}[[:space:]]"; then
  echo "port ${PORT} is already in use — another network-status is running" >&2
  exit 1
fi

cleanup() {
  [ -n "${NS_TEST_KEEP:-}" ] && { echo "kept: $WORKDIR"; return; }
  [ -n "${APP:-}" ] && kill "$APP" 2>/dev/null || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

# 1. the SPA bundle and the binary.  `dist/` is git-ignored and built by the flake, so a
#    stale bundle here means the suite tested a page nobody can deploy.  SKIP_BUILD is the
#    opt-out and it says so in the log line.
if [ -z "${NS_TEST_SKIP_BUILD:-}" ]; then
  (cd frontend && ./build.sh) > "$LOG_FRONTEND" 2>&1 || {
    echo "frontend build failed:" >&2
    cat "$LOG_FRONTEND" >&2
    exit 1
  }
  # cargo writes to the workspace root's target/ (crates/Cargo.toml), so the binary lands
  # one directory above this crate — and build.rs embeds the bundle that was just built, so
  # the binary this suite drives is the one the flake would build.
  (cd .. && cargo build --offline -p network-status) > "$WORKDIR/cargo.log" 2>&1 || {
    echo "binary build failed:" >&2
    cat "$WORKDIR/cargo.log" >&2
    exit 1
  }
else
  echo "NS_TEST_SKIP_BUILD=1: reusing $(ls -l --time-style=+%F_%H:%M frontend/dist | tail -1 | awk '{print $6, $7}')"
fi

BIN="../target/${NS_TEST_RELEASE:+release}${NS_TEST_RELEASE:-debug}/network-status"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

# 2. a headless browser.  Playwright's own download is unusable on NixOS (the unwrapped
#    binary cannot find libglib), so prefer nixpkgs' wrapped browsers.
if [ -z "${NS_TEST_BROWSER:-}" ] && command -v nix > /dev/null; then
  B="$(nix build --no-link --print-out-paths 'nixpkgs#playwright.browsers-chromium' 2>/dev/null || true)"
  if [ -n "$B" ]; then
    NS_TEST_BROWSER="$(echo "$B"/chromium-*/chrome-linux64/chrome)"
  fi
fi

# 3. the app.  `--local` keeps the probe on this box, `--leases-file` points it at the fixture
#    (a lease line whose hostname is markup, which the parser has to treat as a name), and
#    `--interval 1` means the page's poll is one second rather than five, so the suite waits
#    for a poll rather than for a minute.  There is no `--static-dir` any more: the bundle is
#    embedded in the binary, which is what the deployed unit runs, so the suite drives the same
#    page a browser gets on gw.
LEASES="$WORKDIR/dnsmasq.leases"
# One real-shaped row and one row whose hostname is markup. dnsmasq's own `duid` header line is
# included because the parser has to skip it rather than turn it into a row. The hostile name is
# a single token because dnsmasq's file is whitespace-separated: a name with a space in it is two
# fields to the parser, and the suite would then be testing the parser's field split rather than
# the page's escaping.
printf '%s\n' \
  'duid 00:01:00:01:32:6b:9c:78:00:25:90:d3:f0:ea' \
  '1900000000 aa:bb:cc:dd:ee:ff 10.3.2.99 <img/onerror=x> *' > "$LEASES"
"$BIN" --local --bind 127.0.0.1 --port "$PORT" --interval 1 \
  --leases-file "$LEASES" > "$LOG_SERVER" 2>&1 &
APP=$!

for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:${PORT}/api/config" > /dev/null && break
  sleep 0.25
done
if ! curl -sf "http://127.0.0.1:${PORT}/api/config" > /dev/null; then
  echo "network-status did not come up on ${PORT}:" >&2
  cat "$LOG_SERVER" >&2
  exit 1
fi

# The first probe has to have landed before the suite starts: the charts draw from the
# rolling window, and a page that has not had one sample yet is an empty canvas that the
# suite would read as a broken chart rather than as a page that is one poll old.
for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:${PORT}/api/snapshot" | grep -q '"ts":[1-9]' && break
  sleep 0.25
done
if ! curl -sf "http://127.0.0.1:${PORT}/api/snapshot" | grep -q '"ts":[1-9]'; then
  echo "the probe never produced a sample on this box:" >&2
  curl -s "http://127.0.0.1:${PORT}/api/snapshot" >&2 || true
  echo >&2
  cat "$LOG_SERVER" >&2
  exit 1
fi

# 4. the suite
cd tests
export NS_TEST_PORT="$PORT" NS_TEST_BROWSER="${NS_TEST_BROWSER:-}"
[ -d node_modules ] || npm install --no-audit --no-fund
set +e
node suite.mjs
status=$?
set -e
if [ "$status" != 0 ]; then
  echo "--- network-status log:" >&2
  tail -25 "$LOG_SERVER" >&2
fi
exit "$status"
