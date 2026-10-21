# Make sure hackspace LAN is advertised
# sudo tailscale set --advertise-routes=10.3.0.0/16,2001:8b0:1d14::0/48,fd99:dead:beef:0225::/64
# sudo tailscale set --advertise-routes=10.3.0.0/16,fd99:dead:beef:0225::/64
{
  pkgs,
  lib,
  config,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
in {
  # Necessary for secret access
  users.groups.secrets.members = ["headscale"];

  services.headscale = {
    enable = true;
    address = "0.0.0.0";
    port = 8085;

    settings = {
      server_url = "https://${CONFIG.HEADSCALE_DOMAIN}";

      log.level = "debug";

      dns = {
        override_local_dns = false;
        base_domain = "ts.leighhack.org";
        magic_dns = true;
        search_domains = ["int.leighhack.org"];
        nameservers = {
          global = [
            "9.9.9.9"
            "8.8.8.8"
            "1.1.1.1"
          ];
          split = {
            "int.leighhack.org" = ["10.3.1.1"];
          };
        };
      };

      # The tailnet's allocation pool, read from the one copy of the fact
      # (common/config.nix) — this is the string that decides what address a node
      # gets, and the router's forward rules, every `*.int` vhost's nginx ACL and
      # `pg_hba` all trust the same block.  They used to be a /16 and this a /10,
      # which is a node that joins, works over IPv4-by-DERP and is then refused by
      # everything on the LAN side.
      ip_prefixes = [
        CONFIG.TAILNET_V4
        CONFIG.TAILNET_V6
      ];

      derp = {
        server = {
          enabled = true;
          stun_listen_addr = "0.0.0.0:3479"; # Unify uses default STUN port of 3478
          ipv4 = "81.187.195.17";
          ipv6 = "2001:8b0:1d14:225:d::1020";
        };
      };

      oidc = {
        enable = true;
        issuer = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/headplane/";
        client_id = "uvJnUrNJuaUXw3K8cJhT9fX7IMQ9amcBY5vTYJTQ";
        client_secret_path = CONFIG.HEADPLANE_CLIENT_SECRET_FILE;
        allowed_groups = ["Members"];
      };
    };
  };

  services.headplane = let
    format = pkgs.formats.yaml {};

    # A workaround generate a valid Headscale config accepted by Headplane when `config_strict == true`.
    settings = lib.recursiveUpdate config.services.headscale.settings {
      tls_cert_path = "/dev/null";
      tls_key_path = "/dev/null";
      policy.path = "/dev/null";
    };

    headscaleConfig = format.generate "headscale.yml" settings;
  in {
    enable = true;
    settings = {
      server = {
        host = "127.0.0.1";
        port = 8086;
        cookie_secret_path = pkgs.writeText "cookie_secret_path" "12345678123456781234567812345678";
        base_url = "https://${CONFIG.HEADSCALE_DOMAIN}/admin";
      };
      headscale = {
        url = "https://${CONFIG.HEADSCALE_DOMAIN}";
        config_path = "${headscaleConfig}";
        # Was `oidc.headscale_api_key_path` until nixpkgs 26.05: the key is no longer
        # OIDC-specific (the agent needs it too), so the module moved it under
        # `headscale` and asserts it is set whenever `oidc` or the agent is.
        api_key_path = CONFIG.HEADPLANE_API_KEY_FILE;
      };
      # `pre_authkey_path` is gone in the same rewrite: the agent now persists its own
      # tailnet identity under `work_dir` (/var/lib/headplane/agent) and
      # reauthenticates, so it needs no pre-auth key.
      integration.agent = {
        enabled = true;
      };
      oidc = {
        issuer = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/headplane/";
        client_id = "uvJnUrNJuaUXw3K8cJhT9fX7IMQ9amcBY5vTYJTQ";
        client_secret_path = CONFIG.HEADPLANE_CLIENT_SECRET_FILE;
        # Only support login through Authentik (go straight to login)
        disable_api_key_login = true;

        # Might needed when integrating with Authentik.
        # token_endpoint_auth_method = "client_secret_basic";
        token_endpoint_auth_method = "client_secret_post";
        # redirect_uri = "https://${CONFIG.HEADSCALE_DOMAIN}/admin/oidc/callback";
      };
    };
  };

  services.nginx.virtualHosts = {
    "${CONFIG.HEADSCALE_DOMAIN}" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:8085";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        # Redirect to the admin (the root URL is just 404 normally)
        extraConfig = ''
          rewrite ^/$ https://${CONFIG.HEADSCALE_DOMAIN}/admin permanent;
        '';
      };
      locations."/admin" = {
        proxyPass = "http://127.0.0.1:8086";
        recommendedProxySettings = true;
        proxyWebsockets = true;
      };
    };

    # Not needed anymore. Admin rolled into Tailscale endpoint above
    "headplane.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:8086";
        recommendedProxySettings = true;
        proxyWebsockets = true;
      };
    };
  };

  services.tailscale = {
    enable = true;
    useRoutingFeatures = "server";
  };
}
