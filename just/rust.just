# JUST — the cargo workspace and the five Dioxus SPAs.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- Rust crates (toolchain from `nix develop`: cargo/rustc/rustfmt/clippy, lld,
# --- just, git, alejandra, node — nothing that compiles on entry; the SPA recipes
# --- below ask for `nix develop .#wasm`, which adds the pinned wasm-bindgen-cli)
#
# Everything native is one cargo workspace (crates/Cargo.toml), so the whole set
# is `cargo <cmd> --workspace` from there: one lock file, one target dir, and one
# dependency build shared by every crate.  The wasm SPAs are deliberately not
# members (the reason is in that manifest); build one with `just build-frontend`.

# Recreate the git-ignored symlinks the wasm SPAs need to resolve their shared
# path dependencies (frontend/dto, filestore's frontend/common-rs).  The native
# crates need nothing: their path dependencies resolve inside the workspace.
# Run once after a fresh clone; `nix develop` does it automatically.
shared-rs:
    @bash common/shared-rs-links.sh

# cargo test over every crate in the workspace.
# (The default devShell: substituted, not built — the SPAs are not workspace members.)
test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c 'cd crates && cargo test --workspace --offline'

# cargo clippy over the same set.  Warnings are reported, not fatal: the point is to see
# them (they used to go entirely unchecked).  A **failure** is fatal, though, and it was
# not: the body used to pipe clippy into `grep … || true`, so the recipe exited 0 whatever
# clippy said, and a lint gate that cannot fail is worse than no gate at all.  The status is
# now carried out of the filter explicitly.
clippy: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c '
      set -euo pipefail
      cd crates
      out=$(mktemp)
      trap "rm -f \"$out\"" EXIT
      status=0
      cargo clippy --workspace --all-targets --offline >"$out" 2>&1 || status=$?
      # One Compiling/Checking line per crate is most of the output and none of the
      # information; the diagnostics and the linter summary are what is wanted.
      # No apostrophes anywhere inside this single-quoted `bash -c` argument: one closes
      # the string and the outer shell then evaluates the rest — which is exactly what
      # happened here from 2026-10-25 until 2026-11-16, when a comment reading "clippy" +
      # possessive made the whole tail of the script belong to the outer shell and
      # `just clippy` failed on every run with `out: unbound variable` at the first line
      # after it. A gate that exits 1 for a reason unrelated to linting trains people to
      # ignore it, so: if this recipe needs prose with an apostrophe, put it OUTSIDE the
      # quoted script (above), not inside.
      grep -vE "^(Compiling|Checking|Finished|    Finished)" "$out" || true
      exit $status
    '

# --- frontend (the five Dioxus SPAs) ---

# Rebuild a Dioxus SPA into its frontend/dist using the wasm devShell
# (`nix develop .#wasm`: cargo/rustc/lld + the pinned wasm-bindgen-cli).  Same recipe the
# flake runs (common/frontend-build-spa.sh vs CRANE.wasmSpa), so the local dist/ matches
# the one it embeds; those dist/ directories are git-ignored and rebuilt at
# deploy time, so this is only for local iteration.
#
#   just build-frontend                 # all five
#   just build-frontend filestore       # one of them
# (just 1.51 passes any extra argument straight into the single parameter, so
# there are no flags here — a bad name just fails the whitelist check.)
build-frontend crate="all": _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # The parameter arrives through just's quote() — shell-quoted, and the case
    # below is a whitelist anyway.
    case {{ quote(crate) }} in
        all) crates="frigate-monitor filestore gocardless-dashboard network-status switch-gui" ;;
        *) crates={{ quote(crate) }} ;;
    esac
    for c in $crates; do
        if [ ! -f "crates/$c/frontend/build.sh" ]; then
            echo "no crates/$c/frontend/build.sh — expected one of: frigate-monitor filestore gocardless-dashboard network-status switch-gui all" >&2
            exit 2
        fi
        echo "=== build-frontend $c"
        nix develop .#wasm --command bash -c "exec ./crates/$c/frontend/build.sh"
    done
