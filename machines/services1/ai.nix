{lib, ...}: let
  CONFIG = import ./config.nix;
  mkSSOVirtualHost = import ./lib/nginx-sso-helper.nix;
  mkIntVhost = import ./lib/nginx-int-vhost-helper.nix {inherit lib;};
in {
  services.nginx.virtualHosts = {
    "ai.leighhack.org" = lib.mkMerge [
      (mkSSOVirtualHost {
        proxyPass = "http://10.3.1.32:8081";
      })
      {
        locations."/resources" = {
          root = "/srv/ai-resources";
        };

        # Browsers fetch the PWA manifest without cookies, so SSO would 401 it
        # and bounce it to the login page (CORS error in the console). Serve it
        # unauthenticated, like the fake /sw.js above.
        locations."= /manifest.webmanifest" = {
          proxyPass = "http://10.3.1.32:8081";
          recommendedProxySettings = true;
        };
      }
    ];

    # Long timeouts and a big body limit: the chat UI uploads images and can
    # hold a request open while the model thinks.
    "ai.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://10.3.1.32:8081";
      bodySize = "1024M";
      timeouts = "3600";
    };

    "mcp.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://10.3.1.32:8000";
      websockets = true;
      extraConfig = ''
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

    # Whisper.cpp WebSocket gateway (whisper-ws on aibox:8083, OpenAI
    # Realtime protocol). LAN-only; DNS alias added on the router's dnsmasq.
    # Long timeouts: WebSocket sessions can sit idle for a while.
    "whisper.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://10.3.1.32:8083";
      websockets = true;
      timeouts = "3600";
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
