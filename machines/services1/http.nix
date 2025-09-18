{ config, lib, pkgs, modulesPath, ... }:

# Outside IPv4 (NATed by pfSense)   81.187.195.17
# Outside/Inside IPv6               2001:8b0:1d14:225:d::1020

# curl -X GET -H "Authorization: Bearer dop_v1_blah" "https://api.digitalocean.com/v2/domains/leighhack.org/records?name=gw.int.leighhack.org"

let
  CONFIG = import ./config.nix;
in
{
  # Use DNS based challenge to acquire SSL certificates. Works even if NGINX is down.
  security.acme = {
    acceptTerms = true;

    defaults = {
      dnsProvider = "digitalocean";
      dnsPropagationCheck = true;
      email = "admin@leighhack.org"; # Your email for Let's Encrypt notifications
      # server = "https://acme-staging-v02.api.letsencrypt.org/directory";
    };

    # Certs stored in /var/lib/acme
    certs = {
      "leighhack.org" = {
        environmentFile = CONFIG.ENV_FILE;
        group = config.services.nginx.group; # Ensure nginx can access the certificates
        extraDomainNames = [
          "*.leighhack.org"
          "*.ai.leighhack.org"
          "*.int.leighhack.org"
          "*.ai.int.leighhack.org"
        ];
      };
    };
  };

  services.nginx = {
    enable = true;

    appendHttpConfig = ''
      server_names_hash_bucket_size 128;
    '';

    virtualHosts = {
      "discourse.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.38:80";
          recommendedProxySettings = true;
        };
      };

      "web-test.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "https://10.3.1.39:443";
          recommendedProxySettings = true;
        };
      };

      "webhooks.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "https://10.3.1.39:443";
          recommendedProxySettings = true;
        };
      };

      "retro.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = false;

        locations."/" = {
          proxyPass = "https://10.3.1.39:443";
          recommendedProxySettings = true;
        };
      };

      "firewall.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "https://10.3.1.1:60443";
          recommendedProxySettings = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "truenas.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "https://10.3.1.6:443";
          recommendedProxySettings = true;
          proxyWebsockets = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "monster.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "https://10.3.1.11:8006";
          recommendedProxySettings = true;
          proxyWebsockets = true;
        };
      };

      "access-api.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.30:8083";
          recommendedProxySettings = true;
        };
      };

      "api.int.leighhack.org" = {
        serverAliases = [ "api.leighhack.org" ];
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.30:8081";
          recommendedProxySettings = true;
        };
      };

      "filestore.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.30:8001";
          recommendedProxySettings = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "grafana.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.30:3000";
          recommendedProxySettings = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "id.int.leighhack.org" = {
        serverAliases = [ "id.leighhack.org" "authentik.int.leighhack.org" ];
        useACMEHost = "leighhack.org";
        forceSSL = true;

        extraConfig = ''
          ssl_session_cache shared:SSL:1m;
          ssl_session_timeout 10m;
          ssl_prefer_server_ciphers on;
        '';

        locations."/" = {
          proxyPass = "http://10.3.1.36:9000";
          recommendedProxySettings = true;
          proxyWebsockets = true;

          extraConfig = ''
            proxy_buffering off;
          '';
        };
      };

      "jenkins.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.30:8082";
          recommendedProxySettings = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "uptime-kuma.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.30:3001";
          recommendedProxySettings = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "user-tweaker.int.leighhack.org" = {
        serverAliases = [ "user-tweaker.leighhack.org" ];
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.30:8084";
          recommendedProxySettings = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "ha.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        addSSL = true;

        extraConfig = ''
          # Increase buffer size for large headers
          # This is needed only if you get 'upstream sent too big header while reading response
          # header from upstream' error when trying to access an application protected by goauthentik
          proxy_buffers 8 16k;
          proxy_buffer_size 32k;
        '';

        locations."/" = {
          proxyPass = "http://10.3.1.30:8123";

          extraConfig = ''
            # authentik-specific config
            auth_request        /outpost.goauthentik.io/auth/nginx;
            error_page          401 = @goauthentik_proxy_signin;
            auth_request_set $auth_cookie $upstream_http_set_cookie;
            add_header Set-Cookie $auth_cookie;

            proxy_set_header X-Forwarded-Proto $scheme;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header Host $host;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $connection_upgrade;
            proxy_http_version 1.1;
            proxy_buffering off;

            # translate headers from the outposts back to the actual upstream
            auth_request_set $authentik_username $upstream_http_x_authentik_username;
            auth_request_set $authentik_groups $upstream_http_x_authentik_groups;
            auth_request_set $authentik_email $upstream_http_x_authentik_email;
            auth_request_set $authentik_name $upstream_http_x_authentik_name;
            auth_request_set $authentik_uid $upstream_http_x_authentik_uid;

            proxy_set_header X-authentik-username $authentik_username;
            proxy_set_header X-authentik-groups $authentik_groups;
            proxy_set_header X-authentik-email $authentik_email;
            proxy_set_header X-authentik-name $authentik_name;
            proxy_set_header X-authentik-uid $authentik_uid;

            ${CONFIG.LOCAL_NETWORK}
          '';
        };

        # all requests to /outpost.goauthentik.io must be accessible without authentication
        locations."/outpost.goauthentik.io" = {
          proxyPass = "http://10.3.1.36:9000/outpost.goauthentik.io";

          extraConfig = ''
            # ensure the host of this vserver matches your external URL you've configured
            # in authentik
            proxy_set_header    Host $host;
            proxy_set_header    X-Original-URL $scheme://$http_host$request_uri;
            add_header          Set-Cookie $auth_cookie;
            auth_request_set    $auth_cookie $upstream_http_set_cookie;
          '';
        };

        # Special location for when the /auth endpoint returns a 401,
        # redirect to the /start URL which initiates SSO
        locations."@goauthentik_proxy_signin" = {
          extraConfig = ''
            internal;
            add_header Set-Cookie $auth_cookie;
            return 302 /outpost.goauthentik.io/start?rd=$request_uri;
            # For domain level, use the below error_page to redirect to your authentik server with the full redirect path
            # return 302 https://authentik.company/outpost.goauthentik.io/start?rd=$scheme://$http_host$request_uri;
          '';
        };
      };

      "frigate.int.leighhack.org" = {
        # serverAliases = [ "frigate.leighhack.org" ];
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "http://10.3.1.20:5000";
          recommendedProxySettings = true;
          proxyWebsockets = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "unifi-admin.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "https://10.3.1.40:8443";
          recommendedProxySettings = true;
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      "robot.ai.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          root = "/srv/ai-resources/resources/robot";
        };
      };
    };
  };
}
