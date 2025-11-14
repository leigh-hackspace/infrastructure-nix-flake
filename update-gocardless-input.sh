#!/usr/bin/env bash

pushd /home/leigh-admin/Projects/gocardless-tools
git pull
popd

nix flake update gocardless-tools
