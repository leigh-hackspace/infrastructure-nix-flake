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
  users.groups.secrets.members = [ "mattermost" ];

  environment.systemPackages = with pkgs; [
    mattermost
  ];

  services.mattermost = {
    enable = true;
    siteUrl = "https://mattermost.leighhack.org"; # Set this to the URL you will be hosting the site on.
    database.peerAuth = true;
    settings = {
      "GitLabSettings" = {
        "Enable" = true;
        "Secret" = builtins.readFile CONFIG.MATTERMOST_AUTHENTIK_SECRET_FILE;
        "Id" = "CEqUaJOX3VDU2j4HJhnBWY0SHYkvbx7AIoIVSdZJ";
        "Scope" = "";
        "AuthEndpoint" = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/authorize/";
        "TokenEndpoint" = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/token/";
        "UserAPIEndpoint" = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/userinfo/";
        "DiscoveryEndpoint" =
          "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/mattermost/.well-known/openid-configuration";
        "ButtonText" = "Log in with Leigh Hackspace <- CLICK HERE";
        "ButtonColor" = "#000000";
      };
    };
  };

  services.nginx.virtualHosts = {
    # Replace with the domain from your siteUrl
    "mattermost.leighhack.org" = {
      forceSSL = true; # Enforce SSL for the site
      useACMEHost = "leighhack.org";

      locations."/" = {
        proxyPass = "http://127.0.0.1:8065"; # Route to Mattermost
        recommendedProxySettings = true;
        proxyWebsockets = true;
      };
    };
  };
}
