{
  imports = [
    # PARKED — deliberately not imported, so these modules are never evaluated
    # (they can rot silently; `nix flake check` does not cover them).  Re-enable
    # one by uncommenting and then check it evaluates before deploying.
    #   ./affine.nix  — the Affine container was never deployed on this box.
    #   ./samba.nix   — SMB is served by the NAS (10.3.1.6), not here.
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
    # ./samba.nix  (PARKED, see the note at the top of this list)
    ./status.nix
    ./unifi.nix
    ./zigbee2mqtt.nix
  ];
}
