{ config, pkgs, ... }:

{
  imports = [
    ./affine.nix
    ./cockpit.nix
    ./door-entry-management-system.nix
    ./frigate.nix
    ./gocardless-authentik-sync.nix
    ./headscale.nix
    ./matrix.nix
    ./mattermost.nix
    ./outline.nix
    ./postgres.nix
    ./redis.nix
    ./zigbee2mqtt.nix
  ];
}
