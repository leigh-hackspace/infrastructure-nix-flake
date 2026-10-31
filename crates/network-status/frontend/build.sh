#!/usr/bin/env bash
# Builds the network-status SPA into dist/ — NOT committed: the flake builds the
# bundle (machines/gw/network-status.nix) and embeds it via build.rs.
# The shared recipe lives in common/frontend-build-spa.sh.
cd "$(dirname "$0")"
exec ../../../common/frontend-build-spa.sh network_status_web
