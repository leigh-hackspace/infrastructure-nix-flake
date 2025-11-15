{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ./config.nix;
  mkSSOVirtualHost = import ./lib/nginx-sso-helper.nix;
in
{
  services.nginx.virtualHosts = {
    "ai.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://10.3.1.32:8080";
    };

    "7b.ai.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://10.3.1.32:8081";
    };

    # External (with auth)
    "sd.ai.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:7860";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        basicAuthFile = CONFIG.HTTP_BASIC_AUTH_FILE;
      };
    };
    # Internal (no auth)
    "sd.ai.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:7860";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };
  };
}
