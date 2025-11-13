{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

# Outside IPv4 (NATed by pfSense)   81.187.195.17
# Outside/Inside IPv6               2001:8b0:1d14:225:d::1020

# curl -X GET -H "Authorization: Bearer dop_v1_blah" "https://api.digitalocean.com/v2/domains/leighhack.org/records?name=gw.int.leighhack.org"

let
  CONFIG = import ./config.nix;
in
{
  # Necessary for secret access
  users.groups.secrets.members = [ "nginx" ];

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

    # recommendedProxySettings = true;  # Breaks Home Assistant
    recommendedTlsSettings = true;
    recommendedOptimisation = true;
    recommendedGzipSettings = true;

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

      "webhooks.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          proxyPass = "https://10.3.1.39:443";
          recommendedProxySettings = true;
          extraConfig = ''
            proxy_connect_timeout       300;
            proxy_send_timeout          300;
            proxy_read_timeout          300;
            send_timeout                300;
          '';
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
        serverAliases = [
          "id.leighhack.org"
          "authentik.int.leighhack.org"
        ];
        useACMEHost = "leighhack.org";
        forceSSL = true;

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
          proxyWebsockets = true;
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

        locations."/" = {
          proxyPass = "http://10.3.1.30:8123";
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

      "retro.int.leighhack.org" = {
        forceSSL = false;
        addSSL = false;

        locations."/" = {
          root = "/sas-16tb/ds-downloads/Retro";
          extraConfig = ''
            autoindex on;
          '';
        };
      };
    };
  };
}
