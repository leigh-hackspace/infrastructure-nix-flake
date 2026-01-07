{ flakeInputs }:

{
  imports = [
    # ./affine.nix
    ./backup.nix
    ./cockpit.nix
    ((import ./door-entry-management-system.nix) { inherit flakeInputs; })
    ./frigate.nix
    ./gatus.nix
    ./gitlab.nix
    ((import ./gocardless-authentik-sync.nix) { inherit flakeInputs; })
    ./headscale.nix
    ./librespeed.nix
    # ./matrix.nix
    ./mattermost.nix
    ./mqtt.nix
    ./outline.nix
    ./postgres.nix
    ./redis.nix
    # ./samba.nix
    ./unifi.nix
    ./zigbee2mqtt.nix
  ];
}
