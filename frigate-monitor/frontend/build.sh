#!/usr/bin/env bash
# Builds the frigate-monitor SPA into dist/ — NOT committed: the flake builds
# the bundle (machines/aibox/frigate-monitor.nix) and embeds it via build.rs.
# The shared recipe lives in common/frontend-build-spa.sh.
cd "$(dirname "$0")"
exec ../../common/frontend-build-spa.sh frigate_monitor_web
