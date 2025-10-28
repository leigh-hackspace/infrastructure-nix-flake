{ config, pkgs, ... }:

{
  imports = [
    ./door-entry-management-system.nix
    ./gocardless-authentik-sync.nix
    ./matrix.nix
    ./mattermost.nix
  ];
}
