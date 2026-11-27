# JUST — netd: the DHCPv4 / IPv6-RA / DNS daemon that replaces dnsmasq.
#
# The daemon is `crates/netd`, its configuration is rendered by
# `machines/gw/netd-config.nix`, and it is **not deployed**: `services.netd.enable`
# is false on gw and `services.dnsmasq` still owns ports 53 and 67. What these
# recipes do is make the replacement provable without switching it on — the render
# is pure Nix, and the binary is the same one the unit would run.
#
# `docs/netd-2026-11-26.md` is the plan, the reasons, and the cutover runbook.

# Render gw's netd configuration and print it. Pure evaluation of the tables — no
# build, no socket, no daemon — so it works on the router and is the fastest way to
# see what a table change did to the document the daemon would read.
# (The same value `services.netd.config` holds, and the input to both recipes below.)
# Render gw's netd document from the tables and print it — no build, no socket.
netd-render:
    #!/usr/bin/env bash
    set -euo pipefail
    nix eval --impure --json -f machines/gw/netd-config.nix

# The gate: the daemon's own tests, then `netd check` over the *rendered* config.
# This is the thing that proves the Nix side and the Rust side agree — a schema
# this repo's renderer cannot satisfy fails here rather than on the switch that
# turns the daemon on. Build-shaped, so it refuses on gw like every other
# compile-adjacent recipe (`docs/nix-dev-workflow.md`, "The router builds for
# itself, bounded").
# Run the crate tests, then `netd check` over the rendered gw config.
netd-check: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    cfg=$(mktemp)
    trap 'rm -f "$cfg"' EXIT
    nix eval --impure --json -f machines/gw/netd-config.nix > "$cfg"
    nix develop --command bash -c "cd crates && cargo test -p netd --offline && cargo run -q -p netd --offline -- check --config $cfg"

# What would netd hand to one device, and why? Runs the real decision function over
# the real rendered config and the real persisted lease table, and prints the
# reservation or pool it would use, the tags it carries, the boot file if any, and
# every DHCP option in the reply. The answer to "why did this laptop get .173".
#
#   just netd-explain mac=c8:d3:ff:a5:b2:25
#   just netd-explain mac=02:00:00:00:00:01 interface=vlan226 request=1
# Answer "what would netd tell this device, and why" from the real config.
netd-explain mac interface="vlan225" request="" client_id="" requested="" hostname="":
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    cfg=$(mktemp)
    trap 'rm -f "$cfg"' EXIT
    nix eval --impure --json -f machines/gw/netd-config.nix > "$cfg"
    args=(--config "$cfg" --interface {{ quote(interface) }} --mac {{ quote(mac) }})
    [ -n {{ quote(request) }} ] && args+=(--request)
    [ -n {{ quote(client_id) }} ] && args+=(--client-id {{ quote(client_id) }})
    [ -n {{ quote(requested) }} ] && args+=(--requested {{ quote(requested) }})
    [ -n {{ quote(hostname) }} ] && args+=(--hostname {{ quote(hostname) }})
    nix develop --command bash -c "cd crates && cargo run -q -p netd --offline -- explain ${args[*]}"

# Ask a DHCP server a whole lease cycle as a throwaway client (DISCOVER → OFFER →
# REQUEST → ACK → RELEASE) and print every option it answered with. This is the
# cutover's comparison: run it against dnsmasq today and against netd tomorrow, and
# the diff is the answer to "did anything change". Needs root and a device; it
# binds the client port 68 on that interface (the router is not a DHCP client on
# any VLAN, so the port is free).
#
#   just netd-probe interface=vlan226
# Run a whole DHCP lease cycle as a throwaway client and print every option.
netd-probe interface="vlan226" mac="" timeout="4": _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    cfg=$(mktemp)
    trap 'rm -f "$cfg"' EXIT
    nix eval --impure --json -f machines/gw/netd-config.nix > "$cfg"
    args=(--config "$cfg" --interface {{ quote(interface) }} --timeout {{ quote(timeout) }} --verbose)
    [ -n {{ quote(mac) }} ] && args+=(--mac {{ quote(mac) }})
    sudo nix develop --command bash -c "cd crates && cargo run -q -p netd --offline -- probe ${args[*]}"

# Is the daemon answering, here? Reads `/v1/status`: uptime, which interfaces it is
# attached to, which are missing, the lease count and the counters. It says so
# plainly while the daemon is not deployed, rather than printing a curl error.
# Print what the daemon says it is doing (uptime, interfaces, counters).
netd-status:
    #!/usr/bin/env bash
    set -euo pipefail
    url="http://10.3.1.1:8092/v1/status"
    if ! out=$(curl -sS -m 3 "$url" 2>/dev/null); then
        echo "netd is not answering on $url — services.netd.enable is false on gw until the cutover (docs/netd-2026-11-26.md), so dnsmasq is still the box's DHCP and DNS" >&2
        exit 1
    fi
    printf '%s\n' "$out"

# The lease table as the daemon holds it — the replacement for reading
# /var/lib/dnsmasq/dnsmasq.leases. Same address as netd-status.
# Print the lease table as the daemon holds it.
netd-leases:
    #!/usr/bin/env bash
    set -euo pipefail
    url="http://10.3.1.1:8092/v1/leases"
    if ! out=$(curl -sS -m 3 "$url" 2>/dev/null); then
        echo "netd is not answering on $url — services.netd.enable is false on gw until the cutover (docs/netd-2026-11-26.md)" >&2
        exit 1
    fi
    printf '%s\n' "$out"
