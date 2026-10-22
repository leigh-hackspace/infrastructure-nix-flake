{
  config,
  lib,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  mkSSOVirtualHost = import ../../../common/nginx/sso-vhost.nix;
  mkIntVhost = import ../../../common/nginx/int-vhost.nix {inherit lib;};

  HOSTS = import ../../../common/hosts.nix;

  # LAN restart token, shared with aibox via the sops secret
  # status_dashboard_lan_token.
  lanToken = builtins.readFile (config.sopsSecretText "status_dashboard_lan_token");

  # Injected into the LAN-only *.int vhosts so the dashboard's restart
  # endpoint works without SSO sign-in (the vhosts are already
  # ACL-restricted to the local network). Must match
  # services.status-dashboard.restartToken on this machine and on aibox.
  lanTokenHeader = token: ''
    proxy_set_header X-Status-Token ${token};
  '';
in {
  # Runs the shared dashboard (common/status-dashboard.nix) locally on
  # 127.0.0.1:8088, probing services1's NAS mounts.
  imports = [../../../common/status-dashboard.nix];

  services.status-dashboard = {
    enable = true;
    title = "services1";
    restartToken = lanToken;
  };

  services.nginx.virtualHosts = {
    # --- services1's own dashboard --------------------------------------

    # Public, SSO-protected: restart buttons work here (nginx injects
    # X-WEBAUTH-USER which the dashboard requires for POST /api/restart).
    "status.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://127.0.0.1:8088";
    };

    # LAN-only (ACL-restricted), restart allowed via the LAN token below.
    "services1-status.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://127.0.0.1:8088";
      extraConfig = lanTokenHeader lanToken;
    };

    # --- aibox's dashboard (proxied from 10.3.1.32) ---------------------
    # services1 remains the SSL terminator for aibox too; X-WEBAUTH-USER is
    # injected by nginx here and forwarded to the aibox dashboard, so the
    # restart buttons work over the SSO vhost.

    "aibox.status.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://${HOSTS.aibox}:8088";
    };

    "aibox-status.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://${HOSTS.aibox}:8088";
      extraConfig = lanTokenHeader lanToken;
    };

    # --- services2's dashboard (proxied from 10.3.1.21) -------------------
    # LAN-only.  There is no public vhost for services2: the public
    # *.leighhack.org names need an explicit record in the DigitalOcean zone
    # `*.int` names are answered by the router from common/int-dns.nix, but a
    # public name is a record in the DigitalOcean zone and services2 has no public IP, so
    # a public name would point at this box anyway.  Restart buttons work over the
    # LAN vhost via the token header.
    "services2-status.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://${HOSTS.services2}:8088";
      extraConfig = lanTokenHeader lanToken;
    };
  };
}
