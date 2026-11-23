# JUST — the hackspace Slack agent: the tools, the fixture, the digest, the status.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- the hackspace Slack agent ---------------------------------------------------
#
# `hackspace-mcp` (the tools) and `slack-agent` (the harness). The design, the
# measurements behind it and the phase plan are in docs/slack-agent-2026-11-16.md.
#
# Every verb here proves something **without posting to Slack**, which is the
# constraint the whole build order was written around: the Slack leg is the last
# thing switched on, and `--dry-run` rehearses it while sending nothing.
#
# Run them on services1 (read `hostname` first — rule 27). `tools`, `call` and `ask`
# talk to the *deployed* MCP server on 127.0.0.1:8191, and that unit is the only
# process holding the Home Assistant token, so no verb here needs a secret in the
# environment, let alone in argv.

slack-agent-tools: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # Non-zero if a tool group is missing: a bot that cannot see Home Assistant will
    # still answer, and it will be wrong with confidence.
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- tools'

slack-agent-probe: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # Which engine answers, which model id it is serving right now, how fast it decodes.
    # Run after `nix flake update strata`: that bump moves both engines at once, and the
    # ids here are discovered from /v1/models rather than declared anywhere.
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- probe'

# Call one tool on the running MCP server, raw. No model, no token, no Slack — this
# is the verb that answers "is the workshop temperature tool broken, or is the model".
slack-agent-call tool="space_status" arguments='{}': _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # Through the environment rather than interpolated into a quoted shell string:
    # the arguments are JSON, and JSON is full of the characters a nested quote level
    # eats (measured: `'{"search":"cam8"}'` arrived as `{search:cam8}`).
    # Tool names and JSON arguments contain no apostrophes by construction (JSON uses
    # double quotes), so one level of single quoting is enough here.
    export MCP_TOOL='{{tool}}'
    export MCP_ARGS='{{arguments}}'
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- call "$MCP_TOOL" "$MCP_ARGS"'

slack-agent-ask question: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # The whole loop, printed: engine chosen, tool calls made, tokens, and the answer.
    # A quoted heredoc, and `ask -`: just has no shell-quoting builtin at this pin, and
    # prose through two shell levels loses its apostrophes (measured: "what's on
    # tonight?" arrives mangled). Stdin cannot be mangled by quoting, and the delimiter
    # being quoted means `$` and backticks in a question stay text.
    # The body and the terminator are indented with the recipe: just dedents recipe
    # lines, and an unindented line would end the recipe.
    cat <<'AGENT_QUESTION' | nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- ask -'
    {{question}}
    AGENT_QUESTION

# The fixture: the questions the bot is supposed to be able to answer, run one by one
# with timing, for a human to read. `nix flake update strata`, a tool change or a
# prompt edit all move answers, and "it seemed fine" is not how this fleet decides
# that. Hand-scored by design: what "correct" means for "is it cold" is a judgement
# about a building, and the day that can be asserted is the day it becomes a test.
slack-agent-eval: _build-host-guard
    #!/usr/bin/env bash
    set -uo pipefail
    cd crates
    cargo build -q -p slack-agent --offline
    pass=0; fail=0
    while IFS= read -r line; do
        case "$line" in ''|'#'*) continue ;; esac
        echo "=============================================================="
        echo "Q: $line"
        started=$(date +%s)
        out=$(./target/debug/slack-agent ask "$line" 2>&1) || true
        printf '%s\n' "$out" | sed -n '/^answer:/,$p'
        elapsed=$(( $(date +%s) - started ))
        if printf '%s\n' "$out" | grep -q '^answer:'; then
            pass=$(( pass + 1 )); echo "[answered in ${elapsed}s]"
        else
            fail=$(( fail + 1 )); echo "[NO ANSWER in ${elapsed}s] $(printf '%s\n' "$out" | tail -2)"
        fi
    done < slack-agent/questions.txt
    echo "=============================================================="
    echo "$pass answered, $fail not. Read them: only you know if they are right."

# The deterministic calendar path, offline: which tool call a question would produce and
# exactly what the bot would post. No model, no engine, no Slack.
slack-agent-events question: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # stdin for the same reason `ask` takes it: just has no quoting builtin at this pin
    # and "what's on tonight?" loses its apostrophe through two shell levels.
    cat <<'AGENT_QUESTION' | nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- events -'
    {{question}}
    AGENT_QUESTION

# Preview the Monday digest — the exact text and the resolved channel id, posted nowhere.
# The deployed timer runs the same command WITHOUT --dry-run, so this is the one check that
# a renamed #general or a revoked token shows up before a member notices on a Monday.
slack-agent-digest: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates && cargo build -q -p slack-agent --offline
    # The tokens go to the process environment, never argv (world-readable on Linux), and
    # nothing here prints them: the digest's own output is the calendar.
    sudo -n bash -c 'set -a; . /run/secrets/slack_agent_env; set +a; ./target/debug/slack-agent digest --dry-run'

# What the two units, the socket and the engines are doing right now.
slack-agent-status:
    #!/usr/bin/env bash
    set -uo pipefail
    for unit in hackspace-mcp.service slack-agent.service slack-agent-digest.service slack-agent-digest.timer; do
        printf '%-24s %s / %s\n' "$unit" "$(systemctl is-enabled "$unit" 2>&1)" "$(systemctl is-active "$unit" 2>&1)"
    done
    echo "--- socket and turn liveness (127.0.0.1:8192) ---"
    curl -fsS -m 5 http://127.0.0.1:8192/health || echo "health endpoint not answering"
    curl -fsS -m 5 http://127.0.0.1:8192/metrics | grep -E '^slack_agent_' || true
    echo "--- tool server (127.0.0.1:8191) ---"
    curl -fsS -m 5 http://127.0.0.1:8191/health || echo "mcp health endpoint not answering"
    curl -fsS -m 5 http://127.0.0.1:8191/metrics | grep -E '^hackspace_mcp_' || true
    echo "--- recent journal ---"
    journalctl -u slack-agent.service -n 15 --no-pager -o cat 2>/dev/null | tail -15

# Serve with the real Slack events but **nothing posted**. This is the last check
# before the bot speaks to people: the events flow, the decisions log, and no member
# of the workspace can tell the difference. Ctrl-C ends it.
slack-agent-dry-run: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- serve --dry-run'
