{ config, pkgs, ... }:

{
  imports = [
    # ./affine.nix
    ./cockpit.nix
    ./door-entry-management-system.nix
    ./frigate.nix
    ./gocardless-authentik-sync.nix
    ./headscale.nix
    ./librespeed.nix
    # ./matrix.nix
    ./mattermost.nix
    ./mqtt.nix
    ./outline.nix
    ./postgres.nix
    ./redis.nix
    # ./samba.nix
    ./zigbee2mqtt.nix
  ];
}
