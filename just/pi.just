# JUST — pi on each box: the inference endpoints it may dial and the web search it may use.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- pi's inference endpoints (common/pi-agent.nix, docs/pi-agent-models.md) ------
#
# `programs.pi-agent` declares the network's inference endpoints once
# (common/pi-endpoints.nix) and `crates/pi-agent-sync` asks each one, on every box,
# which models it is running and with how large a context, writing
# `~/.pi/agent/models.json`. A timer does it every 15 minutes; these recipes are
# the operator's half — read what the last run saw, or make it run now. All of them
# invoke the **installed** binary, so none is build-shaped and all are safe on the
# router.
#
# There is deliberately no recipe that edits `models.json`: it is generated, and a
# hand-written provider is preserved by id but overwritten the moment an endpoint of
# that name is probed. Put an endpoint in the table instead.
pi-agent-status user="leigh-admin":
    @pi-agent-sync --report "/home/{{user}}/.pi/agent/pi-agent-status.json"

pi-agent-sync user="leigh-admin":
    #!/usr/bin/env bash
    set -euo pipefail
    unit="pi-agent-{{user}}.service"
    if ! systemctl list-unit-files "$unit" --no-legend | grep -q .; then
        echo "$unit is not a unit on $(hostname): programs.pi-agent is not enabled here,"
        echo "or this box has not switched to a generation that has it. 'just pi-agent-status'"
        echo "is the other half and needs no unit at all."
        exit 1
    fi
    # A oneshot: `restart` blocks until it exits. A dead engine is not a failed unit
    # (the tool exits 0 and records the failure in the status file), so `is-failed`
    # here means the config or the filesystem, and the journal is the answer.
    sudo systemctl restart "$unit"
    if systemctl is-failed "$unit"; then
        echo "$unit failed:"
        journalctl -u "$unit" -n 30 --no-pager
        exit 1
    fi
    pi-agent-sync --report "/home/{{user}}/.pi/agent/pi-agent-status.json"

# --- pi's web search (common/pi-search.nix, docs/pi-web-search.md) ----------------
#
# `programs.pi-agent.search.brave` (default: on wherever `programs.pi-agent` is on)
# puts one Brave key in sops, gives it to one group, declares one pinned pi package,
# and owns the two keys of `~/.pi/agent/web-search.json` that say which provider to use
# and where its credential comes from. `just pi-agent-sync` is what applies the last
# two; this recipe is the one that answers "is it working, as me, here" — the four
# facts a "the agent has no web search" report is hiding behind, in the order they
# actually break.
#
# It prints no secret: the key is read into a curl config handed to curl on **stdin**,
# never into its arguments (a command line is world-readable on Linux), and the only
# things said about it are its mode, its group and its HTTP status. Not build-shaped,
# so it is safe on gw.
pi-search-status user="leigh-admin":
    #!/usr/bin/env bash
    set -uo pipefail
    agent="/home/{{user}}/.pi/agent"
    key="${BRAVE_API_KEY_FILE:-/run/secrets/brave_api_key}"

    echo "=== $(hostname), as $(whoami)"
    echo "--- the credential"
    if [ -r "$key" ]; then
        echo "  readable: $key ($(stat -c 'mode %a, owner %U, group %G' "$key"), $(wc -c < "$key" | tr -d ' ') bytes)"
    else
        echo "  NOT READABLE: $key"
        echo "  it is 0440 root:brave-search-readers, and that group's members are"
        echo "  programs.pi-agent.users. $(whoami) is in:"; id -nG | sed 's/^/    /'
        if getent group brave-search-readers | grep -q "$(id -un)"; then
            # The common case, and not a permissions bug: /etc/group changed at the
            # switch, and a process keeps the supplementary groups it was started
            # with — so the tmux server, and every pi inside it, predates the grant.
            echo "  BUT /etc/group DOES list $(id -un): this session (and its tmux server,"
            echo "  and the pi inside it) predates the switch. New logins are fine; to fix"
            echo "  the running one, restart the tmux server (or log out and back in)."
        else
            echo "  and /etc/group does not list this account either, so this box has not"
            echo "  switched to a generation with common/pi-search.nix, or the user is not in"
            echo "  programs.pi-agent.users."
        fi
    fi

    echo "--- a search with it (Brave, count=1)"
    if [ -r "$key" ]; then
        code=$( { printf 'silent\nshow-error\nmax-time = 20\noutput = "/dev/null"\nurl = "https://api.search.brave.com/res/v1/web/search?q=nixos&count=1&text_decorations=false"\nwrite-out = "%%{http_code}"\nheader = "Accept: application/json"\nheader = "X-Subscription-Token: %s"\n' "$(cat "$key")"; } | curl --config - 2>/dev/null)
        case "$code" in
            200) echo "  200 ok — the key works and this box can reach the API" ;;
            401 | 403) echo "  $code — Brave refused the key. It is the one in sops; check it is still the current one (docs/follow-ups.md)." ;;
            402) echo "  402 — the plan behind the key has no credit left." ;;
            429) echo "  429 — rate limited. Brave's own window; try again in a minute." ;;
            000 | "") echo "  no answer — no route or no DNS from this box (services3's uplink is a USB dongle; the WAN backup gives no inbound but does give out)." ;;
            *) echo "  $code — unexpected; curl's body was thrown away (output=/dev/null), so re-run by hand if it repeats." ;;
        esac
    else
        echo "  skipped: no credential to ask with"
    fi

    echo "--- the two files the timer merges"
    for f in settings.json web-search.json; do
        if [ -e "$agent/$f" ]; then
            printf '  %-16s %s\n' "$f" "$(stat -c 'mode %a, owner %U:%G' "$agent/$f")"
        else
            printf '  %-16s MISSING\n' "$f"
        fi
    done
    if grep -q 'pi-web-access' "$agent/settings.json" 2>/dev/null; then
        echo "  pi-web-access is declared: $(grep -o 'npm:pi-web-access[^"]*' "$agent/settings.json" | head -1)"
    else
        echo "  pi-web-access is NOT declared in settings.json — the timer has not run, or it refused to touch the file: 'just pi-agent-sync' prints why."
    fi
    grep -o '"provider": *"[a-z-]*"' "$agent/web-search.json" 2>/dev/null | head -1 | sed 's/^/  /' || echo "  no provider line in web-search.json"

    echo "--- pi's own view (it installs the package on first start, from the npm registry)"
    if command -v pi >/dev/null 2>&1; then
        pi list 2>&1 | tail -4
    else
        echo "  pi is not on this account's PATH, so there is nothing to install it."
    fi
