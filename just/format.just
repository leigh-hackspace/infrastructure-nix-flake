# JUST — the formatter: alejandra through the flake pin (AGENTS.md rule 6).
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- Nix formatting ---

# Format every tracked .nix file with alejandra (the repo standard: it is what
# .zed/settings.json runs on save).  `nix fmt` takes the formatter from the flake's
# pinned nixpkgs (flake.nix's `formatter`), i.e. from the binary cache — NOT from a
# devShell, because the wasm shell would build the pinned wasm-bindgen-cli and a
# formatter must never be the reason a box compiles Rust (see `_build-host-guard`).
# The whole tree is kept clean so `nix fmt -- --check` is a real gate rather than a
# warning about the last file someone touched.
fmt:
    nix fmt -- $(git ls-files '*.nix')

# The check half of the same rule, wired into `just check`.
fmt-check:
    #!/usr/bin/env bash
    set -euo pipefail
    nix fmt -- --check $(git ls-files '*.nix')
