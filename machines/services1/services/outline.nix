{
  pkgs,
  lib,
  config,
  ...
}:

let
  CONFIG = import ../config.nix;
in
{
  # Necessary for secret access
  users.groups.secrets.members = [ "outline" ];

  services.outline = {
    enable = true;
    port = 8400;
    publicUrl = "https://outline.leighhack.org";
    forceHttps = true;
    storage.storageType = "local";
    databaseUrl = "postgresql://outline:${CONFIG.PG_PASS}@127.0.0.1:5432/outline";
    redisUrl = "redis://127.0.0.1:8401";
    oidcAuthentication = {
      authUrl = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/authorize/";
      tokenUrl = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/token/";
      userinfoUrl = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/userinfo/";
      clientId = "pP0YLbCtnHYNJmMs6MUTeQ1WEFOMX4vA36eTHCFQ";
      clientSecretFile = CONFIG.OUTLINE_CLIENT_SECRET_FILE;
      scopes = [
        "openid"
        "email"
        "profile"
      ];
      usernameClaim = "preferred_username";
      displayName = "Leigh Hackspace";
    };
  };

  services.nginx.virtualHosts = {
    "outline.leighhack.org" = {
      forceSSL = true;
      useACMEHost = "leighhack.org";

      locations."/" = {
        proxyPass = "http://127.0.0.1:8400";
        recommendedProxySettings = true;
        proxyWebsockets = true;
      };
    };
  };
}
