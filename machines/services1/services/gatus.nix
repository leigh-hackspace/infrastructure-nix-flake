# Gatus — the endpoint checks and the Slack alerting, on services1.
#
# This used to be the reason services2 exists as more than spare hardware: the
# thing that decides whether a box is down must not run on that box.  When gatus
# lived on services1 it could not report services1 as down — the process answering
# the question was the subject of it — and a services1 failure took the alerting
# with it, including the checks for everything else on the LAN.
#
# That reason stopped holding on 2026-10-17: services2 has 30 GB of RAM and strata's
# Coder engine runs at ~28 GiB RSS against it, so gatus living there is what takes
# the monitor down under memory pressure (machines/services2/README.md, and the
# OOM record in docs/work-log-2026-10.md).  It moved to services1, which has the
# headroom and already serves the status page's vhost.
#
# What is lost is the one check that used to be worth having: gatus now runs on the
# box it watches, so a dead services1 goes down with its own monitor and reports
# nothing, and the "Services1" ping below is a self-check that only catches a hung
# nginx or a dead listener, not a dead box.  What is kept is everything that
# measures the *other* boxes — services2 (whose own status dashboard is the thing
# that survives strata thrashing), aibox, the switches, the printers, the cameras —
# and the Slack alerting, which goes out over the network rather than through
# services1's nginx.
#
# The status page itself is served by this box now (./monitoring.nix, one file up
# at services/monitoring.nix), which is also where the ACME cert and nginx-sso live,
# so the vhost proxies to a loopback bind rather than across the LAN.
#
#   https://gatus.int.leighhack.org   (LAN/tailnet only, like kuma was)
#
# The endpoint list is data in common/monitoring-endpoints.nix; the drift guard
# that goes with it lives in ./monitoring.nix, where the vhosts are.
{
  config,
  lib,
  INFRA,
  ...
}: let
  ENDPOINTS = import ../../../common/monitoring-endpoints.nix {inherit lib;};
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
