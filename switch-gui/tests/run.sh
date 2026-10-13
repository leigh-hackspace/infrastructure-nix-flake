#!/usr/bin/env bash
# Headless-browser test runner for the switch-gui web UI.
#
# Builds the SPA and the binary, starts a fake ProCurve console (a real Telnet
# server with an in-memory model), points the service at it with --no-auth on a
# loopback bind and a scratch Postgres schema, and drives the page with headless
# Chromium.  Run via `just switch-gui-test`, which puts cargo/rustc/node on PATH
# through the devshell.
#
# Why a fake device rather than the real switches: the suite writes.  Even to a
# shut port, a test that configured a real switch would be a test that can only
# run once and leaves something to undo.  And the real switches cannot be
# restarted into a known state, which a test suite has to be able to do.
#
# Environment:
#   SG_TEST_DB_URL   Postgres DSN of a database the test role may create
#                    schemas in (default: the service's own, over the socket)
#   SG_TEST_PORT     web port            (default 18097)
#   SG_TEST_SWITCH_PORT / SG_TEST_STATE_PORT   the fake device's ports
#   SG_TEST_BROWSER  chromium executable (default: nixpkgs' wrapped one)
#   SG_TEST_KEEP=1   leave the scratch directory and the schema behind
#   SG_TEST_SKIP_BUILD=1  reuse the existing bundle and binary (iterate fast)
#   SG_TEST_RELEASE=1     build the binary in release mode

set -euo pipefail

cd "$(dirname "$0")"          # switch-gui/tests
cd ..                          # switch-gui

WORKDIR="${SG_TEST_TMPDIR:-$(mktemp -d "${TMPDIR:-/tmp}/switch-gui-test.XXXXXX")}"
# An SG_TEST_TMPDIR is used exactly as given, so it has to exist before the first
# log line is redirected into it — otherwise the fake switch dies on a bad
# redirect and the failure surfaces as "the fake switch is not answering".
mkdir -p "$WORKDIR"
LOG_FRONTEND="$WORKDIR/frontend.log"
LOG_SWITCH="$WORKDIR/fake-switch.log"
LOG_SERVER="$WORKDIR/server.log"

PORT="${SG_TEST_PORT:-18097}"
SWITCH_PORT="${SG_TEST_SWITCH_PORT:-22323}"
STATE_PORT="${SG_TEST_STATE_PORT:-22324}"
# One schema per run, so two runs (or a crashed one) cannot see each other's
# shadow copy.  `$$` is enough here: it is unique per invocation and the name
# outlives this script only with SG_TEST_KEEP.
SCHEMA="${SG_TEST_SCHEMA:-sgtest_$$}"
# The service's own DSN: the unix socket, as the service's own role, which is
# what the unit uses too (see machines/services1/services/switch-gui.nix) — no
# password anywhere, and the schema is the only thing this needs to be able to
# create.
DB_URL="${SG_TEST_DB_URL:-postgresql://switch_gui@%2Frun%2Fpostgresql/switch_gui}"
BIN="target/${SG_TEST_RELEASE:+release}${SG_TEST_RELEASE:-debug}/switch-gui"

for p in "$PORT" "$SWITCH_PORT" "$STATE_PORT"; do
  if ss -ltn 2>/dev/null | grep -q ":${p}[[:space:]]"; then
   echo "port ${p} is already in use — another switch-gui or fake switch is running" >&2
    exit 1
 fi
done

