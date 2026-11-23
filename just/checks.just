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
    # The grep drop-in: what `grep` is on this box's PATH against the gnugrep in the
    # closure, over the invocations the justfile and the guards actually run (common/tools.nix
    # supplies it as a symlink named `grep`, and ugrep's compat mode is keyed to argv[0]).
    # Greps and one `nix-store` query: eval-safe, so it belongs in this half and runs on
    # `gw` too — the box whose `grep -rl '^# PARKED' machines/` is the list of what is not
    # running (AGENTS.md rule 23).
    echo "=== grep drop-in (gnugrep <-> the grep on PATH)"
    just grep-compat
    echo "=== source file sizes (AGENTS.md rule 26)"
    just check-source-size
    echo "=== alejandra --check"
    just fmt-check

# The parity gate for `grep` itself (common/tools.nix installs ugrep under that NAME).
# One rule: **whatever this box's PATH calls `grep` today must agree with the gnugrep
# in the same closure on every invocation this repo runs**, because the swap is
# invisible: agents, this justfile and the guards all call `grep` and none of them ask
# which one they got, so the real grep is read out of the closure by absolute store
# path - after the swap, the word `grep` in a script *is* the thing under test.
# Differences are whitelisted by CLASS rather than by line, and the per-class counts
# are asserted below: a case that starts differing outside the classes is a finding,
# and a class that stops differing makes docs/grep-dropin.md a lie.  Both fail rather
# than quietly re-tune a list someone forgets; the classes and the receipts behind each
# are in that doc.  Greps and one `nix-store` query, so this belongs in the eval-only
# half and runs on `gw` too, whose `grep -rl '^# PARKED' machines/` is the list of what
# is not running (AGENTS.md rule 23).
grep-compat:
    #!/usr/bin/env bash
    set -euo pipefail
    sw=$(readlink -f /run/current-system/sw)
    gnugrep=$(nix-store -q --references "$sw" 2>/dev/null | grep -e 'gnugrep-[0-9]' | grep -v 'info$' | head -1)
    if [ -z "$gnugrep" ] || [ ! -x "$gnugrep/bin/grep" ]; then
        echo "no gnugrep in the closure — the gate has nothing to compare against, and silence is not a pass" >&2
        exit 1
    fi
    real="$gnugrep/bin/grep"
    new=$(command -v grep)
    [ -n "$new" ] || new="$real"
    if [ "$(readlink -f "$new")" = "$(readlink -f "$real")" ]; then
        echo "not deployed on this box: PATH's grep resolves to the same binary as the closure's gnugrep, so a comparison would prove nothing.  For the preview, put a directory holding a \`grep\` symlink to ugrep first on PATH (docs/grep-dropin.md) — that is what the switch will make PATH's grep resolve to."
        exit 0
    fi
    case "$new" in *gnugrep*) engine="gnugrep (drop-in not deployed here)" ;; *) engine="the drop-in ($new)" ;; esac
    fix=$(mktemp -d)
    printf '  Endpoint: 7\nEndpoint: 8\nnoise\n' > "$fix/f2"
    printf 'a+ here\nplain a plus\naaa+\n' > "$fix/f1"
    printf 'nul\000byte NEEDLE3\nother\n' > "$fix/fbin"
    # Each line: want:::label:::command:::why, with $E where the engine goes and $fix the
    # fixture dir — both expanded at eval time, so the same text runs under both engines.
    # The commands are transcribed from what this repo actually runs: `cam-check`/`cam-set`
    # read an address out of common/dns-records.nix with -A4 and pipe it to -oE; `cam-fw-verify`
    # compares grep -c "^mtd" and grep -c ""; the pbx health line counts -cE through a
    # [[:space:]] class; check-source-size pipes -v ' total$'; AGENTS.md rule 23's list of
    # what is not running is grep -rl '^# PARKED' machines/; a unit's ExecStart asks
    # `grep -qi thingino /etc/os-release`.  `ok` must agree byte-for-byte and on exit
    # status.  The rest are the differences the docs name, and the counts are asserted
    # below: a case that starts differing that the whitelist does not name is a finding,
    # and a whitelisted case that stops differing makes the doc a lie — both fail.
    cases="
    order:::recursive needles (the shape agents type):::\$E -rn dnsmasq common/:::
    order:::what is parked (AGENTS.md rule 23):::\$E -rl '^# PARKED' machines/:::
    order:::path order in a recursive grep:::\$E -rn dnsmasq docs/ common/:::GNU gives readdir order, compat mode sorts by path — same lines, different order
    first:::head -1 of a multi-file grep:::\$E -rh dnsmasq docs/ common/ | head -1:::a recipe taking head -1 of a multi-file grep names a different file
    notice:::the binary-file notice:::printf x | \$E -n NEEDLE3 \"$fix/fbin\":::GNU writes it to stderr, ugrep writes \`Binary file … matches\` to stdout
    rc2:::a directory without -r:::\$E -q nosuchneedle docs/:::GNU errors (rc 2, \`Is a directory\`); compat mode recurses (rc 1)
    ok:::address window (-A4, cam-check's read):::\$E -A4 '^  cam5 = {' common/dns-records.nix:::
    ok:::address extraction (-A4 | -oE chain):::\$E -A4 '^  cam5 = {' common/dns-records.nix | \$E -oE '10\\.3\\.[0-9]+\\.[0-9]+' | head -1:::
    ok:::line count (grep -c \"\", cam-fw-verify):::\$E -c '' just/checks.just:::
    ok:::POSIX class (-cE, the pbx health line):::\$E -cE '^[[:space:]]+Endpoint:' \"$fix/f2\":::
    ok:::invert over a pipe (-v, check-source-size):::printf 'a total\\nb\\nc total\\n' | \$E -v ' total$':::
    ok:::quiet, no match (rc only):::\$E -q nosuchneedle just/checks.just:::
    ok:::count, no match (stdout + rc 1, the set -e shape):::\$E -c nosuchneedle docs/follow-ups.md:::
    ok:::BRE stays BRE (grep 'a+' is literal):::\$E -n 'a+' \"$fix/f2\":::compat mode keeps GNU's BRE default: + is not a quantifier
    ok:::the unit ExecStart shape (-qi on os-release):::\$E -qi thingino /etc/os-release:::
    "
    fails=0
    got_ok=0
    got_order=0
    got_notice=0
    got_rc2=0
    got_first=0
    want_first=0
    want_order=0
    want_notice=0
    want_rc2=0
    while IFS= read -r line; do
        line=${line#"${line%%[![:space:]]*}"}
        [ -n "$line" ] || continue
        want=${line%%::*}
        rest=${line#*:::}
        label=${rest%%::*}
        rest=${rest#*:::}
        cmd=${rest%%::*}
        why=${rest#*:::}
        case "$want" in
        order) want_order=$((want_order + 1)) ;;
        notice) want_notice=$((want_notice + 1)) ;;
        rc2) want_rc2=$((want_rc2 + 1)) ;;
        first) want_first=$((want_first + 1)) ;;
        esac
        rc1=0
        rc2v=0
        E="$real"
        eval "$cmd" > "$fix/o1" 2> "$fix/e1" || rc1=$?
        E="$new"
        eval "$cmd" > "$fix/o2" 2> "$fix/e2" || rc2v=$?
        same=diff
        if cmp -s "$fix/o1" "$fix/o2" && [ "$rc1" = "$rc2v" ]; then
            same=same
        elif cmp -s <(sort "$fix/o1") <(sort "$fix/o2") && [ "$rc1" = "$rc2v" ] && [ -s "$fix/o1" ]; then
            same=reorder
        elif [ -n "$why" ]; then
            same=diff-explained
        fi
        case "$want/$same" in
        ok/same)
            printf '  ok    %-48s rc=%s\n' "$label" "$rc1"
            got_ok=$((got_ok + 1))
            ;;
        order/reorder)
            printf '  order %-48s %s\n' "$label" "$why"
            got_order=$((got_order + 1))
            ;;
        first/diff-explained|first/same)
            printf '  first %-47s the first line of a multi-file grep is a different file (and the exit status a closed pipe reports differs)\n' "$label"
            got_first=$((got_first + 1))
            ;;
        notice/diff-explained)
            printf '  notice %-47s GNU->stderr, %s->stdout: a `| wc -l` over a NUL-bearing log gains a phantom line\n' "$label" "$engine"
            got_notice=$((got_notice + 1))
            ;;
        rc2/diff-explained)
            printf '  rc     %-47s GNU[%s] vs %s[%s]: %s\n' "$label" "$rc1" "$engine" "$rc2v" "$why"
            got_rc2=$((got_rc2 + 1))
            ;;
        *)
            printf '  FAIL  %s — expected %s, got %s\n' "$label" "$want" "$same"
            printf '        GNU[%s]: %s\n' "$rc1" "$(head -2 "$fix/o1" | tr '\n' ' ')"
            printf '        NEW[%s]: %s\n' "$rc2v" "$(head -2 "$fix/o2" | tr '\n' ' ')"
            fails=$((fails + 1))
            ;;
        esac
    done <<< "$cases"
    rm -rf "$fix"
    if [ "$fails" != 0 ] || [ "$got_order" != "$want_order" ] || [ "$got_notice" != "$want_notice" ] || [ "$got_rc2" != "$want_rc2" ] || [ "$got_first" != "$want_first" ]; then
        echo "$fails finding(s); whitelisted differences seen: $got_order order, $got_first first-line, $got_notice notice, $got_rc2 rc — expected $want_order/$want_first/$want_notice/$want_rc2 (docs/grep-dropin.md owns what each one means)" >&2
        exit 1
    fi
    echo "grep parity ok: $engine agrees with gnugrep on $got_ok invocation(s), and the only differences are the $((want_order + want_first + want_notice + want_rc2)) named in docs/grep-dropin.md"

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
