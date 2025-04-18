{ config, lib, pkgs, modulesPath, ... }:

let
  CONFIG = import ./config.nix;
in
{
  services.nginx.virtualHosts = {
    # External (with auth)
    "ai.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:8080";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        basicAuthFile = CONFIG.HTTP_BASIC_AUTH_FILE;
      };

      locations."/resources" = {
        root = "/srv/ai-resources";
      };
    };
    # Internal (no auth)
    "ai.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:8080";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };

      locations."/resources" = {
        root = "/srv/ai-resources";
      };
    };

    # External (with auth)
    "ai-7b.leighhack.org" = {
      serverAliases = [ "7b.ai.leighhack.org" ];
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:8081";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        basicAuthFile = CONFIG.HTTP_BASIC_AUTH_FILE;
      };
    };
    # Internal (no auth)
    "ai-7b.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:8081";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };
  };
}