cleanup() {
  [ -n "${SG_TEST_KEEP:-}" ] && { echo "kept: $WORKDIR (schema $SCHEMA)"; return; }
  [ -n "${APP:-}" ] && kill "$APP" 2>/dev/null || true
  [ -n "${FAKE:-}" ] && kill "$FAKE" 2>/dev/null || true
  psql "$DB_URL" -q -c "drop schema if exists \"${SCHEMA}\" cascade" >/dev/null 2>&1 || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

# 1. the SPA bundle and the binary.  build.rs embeds the bundle, so a stale
#    frontend/dist means the suite tested a page nobody can deploy; SKIP_BUILD is
#    the opt-out, and it says so in the log line below.
if [ -z "${SG_TEST_SKIP_BUILD:-}" ]; then
  (cd frontend && ./build.sh) > "$LOG_FRONTEND" 2>&1 || {
   echo "frontend build failed:" >&2
    cat "$LOG_FRONTEND" >&2
    exit 1
  }
  cargo build ${SG_TEST_RELEASE:+--release} --offline
else
  echo "SG_TEST_SKIP_BUILD=1: reusing $(ls -l --time-style=+%F_%H:%M frontend/dist | tail -1 | awk '{print $6, $7}')"
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

# 2. a headless browser.  Playwright's own download is unusable on NixOS (the
#    unwrapped binary cannot find libglib), so prefer nixpkgs' wrapped browsers.
if [ -z "${SG_TEST_BROWSER:-}" ] && command -v nix > /dev/null; then
  B="$(nix build --no-link --print-out-paths 'nixpkgs#playwright.browsers-chromium' 2>/dev/null || true)"
  if [ -n "$B" ]; then
    SG_TEST_BROWSER="$(echo "$B"/chromium-*/chrome-linux64/chrome)"
 fi
fi

# 3. a scratch schema.  The app migrates itself on startup, so this is the only
#    DDL the suite does; everything else is the app's own `SCHEMA`.
psql "$DB_URL" -q -c "create schema \"${SCHEMA}\"" >/dev/null || {
  echo "could not create schema ${SCHEMA} in the test database." >&2
  echo "Point SG_TEST_DB_URL at a database whose role may create schemas." >&2
 exit 1
}

# 4. the fake device, then the service pointed at it.
node tests/fake-switch.mjs --telnet "$SWITCH_PORT" --state "$STATE_PORT" > "$LOG_SWITCH" 2>&1 &
FAKE=$!
# Wait for it rather than sleeping: a fake that cannot bind its port dies in the
# first second, and the failure is then a service that cannot read a switch and a
# page with no matrix — three layers away from the real reason in the log.
for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:${STATE_PORT}/meta" > /dev/null && break
  sleep 0.25
done
if ! curl -sf "http://127.0.0.1:${STATE_PORT}/meta" > /dev/null; then
  echo "the fake switch is not answering on ${STATE_PORT}:" >&2
  cat "$LOG_SWITCH" >&2
 exit 1
fi

# The credentials are the fake's: it accepts any login, and `enable` is the
# second login this firmware does. Nothing here can reach a real switch — the
# only address the service is given is 127.0.0.1.
SWITCH_GUI_SWITCHES="switch1=127.0.0.1:${SWITCH_PORT}" \
SWITCH_GUI_LOGIN=agent SWITCH_GUI_PASSWORD=test-fixture-not-a-secret \
"$BIN" --no-auth --bind "127.0.0.1:${PORT}" \
  --db-url "${DB_URL}?options=-csearch_path%3D${SCHEMA}" \
  --interval 2 > "$LOG_SERVER" 2>&1 &
APP=$!

for _ in $(seq 1 60); do
  curl -sf "http://127.0.0.1:${PORT}/healthz" > /dev/null && break
  sleep 0.25
done
if ! curl -sf "http://127.0.0.1:${PORT}/healthz" > /dev/null; then
  echo "switch-gui did not come up on ${PORT}:" >&2
  cat "$LOG_SERVER" >&2
 exit 1
fi
# The first read has to have happened before the suite starts, or the first test
# spends its timeout waiting for a matrix that the page cannot draw until the
# shadow copy has rows.  It is also the suite's only chance to see a console
# session fail for a reason the page would only show as a warning.
for _ in $(seq 1 80); do
  curl -sf "http://127.0.0.1:${PORT}/api/state" | grep -q '"ports":[1-9]' && break
  sleep 0.25
done
if ! curl -sf "http://127.0.0.1:${PORT}/api/state" | grep -q '"ports":[1-9]'; then
  echo "the sync never read the fake switch:" >&2
  curl -s "http://127.0.0.1:${PORT}/api/state" >&2 || true
  echo >&2
  cat "$LOG_SERVER" >&2
 exit 1
fi

# 5. the suite
cd tests
export SG_TEST_BIN="$PWD/../$BIN" SG_TEST_PORT="$PORT" SG_TEST_SWITCH_PORT="$SWITCH_PORT"
export SG_TEST_STATE_PORT="$STATE_PORT" SG_TEST_SWITCH=switch1
export SG_TEST_BROWSER="${SG_TEST_BROWSER:-}"
[ -d node_modules ] || npm install --no-audit --no-fund
# The two logs are deleted with the scratch directory, and the most common
# failure is one of these two processes dying — so print them before the exit
# status leaves for the caller.
set +e
node suite.mjs
status=$?
set -e
if [ "$status" != 0 ]; then
  echo "--- fake switch log:" >&2
  tail -25 "$LOG_SWITCH" >&2
  echo "--- switch-gui log:" >&2
  tail -25 "$LOG_SERVER" >&2
fi
exit "$status"
