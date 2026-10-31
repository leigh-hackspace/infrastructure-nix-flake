#!/usr/bin/env bash
# (Re)creates the git-ignored symlinks the wasm SPAs need to resolve their
# shared path dependencies.
#
# The native crates do not need this any more: `crates/` is one cargo workspace
# and a shared crate is reached by a real relative path (`path =
# "../common-rs/json"`), which resolves in a checkout with no help.
#
# A SPA is a different story.  Each `frontend/` is its own cargo workspace (it is
# built for wasm32 against a pinned wasm-bindgen, and must not share the native
# workspace's lockfile — see crates/Cargo.toml), and cargo lets a package belong
# to exactly one workspace.  `frontend/dto` therefore cannot simply be
# `path = "../dto"`: that crate is a member of the native workspace, and cargo
# refuses it with "is a member of the wrong workspace".  A symlink *into* the
# SPA's own directory makes it a crate of the SPA's workspace instead, which is
# the same source file under a different workspace — exactly what a shared wire
# type needs.  The Nix build materialises a real copy the same way (`sharedCrates`
# in common/crane.nix), because a symlink cannot travel in a flake source.
#
# Run `just shared-rs` after a fresh clone; `nix develop` does it automatically.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

# `nix develop` may run the copy of this script that lives in the shell's store
# path, where symlinking is pointless (and read-only).  Only act in a checkout.
if [ ! -w "$root" ] || [ ! -d "$root/crates" ]; then
    echo "not a writable checkout ($root); nothing to do" >&2
    exit 0
fi

# The wasm frontends share their path dependencies this way: the wire-type crate
# each SPA renders, filestore's SPA reading the preview table its API uses, and the
# component library (`common-rs/ui`, the vendored dioxus-components subset — see
# docs/component-library.md) for whichever SPAs have been converted to it.
ln -sfn ../dto "$root/crates/gocardless-dashboard/frontend/dto"
ln -sfn ../dto "$root/crates/switch-gui/frontend/dto"
ln -sfn ../../common-rs "$root/crates/filestore/frontend/common-rs"
ln -sfn ../../common-rs "$root/crates/switch-gui/frontend/common-rs"
ln -sfn ../../common-rs "$root/crates/network-status/frontend/common-rs"

echo "SPA path-dependency symlinks are in place"
