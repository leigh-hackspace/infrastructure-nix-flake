#!/usr/bin/env bash

sudo nixos-rebuild switch --flake . --impure

sudo systemctl restart nftables.service

echo "Don't forget to confirm with "sudo nixos-confirm" !!!"
