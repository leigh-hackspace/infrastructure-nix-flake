#!/usr/bin/env bash
# Builds the gocardless-dashboard SPA into dist/ — NOT committed: the flake
# builds the bundle (machines/services1/services/gocardless-dashboard.nix) and
# embeds it via build.rs.  The shared recipe lives in
# common/frontend-build-spa.sh.
cd "$(dirname "$0")"
exec ../../common/frontend-build-spa.sh gocardless_dashboard_web
