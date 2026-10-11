# Gatus — the endpoint checks and the Slack alerting.
#
# This is the reason services2 exists as more than spare hardware: the thing that
# decides whether a box is down must not run on that box.  When gatus lived on
# services1 it could not report services1 as down — the process answering the
# question was the subject of it — and a services1 failure took the alerting with
# it, including the checks for everything else on the LAN.
#
# Here it is independent: it pings 10.3.1.20, and it alerts to Slack over the
# network rather than through services1's nginx.  The status page itself is still
# served by services1 (which terminates TLS for the whole network), so the vhost
# stays in machines/services1/services/monitoring.nix and proxies to this box.
#
#   https://gatus.int.leighhack.org   (LAN/tailnet only, like kuma was)
#
# The endpoint list is data in common/monitoring-endpoints.nix; the drift guard
# that goes with it lives on services1, where the vhosts are.
{
  config,
  lib,
  INFRA,
  ...
}: let
  ENDPOINTS = import ../../common/monitoring-endpoints.nix {inherit lib;};
in {
  services.gatus = {
    enable = true;
    settings = {
      web.port = 8999;
      alerting.slack.webhook-url = lib.strings.trim (
        builtins.readFile (config.sopsSecretText "slack_url")
      );

      endpoints = ENDPOINTS.list;
    };
  };

  # The house "never give up" policy (common/systemd.nix). gatus's own module sets
  # Restart=on-failure with systemd's default start rate limit, which is the wrong
  # shape for the thing that is supposed to notice a box being down: a crash loop
  # would leave it permanently stopped and silent.
  systemd.services.gatus = INFRA.mkNeverGiveUpOverride;
}
