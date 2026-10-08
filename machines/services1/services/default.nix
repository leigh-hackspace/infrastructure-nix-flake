{
  imports = [
    # ./affine.nix
    ./backup.nix
    ./cockpit.nix
    ./door-entry-management-system.nix
    ./filestore.nix
    ./frigate-monitor.nix
    ./frigate.nix
    ./gatus.nix
    # Disabled: the GitLab container is not deployed.  Its config is still
    # rendered by lib/config-to-gitlab.nix, which lib/check-config-to-gitlab.nix
    # keeps tested against the attrset in ./gitlab.nix — re-enable this line and
    # the check together.
    # ./gitlab.nix
    ./gocardless-dashboard.nix
    ./headscale.nix
    ./librespeed.nix
    ./matrix.nix
    ./mattermost.nix
    ./monitoring.nix
    ./mqtt.nix
    ./outline.nix
    ./postgres.nix
    ./printer-monitoring.nix
    ./redis.nix
    # ./samba.nix
    ./status.nix
    ./unifi.nix
    ./zigbee2mqtt.nix
  ];
}
