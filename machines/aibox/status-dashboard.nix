# aibox runs the shared status dashboard (common/status-dashboard.nix).
#
# aibox has no nginx of its own: it binds on 0.0.0.0 and services1 is the
# SSL-terminating reverse proxy for it (aibox.status.leighhack.org /
# aibox.status.int.leighhack.org, see machines/services1/services/status.nix).
{ ... }:

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
    # Shared with the *.int vhosts on services1 (see
    # CONFIG.STATUS_DASHBOARD_LAN_TOKEN in
    # machines/services1/config.nix) so restart works on the LAN without
    # SSO sign-in.
    restartToken = "11d01e910d40fc204701df70a7e0db0436630a0d4e2a2872";
  };
}
