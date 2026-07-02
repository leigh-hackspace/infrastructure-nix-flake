{ lib, ... }:

let
  CONFIG = import ./config.nix;
  mkSSOVirtualHost = import ./lib/nginx-sso-helper.nix;
in
{
  services.nginx.virtualHosts = {
    "ai.leighhack.org" = lib.mkMerge [
      (mkSSOVirtualHost {
        proxyPass = "http://10.3.1.32:8081";
      })
      {
        locations."/resources" = {
          root = "/srv/ai-resources";
        };
      }
    ];

    "ai.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:8081";
        recommendedProxySettings = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };

      extraConfig = ''
        client_max_body_size        1024M;
        proxy_connect_timeout       3600;
        proxy_send_timeout          3600;
        proxy_read_timeout          3600;
        send_timeout                3600;
      '';
    };

    "mcp.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://10.3.1.32:8000";
        recommendedProxySettings = true;
        proxyWebsockets = true;

        extraConfig = ''
          ${CONFIG.LOCAL_NETWORK}

          add_header 'Access-Control-Allow-Origin' * always;

          if ($request_method = 'OPTIONS') {
            add_header 'Access-Control-Allow-Origin' '*';
            add_header 'Access-Control-Allow-Credentials' 'true';
            add_header 'Access-Control-Allow-Methods' '*';
            add_header 'Access-Control-Allow-Headers' '*';
            add_header 'Access-Control-Max-Age' 86400;
            add_header 'Content-Type' 'text/plain charset=UTF-8';
            add_header 'Content-Length' 0;
            return 204; break;
          }
        '';
      };
    };

    # "sd.ai.leighhack.org" = {
    #   useACMEHost = "leighhack.org";
    #   forceSSL = true;

    #   locations."/" = {
    #     proxyPass = "http://10.3.1.32:7860";
    #     recommendedProxySettings = true;
    #     proxyWebsockets = true;
    #     basicAuthFile = CONFIG.HTTP_BASIC_AUTH_FILE;
    #   };
    # };
  };
}
