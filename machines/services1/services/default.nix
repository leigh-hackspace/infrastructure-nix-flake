{ config, pkgs, ... }:

{
  imports = [
    ./door-entry-management-system.nix
    ./gocardless-authentik-sync.nix
    ./headscale.nix
    ./matrix.nix
    ./mattermost.nix
  ];
}
