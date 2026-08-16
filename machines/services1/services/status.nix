{
  config,
  lib,
  pkgs,
  ...
}:

let
  CONFIG = import ../config.nix;
  mkSSOVirtualHost = import ../lib/nginx-sso-helper.nix;

  statusDashboard = pkgs.rustPlatform.buildRustPackage {
    pname = "status-dashboard";
    version = "0.1.0";
    src = ./status-dashboard;
    cargoLock.lockFile = ./status-dashboard/Cargo.lock;
    doCheck = false;
  };
in
{
  # Simple web dashboard showing the health of the NAS, its mounts and all
  # container services (good / bad / waiting for deps), with a one-click
  # restart for fault finding.
  systemd.services.status-dashboard = {
    description = "Systemd status dashboard";
    wantedBy = [ "multi-user.target" ];
    after = [ "network.target" ];
    # Tools the dashboard shells out to (findmnt / ping / uptime / hostname).
    path = [
      pkgs.util-linux
      pkgs.iputils
      pkgs.procps
    ];
    serviceConfig = {
      Type = "simple";
      ExecStart = "${statusDashboard}/bin/status-dashboard --bind 127.0.0.1 --port 8088";
      Restart = "always";
      RestartSec = "5s";
    };
    startLimitIntervalSec = 0;
  };

  services.nginx.virtualHosts = {
    # Public, SSO-protected: restart buttons work here (nginx injects
    # X-WEBAUTH-USER which the dashboard requires for POST /api/restart).
    "status.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://127.0.0.1:8088";
    };

    # LAN-only, read-only (no SSO headers, so restarts are refused).
    "status.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://127.0.0.1:8088";
        recommendedProxySettings = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };
  };
}
