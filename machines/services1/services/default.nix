{
  imports = [
    # PARKED — deliberately not imported, so these modules are never evaluated
    # (they can rot silently; `nix flake check` does not cover them).  Re-enable
    # one by uncommenting and then check it evaluates before deploying.
    #   ./affine.nix  — the Affine container was never deployed on this box.
    #   ./samba.nix   — SMB is served by the NAS (10.3.1.6), not here.
    # ./affine.nix
    # PARKED 2026-10-15: the authentik *shadow* — a podman copy of the 10.3.1.36
    # appliance, built and verified against a live copy of its database, then
    # switched off until the session that does the real cutover.  10.3.1.36 is
    # the IdP either way; see docs/authentik-container-migration.md for the
    # checklist and docs/follow-ups.md for what blocks it.  Uncommenting brings
    # back postgres/redis/server and the id-shadow.int vhost; the state under
    # /srv/authentik outlives the parking (bind mounts, not volumes), but the
    # copy is stale, so re-run `authentik-migrate all --force` before trusting it.
    # ./authentik.nix
    ./backup.nix
    ./cockpit.nix
    ./door-entry-management-system.nix
    ./filestore.nix
    ./frigate-monitor.nix
    ./frigate.nix
    # Gatus — the endpoint checks and the Slack alerting.  Moved here from
    # machines/services2/gatus.nix on 2026-10-17; the endpoint list is data in
    # common/monitoring-endpoints.nix and the vhost is ./monitoring.nix.
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
    # The observability vhosts + the gatus drift guard.  The apps themselves are
    # ./gatus.nix and ./observability.nix — since 2026-10-17 both the apps and the
    # vhosts are on this box (services2 keeps only strata), so the vhosts proxy to
    # a loopback bind.  The dashboards are ./monitoring-dashboards.nix, pulled in
    # by ./observability.nix rather than from this list.
    ./monitoring.nix
    ./mqtt.nix
    # Prometheus + Grafana, moved off services2 on 2026-10-17 (see ./observability.nix
    # for why: services2's 30 GB is strata's).
    ./observability.nix
    ./outline.nix
    ./postgres.nix
    ./printer-monitoring.nix
    ./redis.nix
    # ./samba.nix  (PARKED, see the note at the top of this list)
    ./status.nix
    # The vhost for services2's Strata engine (see machines/services2/strata.nix);
    # the name must stay in that file's allowed_hosts too.
    ./strata-proxy.nix
    # Ports and VLANs on the ProCurve core switch (10.3.1.2) over HTTPS.
    # Needs the switch_gui_env_file sops secret before it will start.
    ./switch-gui.nix
    # Trial of the new appliance, on offset ports, next to ./unifi.nix rather
    # than in its place.  Remove this line to take it back out.
    ./unifi-os-server.nix
    ./unifi.nix
    ./zigbee2mqtt.nix
  ];
}
