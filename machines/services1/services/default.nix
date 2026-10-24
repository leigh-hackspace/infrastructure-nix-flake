{
  imports = [
    # PARKED modules — the ones commented out below — now say so on **their own first
    # line**, in the shape AGENTS.md rule 23 fixes:
    #
    #   # PARKED — <date> (<commit>) — <the doc that owns it> — re-verify with: <cmd>
    #
    # so `grep -rl '^# PARKED' machines/` *is* the list of parked things (it was not:
    # the reason lived in a comment beside the import, which is how a parked module
    # becomes a secret).  The lines below name the file and nothing more, because a
    # parked module is never evaluated — `nix flake check` does not cover it, `just
    # check` does not cover it, and the import list is not where its state of repair
    # should be recorded.  Uncomment, then run `just check`, and expect to fix it.
    #
    # ./affine.nix
    # ./authentik.nix
    ./backup.nix
    ./cockpit.nix
    ./door-entry-management-system.nix
    ./filestore.nix
    ./frigate.nix
    # Gatus — the endpoint checks and the Slack alerting.  Moved here from
    # machines/services2/gatus.nix on 2026-10-17; the endpoint list is data in
    # common/monitoring-endpoints.nix and the vhost is ./monitoring.nix.
    ./gatus.nix
    # ./gitlab.nix
    ./gocardless-dashboard.nix
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
    # ./samba.nix
    ./status.nix
    # The vhost for services2's Strata engine moved to the front door's table
    # (`common/front-door.nix`'s `strata.int`/`strata.leighhack.org` rows) — the door
    # proxies directly to 10.3.1.21:8080 and injects the engine key on the LAN name.
    # ./strata-proxy.nix is deleted with it.
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
