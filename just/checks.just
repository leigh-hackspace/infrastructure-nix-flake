# JUST — the gate: evaluation, the assertion evals, the file-size rule, the formatter check.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- checks and the test surface ---

# Nix-level checks: the flake evaluates, the machines' option assertions hold, the
# hand-rolled config serializers and the generated printer-state tables match their
# expectations, and every tracked .nix file is alejandra-clean.  Fast (no compilation) —
# except that its `nix flake check` step is not eval-only in practice and is skipped on `gw`
# with the reason printed, so read that recipe before assuming what a green `just check` on
# the router covered.  The rest of the test surface: `just test` (cargo tests),
# `just clippy` (lints), `just filestore-test` (headless-browser suite) and
# `just check-build` (the deploy-readiness build).
check: check-eval
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(hostname)" = "gw" ]; then
        echo "=== nix flake check: SKIPPED on gw"
        echo "    It cannot pass here and pretending otherwise costs a confusing failure:"
        echo "    evaluating the machines reads a derivation (crane stubs each SPA's source tree"
        echo "    at eval time, because a SPA's path dependencies live outside its directory), so"
        echo "    the line needs one trivial source copy and --no-build refuses to do it —"
        echo "    directly or over the hop to aibox.  The router's gate is 'just check-eval',"
        echo "    which just ran; run the full 'just check' on aibox or services1."
        echo "    The class, the two doors still open, and the honest detector:"
        echo "    docs/nix-dev-workflow.md, 'Closed 2026-10-25, one read at a time'."
        exit 0
    fi
    echo "=== nix flake check"
    # Not eval-only in practice: on a flake source nothing has evaluated yet it needs the stub
    # described above, so a fresh worktree or clone fails this line on *any* box, warm store or
    # not, and one full evaluation (or any deploy) writes them and it passes. Which is why
    # `check-eval` exists as the half you can always trust.
    nix flake check --no-build

# The eval-only half of `just check`, and the whole gate on `gw`: the assertion evals and the
# formatter check.  None of it evaluates a *machine*, so none of it can be made to build
# something by a crane helper that reads sources at eval time — the property that makes this
# the honest thing to run on the router, where evaluation is paid for by the box that answers
# DNS for the network (AGENTS.md, "Don't build on the router").
check-eval:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== printer-state tables (exporter <-> dashboards/alerts)"
    nix eval --impure --raw -f common/check-printer-states.nix
    # The router's two network tables: common/vlans.nix and common/dns-records.nix
    # generate dnsmasq's conf, nftables' rules and the VLAN devices, so a VLAN id
    # that is not in the first table or an address claimed twice is a client that
    # gets the wrong address rather than a build failure.  The same assertions run
    # as a module (machines/gw/lib/assert-network-tables.nix) and so inside `check`'s
    # `nix flake check` step; this is the version that prints which record is wrong.
    echo "=== router network tables (vlans <-> dns-records <-> ACL)"
    nix eval --impure --raw -f machines/gw/lib/check-network-tables.nix
    # The instructions themselves: every `just <name>` in AGENTS.md/README.md/justfile and the
    # `just/` modules the justfile imports is a recipe, every repo path they name exists,
    # every docs/*.md is linked from somewhere, and AGENTS.md does not call the router a
    # shadow. Eval-only, like the
    # three above, so it is safe on gw — which is the reason it is a Nix evaluator and
    # not the compiled linter the audit proposed (see its header).
    echo "=== doc pointers (recipes, paths, docs, the router's own status)"
    nix eval --impure --raw -f common/check-doc-pointers.nix
    # The gatus endpoint guard is a module assertion in
    # machines/services1/services/monitoring.nix, so `check`'s `nix flake check` step
    # already runs it: every https *.int endpoint in common/monitoring-endpoints.nix
    # must be a vhost services1 serves.  Worth naming here because the guard now
    # lives on a different machine from gatus itself, so it is easy to forget which
    # eval catches a drift.
    # The voice assistant's device boundary: no host, URL, vendor name or child process in
    # the turn logic, and the adapters still there to own those facts instead.  This is what
    # keeps "a new microphone is a new driver, not a refactor" true after the next person
    # finds it easier to test `source.starts_with("rtsp://")` in the loop than to write one
    # (docs/esp32-voice-satellites.md is the contract the boundary is drawn for).
    echo "=== voice platform boundary (core <-> drivers)"
    nix eval --impure --raw -f common/check-platform-boundary.nix
    # AGENTS.md rule 26.  Eval-only — one `wc -l` over the tracked tree — so it is in
    # this half rather than left for `check`, which the router cannot finish: a file
    # is not allowed to grow past 1000 lines on the way past the router.
    echo "=== source file sizes (AGENTS.md rule 26)"
    just check-source-size
    echo "=== alejandra --check"
    just fmt-check

check-source-size:
    #!/usr/bin/env bash
    # AGENTS.md rule 26: no source file over 1000 lines unless it can be helped.
    # Four files are already over and are named here rather than hidden — a new one
    # fails, and if one of these gets shorter the guard says so, because a baseline
    # nobody prunes is a baseline nobody trusts.  Generated and machine-owned trees
    # are skipped: `*/dist/*` is a minified bundle.  Prose is not source, and a Nix
    # data table is NOT exempt — `monitoring-dashboards.nix` is on the baseline.
    #
    # `justfile` and `just/*.just` are on the list because the rule was blind to them for
    # a year: `git ls-files` here was given extensions only, and a bare `justfile` has no
    # extension, so the file that broke rule 26 was the one file rule 26's own guard could
    # not see — 2338 lines, and nothing said so.
    set -euo pipefail
    baseline="crates/switch-gui/src/switch.rs crates/switch-gui/src/shadow.rs crates/filestore/src/server.rs machines/services1/services/monitoring-dashboards.nix"
    over=$(git ls-files 'justfile' '*.just' '*.rs' '*.nix' '*.ts' '*.tsx' '*.sh' '*.cjs' '*.py' '*.js' '*.css' \
        | grep -v '/dist/' \
        | xargs -r wc -l 2>/dev/null \
        | grep -v ' total$' \
        | awk '$1 > 1000 { print $2 }' | sort)
    offenders=""
    for f in $over; do
        case " $baseline " in
            *" $f "*) ;;
            *) offenders="$offenders $f" ;;
        esac
    done
    for f in $baseline; do
        if [ ! -f "$f" ]; then
            echo "  gone from the tree, drop it from the baseline: $f" >&2
        elif [ "$(wc -l < "$f")" -le 1000 ]; then
            echo "  under 1000 now, drop it from the baseline: $f ($(wc -l < "$f"))" >&2
        fi
    done
    if [ -n "$offenders" ]; then
        echo "over 1000 lines and not on the baseline — split it (AGENTS.md rule 26):" >&2
        for f in $offenders; do echo "  $(wc -l < "$f") $f" >&2; done
        exit 1
    fi
    echo "source sizes ok: $(echo "$over" | grep -c . || true) file(s) over 1000, all on the baseline"
