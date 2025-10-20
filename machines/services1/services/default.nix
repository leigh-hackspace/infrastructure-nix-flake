{ config, pkgs, ... }:

{
  imports = [
    ./door-entry-management-system.nix
    ./mattermost.nix
    ./matrix.nix
  ];
}
