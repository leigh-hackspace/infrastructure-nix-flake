{ ... }:

let
  CONFIG = import ../config.nix;
  mkSSOVirtualHost = import ../lib/nginx-sso-helper.nix;

  AIBOX_IP = "10.3.1.32";

  # Injected into the LAN-only *.int vhosts so the dashboard's restart
  # endpoint works without SSO sign-in (the vhosts are already
  # ACL-restricted to the local network). Must match
  # services.status-dashboard.restartToken on this machine and on aibox.
  lanTokenHeader =
    token:
    ''
      proxy_set_header X-Status-Token ${token};
    '';
in
{
  # Runs the shared dashboard (common/status-dashboard.nix) locally on
  # 127.0.0.1:8088, probing services1's NAS mounts.
  imports = [ ../../../common/status-dashboard.nix ];

  services.status-dashboard = {
    enable = true;
    title = "services1";
    restartToken = CONFIG.STATUS_DASHBOARD_LAN_TOKEN;
  };

  services.nginx.virtualHosts = {
    # --- services1's own dashboard --------------------------------------

    # Public, SSO-protected: restart buttons work here (nginx injects
    # X-WEBAUTH-USER which the dashboard requires for POST /api/restart).
    "status.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://127.0.0.1:8088";
    };

    # LAN-only (ACL-restricted), restart allowed via the LAN token below.
    "services1-status.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://127.0.0.1:8088";
        recommendedProxySettings = true;
        extraConfig =
          CONFIG.LOCAL_NETWORK
          + lanTokenHeader CONFIG.STATUS_DASHBOARD_LAN_TOKEN;
      };
    };

    # --- aibox's dashboard (proxied from 10.3.1.32) ---------------------
    # services1 remains the SSL terminator for aibox too; X-WEBAUTH-USER is
    # injected by nginx here and forwarded to the aibox dashboard, so the
    # restart buttons work over the SSO vhost.

    "aibox.status.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://${AIBOX_IP}:8088";
    };

    "aibox-status.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://${AIBOX_IP}:8088";
        recommendedProxySettings = true;
        extraConfig =
          CONFIG.LOCAL_NETWORK
          + lanTokenHeader CONFIG.STATUS_DASHBOARD_LAN_TOKEN;
      };
    };
  };
}
