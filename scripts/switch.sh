#!/usr/bin/env bash

sudo nixos-rebuild switch --flake . --impure

# Restart nftables if it is installed
if [ $(systemctl | grep nftables | wc -l) -gt 0 ]; then
    sudo systemctl restart nftables.service
fi

echo "Don't forget to confirm with "sudo nixos-confirm" !!!"
