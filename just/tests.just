# JUST — the headless-browser suites and the loopback dev server.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# Start an unauthenticated filestore on 127.0.0.1 for poking at the API/UI by
# hand. --no-auth is only honoured on a loopback bind, so this can never expose
# the store on a routable interface; the deployed service never uses it.
filestore-dev: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'cd crates && cargo build --release --offline -p filestore && exec ./target/release/filestore --root "${FS_TEST_ROOT:-/tmp/filestore-test-root}" --no-auth --port "${FS_TEST_PORT:-18097}"'

# Builds the SPA + binary, starts filestore with --no-auth on 127.0.0.1 against
# a scratch fixture, and drives the UI with headless Chromium (Playwright).
# See crates/filestore/tests/README.md.
filestore-test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#wasm --command bash -c 'exec crates/filestore/tests/run.sh'

# The switch-gui headless-browser suite: builds the SPA + binary, starts a fake
# ProCurve console (a real Telnet server with an in-memory model), points the
# service at it with --no-auth on a loopback bind and a scratch Postgres schema,
# and drives the matrix with headless Chromium.  Nothing can reach a real switch:
# the only address the service is given is 127.0.0.1.  Needs the local Postgres
# cluster (see SG_TEST_DB_URL in crates/switch-gui/tests/run.sh).
switch-gui-test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#wasm --command bash -c 'exec crates/switch-gui/tests/run.sh'

# The network-status headless-browser suite: builds the SPA + binary, starts the service on a
# loopback bind with `--local` and with the connection table and the lease file switched off,
# and drives the page with headless Chromium.  Nothing can reach the router: the two files
# that would make a result depend on the box are the router's group-read files
# (`machines/gw/conntrack-reader.nix`, `machines/gw/lease-reader.nix`), so the panels' error
# path is what is under test.  See crates/network-status/tests/README.md.
network-status-test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#wasm --command bash -c 'exec crates/network-status/tests/run.sh'
