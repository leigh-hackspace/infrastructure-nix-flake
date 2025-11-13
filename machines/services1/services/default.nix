{ config, pkgs, ... }:

{
  imports = [
    ./affine.nix
    ./cockpit.nix
    ./door-entry-management-system.nix
    ./gocardless-authentik-sync.nix
    ./headscale.nix
    ./matrix.nix
    ./mattermost.nix
    ./outline.nix
    ./redis.nix
  ];
}
