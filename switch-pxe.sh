#!/usr/bin/env bash
set -euo pipefail

# Convenience script for updating the "pxe-server" flake and rebuilding the system

nix flake update pxe-server
sudo umount -f -l /exports/pxe-server-squashfs
sudo nixos-rebuild switch --flake . --impure
sudo nixos-confirm
sudo mount -a
