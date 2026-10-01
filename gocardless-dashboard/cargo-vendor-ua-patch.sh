# Patch nixpkgs' cargo vendor fetcher to send a cargo-style user agent.
#
# nixpkgs' rustPlatform.fetchCargoVendor downloads crate tarballs from
# crates.io's API using python-requests' default user agent
# ("python-requests/x.y.z"), which crates.io refuses from our network (HTTP
# 403).  A cargo-style UA is accepted, so this script copies the fetcher
# (a plain-text python script) into the current directory, rewrites it to
# use such a UA, and puts the copy first on PATH.
#
# This file is *sourced* from the preBuild of every fetchCargoVendor call in
# machines/aibox/frigate-monitor.nix (the SPA and wasm-bindgen-cli vendors)
# so the PATH export survives into the build phase.  The fixed-output result
# is cached in the store afterwards, so this only runs when a vendor
# derivation is fetched for the first time.

util="$(command -v fetch-cargo-vendor-util)"
cp "$util" ./fetch-cargo-vendor-util
chmod +w ./fetch-cargo-vendor-util
sed -i \
  '/^import requests$/a requests.utils.default_user_agent = lambda: "cargo/1.95.0 (nix)"' \
  ./fetch-cargo-vendor-util
export PATH="$PWD:$PATH"
