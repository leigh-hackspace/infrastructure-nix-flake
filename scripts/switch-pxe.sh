#!/usr/bin/env bash
set -euo pipefail

sudo bash -c 'umount -f -l /exports/netboot-squashfs | true'
sudo nixos-rebuild switch --flake . --impure
sudo nixos-confirm
sudo mount -a
