#!/usr/bin/env bash
# (Re)creates the git-ignored symlinks that let cargo resolve this repo's
# in-repo path dependencies when building in a crate's own directory.
#
# The Nix build does not need them (common/crane.nix copies the real crates
# into the build), but a fresh clone does: `cargo build` inside e.g. filestore/
# reads `path = "./common-rs/oidc"` from Cargo.toml, and that directory only
# exists through this symlink.  Run `just shared-rs` after a fresh clone.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

# `nix develop` may run the copy of this script that lives in the shell's store
# path, where symlinking is pointless (and read-only).  Only act in a checkout.
if [ ! -w "$root" ] || [ ! -d "$root/common-rs" ]; then
    echo "not a writable checkout ($root); nothing to do" >&2
    exit 0
fi

for crate in dns-sync moonraker-exporter filestore gocardless-dashboard frigate-monitor status-dashboard network-status; do
    ln -sfn ../common-rs "$root/$crate/common-rs"
done

# The wasm frontends share their path dependencies the same way
# (gocardless-dashboard/frontend/dto -> ../dto, and filestore's SPA reads the
# preview table the server uses through frontend/common-rs).  Both are
# git-ignored; the flake materialises a real copy.
ln -sfn ../dto "$root/gocardless-dashboard/frontend/dto"
ln -sfn ../../common-rs "$root/filestore/frontend/common-rs"

echo "shared path-dependency symlinks are in place"
