{
  config,
  lib,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};

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

    # Public name, gated by the front door (the `status.leighhack.org` row in
    # common/front-door.nix is `sso = true`). The dashboard still needs
    # `X-WEBAUTH-USER` for POST /api/restart, and nginx forwards the header the door
    # sets because this vhost does not replace it.
    "status.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8088";
    };

    # LAN-only name (ACL at the door), restart allowed via the LAN token below.
    "services1-status.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8088";
      extraConfig = lanTokenHeader lanToken;
    };
  };
}
