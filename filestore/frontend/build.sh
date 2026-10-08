#!/usr/bin/env bash
# Builds the filestore SPA into dist/ — NOT committed: the flake builds the
# bundle (machines/services1/services/filestore.nix) and embeds it via
# build.rs.  The shared recipe lives in common/frontend-build-spa.sh.
cd "$(dirname "$0")"
exec ../../common/frontend-build-spa.sh filestore_web
