# Patch nixpkgs' cargo vendor fetcher to send a cargo-style user agent.
#
# nixpkgs' rustPlatform.fetchCargoVendor downloads crate tarballs from
# crates.io's API using python-requests' default user agent
# ("python-requests/x.y.z"), which crates.io refuses from our network (HTTP
# 403).  A cargo-style UA is accepted, so this script copies the fetcher
# (a plain-text python script) into the current directory, rewrites it to
# use such a UA, and puts the copy first on PATH.
#
# This file is *sourced* from the preBuild of the one fetchCargoVendor call that
# still needs it — the wasm-bindgen-cli vendor in common/crane.nix — so the PATH
# export survives into the build phase.  The fixed-output result is cached in the
# store afterwards, so this only runs when a vendor derivation is fetched for the
# first time.  Crane's own crate fetcher (used for everything else) downloads
# from static.crates.io with curl, which our network allows, so it does not need
# this patch.

util="$(command -v fetch-cargo-vendor-util)"
cp "$util" ./fetch-cargo-vendor-util
chmod +w ./fetch-cargo-vendor-util
sed -i \
  '/^import requests$/a requests.utils.default_user_agent = lambda: "cargo/1.95.0 (nix)"' \
  ./fetch-cargo-vendor-util
export PATH="$PWD:$PATH"
