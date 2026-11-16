# aibox runs the shared status dashboard (common/status-dashboard.nix).
#
# aibox has no nginx of its own: it binds on 0.0.0.0 and the front door on `gw`
# proxies to it (`aibox-status.int.leighhack.org`, a direct row in
# common/front-door.nix). It used to have a public twin,
# `aibox.status.leighhack.org`; that row was deleted on 2026-11-16 because the name
# was never added to the DigitalOcean zone, so the wildcard certificate could not
# have matched it even if it resolved (`docs/router-front-door-migration.md`).
{config, ...}: {
  imports = [../../common/status-dashboard.nix];

  services.status-dashboard = {
    enable = true;
    title = "aibox";
    mounts = [
      "/mnt/filestore"
      "/mnt/ds-photos"
    ];
    bind = "0.0.0.0";
    # Shared with the *.int vhosts on services1 (machines/services1/
    # services/status.nix) via the sops secret status_dashboard_lan_token,
    # so restart works on the LAN without SSO sign-in.
    restartToken = builtins.readFile (config.sopsSecretText "status_dashboard_lan_token");
  };
}
