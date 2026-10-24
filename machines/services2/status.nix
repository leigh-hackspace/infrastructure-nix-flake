# services2's own status dashboard (shared module: common/status-dashboard.nix).
#
# Bound on 0.0.0.0 because `gw` — the network's front door — reverse-proxies it,
# exactly as it does for aibox's.  The name is
# `services2-status.int.leighhack.org`, a row in common/front-door.nix.
#
# The dashboard is the one thing on this box that is about *this* box: it probes
# services2's own units and its own NAS mounts.
{config, ...}: let
  # LAN restart token, shared with services1 and aibox via the sops secret
  # status_dashboard_lan_token.  Must match the value services1 injects as
  # X-Status-Token for this dashboard's vhost.
  lanToken = builtins.readFile (config.sopsSecretText "status_dashboard_lan_token");
in {
  imports = [../../common/status-dashboard.nix];

  services.status-dashboard = {
    enable = true;
    title = "services2";
    bind = "0.0.0.0";
    restartToken = lanToken;
  };
}
