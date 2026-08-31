# aibox runs the shared status dashboard (common/status-dashboard.nix).
#
# aibox has no nginx of its own: it binds on 0.0.0.0 and services1 is the
# SSL-terminating reverse proxy for it (aibox.status.leighhack.org /
# aibox-status.int.leighhack.org, see machines/services1/services/status.nix).
{ config, ... }:

{
  imports = [ ../../common/status-dashboard.nix ];

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
