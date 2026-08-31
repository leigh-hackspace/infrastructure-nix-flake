{
  lib,
  config,
  ...
}:

{
  services.gatus = {
    enable = true;
    settings = {
      web.port = 8999;
      alerting.slack.webhook-url = lib.strings.trim (builtins.readFile (config.sopsSecretText "slack_url"));
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

  services.nginx.virtualHosts = {
    "gatus.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://localhost:8999";
        recommendedProxySettings = true;
      };
    };
  };
}
