{
  config,
  lib,
  pkgs,
  modulesPath,
  INFRA,
  ...
}:
# Outside IPv4 (NATed by pfSense)   81.187.195.17
# Outside/Inside IPv6               2001:8b0:1d14:225:d::1020
# curl -X GET -H "Authorization: Bearer dop_v1_blah" "https://api.digitalocean.com/v2/domains/leighhack.org/records?name=gw.int.leighhack.org"
let
  CONFIG = import ../../common/config.nix;
  nginxSsoConfig = import ./lib/nginx-sso-config.nix;
  mkSSOVirtualHost = import ../../common/nginx/sso-vhost.nix;
  mkIntVhost = import ../../common/nginx/int-vhost.nix {inherit lib;};
in {
  # Necessary for secret access
  users.groups.secrets.members = ["nginx"];

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
          "*.test.leighhack.org"
        ];
      };
    };
  };

  # journalctl -u nginx-sso -f
  systemd.services.nginx-sso = INFRA.mkNeverGiveUp (
    let
      configFilePath = (
        pkgs.writeText "nginx-sso-config" (nginxSsoConfig {
          inherit lib;
          sopsSecretText = config.sopsSecretText;
        })
      );
    in {
      description = "NGINX SSO";

      # Ensure the service is started at boot
      wantedBy = ["multi-user.target"];

      serviceConfig = {
        ExecStart = "${pkgs.nginx-sso}/bin/nginx-sso --frontend-dir=${pkgs.nginx-sso}/share/frontend -c ${configFilePath}";
      };
    }
  );

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
      # The DNS canary (`bar.int`).  It serves nothing but this page: if it loads
      # by name, the router's generated record, dnsmasq, this vhost and the ACME
      # cert all work, which is the shortest end-to-end test of the front door
      # there is (common/int-dns.nix is the record, machines/gw/dns.nix serves it).
      "bar.int.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = true;

        locations."/" = {
          root = "${pkgs.writeTextDir "bar/index.html" ''
            <!doctype html>
            <html lang="en">
              <head>
                <meta charset="utf-8">
                <title>bar.int.leighhack.org</title>
              </head>
              <body>
                <h1>bar.int.leighhack.org</h1>
                <p>This page is served by services1 nginx and resolves via the
                   router dnsmasq and DigitalOcean DNS (the DNS canary).</p>
              </body>
            </html>
          ''}/bar";
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      # Alexandria audiobook generator on aibox (machines/aibox/alexandria.nix):
      # AI book -> voiced audiobook WebUI (FastAPI on 4200). LAN/tailscale only;
      # the int record lives in common/int-dns.nix.
      "alexandria.int.leighhack.org" = mkIntVhost {
        proxyPass = "http://10.3.1.32:4200";
        websockets = true;
        # Book uploads + long audio-generation requests
        bodySize = "1024M";
        timeouts = "3600s";
        proxyBuffering = false;
      };

      # Router network dashboard (machines/gw/network-status.nix):
      # realtime per-interface bandwidth, conntrack entries and issues for
      # gw (10.3.1.1), where the service now runs.  LAN-only; the record lives in
      # common/int-dns.nix.
      "network-info.int.leighhack.org" = mkIntVhost {
        proxyPass = "http://10.3.1.1:8091";
      };

      "login.int.leighhack.org" = mkIntVhost {
        serverAliases = ["login.leighhack.org"];
        proxyPass = "http://127.0.0.1:8082";
        # The SSO cookie nginx-sso sets on a successful login exceeds
        # nginx's default 4k proxy header buffer, which makes the /login
        # callback fail with "upstream sent too big header" (502).
        proxyHeaderBuffers = true;
        acl = "";
      };

      "webhooks.leighhack.org" = mkIntVhost {
        proxyPass = "https://10.3.1.39:443";
        timeouts = "300";
        acl = "";
      };

      "web-test.leighhack.org" = mkIntVhost {
        proxyPass = "https://10.3.1.39:443";
        acl = "";
      };

      "retro.leighhack.org" = {
        useACMEHost = "leighhack.org";
        forceSSL = false;

        locations."/" = {
          proxyPass = "https://10.3.1.39:443";
          recommendedProxySettings = true;
        };
      };

      "firewall.int.leighhack.org" = mkIntVhost {
        proxyPass = "https://10.3.1.1:60443";
      };

      "truenas.int.leighhack.org" = mkIntVhost {
        proxyPass = "https://10.3.1.6:443";
        websockets = true;
      };

      # Proxmox web UI. Not LAN-ACL'd (deliberately: reachable from the tailnet
      # ranges in CONFIG.LOCAL_NETWORK anyway) — kept as it was, `acl = ""`.
      "monster.int.leighhack.org" = mkIntVhost {
        proxyPass = "https://10.3.1.11:8006";
        websockets = true;
        acl = "";
      };

      "access-api.int.leighhack.org" = mkIntVhost {
        proxyPass = "http://10.3.1.30:8083";
        acl = "";
      };

      "api.int.leighhack.org" = mkIntVhost {
        serverAliases = ["api.leighhack.org"];
        proxyPass = "http://10.3.1.30:8081";
        acl = "";
      };

      # filestore.int.leighhack.org moved to services/filestore.nix (the
      # Rust filestore binary on this box, 127.0.0.1:8096 — replacing the
      # old apps1:8001 deployment).

      # The authentik appliance (10.3.1.36) is the live IdP.  This box has a
      # shadow of it written and verified but PARKED (services/authentik.nix is
      # not imported, so there is no id-shadow.int vhost right now); the cutover
      # deletes this vhost in the same change that un-parks the module and sets
      # `shadow = false` — two vhosts for one name is the safe way to fail that.
      # Read docs/authentik-container-migration.md first.
      "id.int.leighhack.org" = mkIntVhost {
        serverAliases = [
          "id.leighhack.org"
          "authentik.int.leighhack.org"
        ];
        proxyPass = "http://10.3.1.36:9000";
        websockets = true;
        proxyBuffering = false;
        acl = "";
      };

      "jenkins.int.leighhack.org" = mkIntVhost {
        proxyPass = "http://10.3.1.30:8082";
      };

      "user-tweaker.int.leighhack.org" = mkIntVhost {
        serverAliases = ["user-tweaker.leighhack.org"];
        proxyPass = "http://10.3.1.30:8084";
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

        # Authentik support
        locations."/auth/authorize" = {
          proxyPass = "http://10.3.1.30:8123";
          recommendedProxySettings = true;
          proxyWebsockets = true;
          extraConfig = ''
            ${CONFIG.LOCAL_NETWORK}
            sub_filter_once off;
            sub_filter_types text/html;
            sub_filter '<head>' '<head><script>setTimeout(function(){window.location.href="/auth/oidc/redirect";},1000);</script>';
            proxy_set_header Accept-Encoding "";
          '';
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

      "voron.leighhack.org" = mkSSOVirtualHost {
        proxyPass = "http://10.3.2.50";
      };

      "immich.leighhack.org" = mkIntVhost {
        proxyPass = "http://10.3.1.32:2283";
        websockets = true;
        bodySize = "50000M";
        timeouts = "600s";
        acl = "";
      };
    };
  };
}
