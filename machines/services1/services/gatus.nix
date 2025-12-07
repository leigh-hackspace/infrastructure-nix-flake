{
  lib,
  ...
}:

let
  CONFIG = import ../config.nix;
in
{
  services.gatus = {
    enable = true;
    settings = {
      web.port = 8999;
      alerting.slack.webhook-url = lib.strings.trim (builtins.readFile CONFIG.SLACK_URL_FILE);
      endpoints = [
        {
          name = "Uptime Kuma";
          url = "https://uptime-kuma.int.leighhack.org/dashboard";
          interval = "60s";
          conditions = [
            "[STATUS] == 200"
            "[RESPONSE_TIME] < 300"
          ];
          alerts = [
            {
              type = "slack";
              description = "healthcheck failed 3 times in a row";
              send-on-resolved = true;
            }
          ];
          # Send on resolved
        }
      ];
    };
  };
}
