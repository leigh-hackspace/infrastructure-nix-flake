{ config, lib, pkgs, ... }:

let
  CONFIG = import ../config.nix;
in
{
  # sudo journalctl -u door-entry-management-system-backend -f
  systemd.services.door-entry-management-system-backend = {
    description = "Door Entry Management System Backend";
    requires = [ "network.target" "postgresql.service" ];

    # Ensure the service is started at boot
    wantedBy = [ "multi-user.target" ];

    environment = {
      DE_MODE = "production";
      DE_BACKEND_PORT = "8472";
      DE_FRONTEND_PORT = "8473";
      DE_AUTHENTIK_HOST = "${CONFIG.AUTHENTIK_DOMAIN}";
      DE_AUTHENTIK_CLIENT_ID = "clzfOASFMcArxJ2t4zl9fLRAHypykiHQNcHLHcYK";
      DE_HOME_ASSISTANT_WS_URL = "wss://ha.int.leighhack.org/api/websocket";
      DE_SLACK_CHANNEL = "doors";
    };

    # Executable from your flake, running with systemd
    serviceConfig = {
      ExecStart = "${pkgs.door-entry-management-system}/bin/door-entry-management-system-backend";
      Restart = "always";
      RestartSec = 5;
      EnvironmentFile = CONFIG.ENV_FILE;
    };
  };

  # sudo journalctl -u door-entry-management-system-frontend -f
  systemd.services.door-entry-management-system-frontend = {
    description = "Door Entry Management System Frontend";
    requires = [ "door-entry-management-system-backend.service" ];

    # Ensure the service is started at boot
    wantedBy = [ "multi-user.target" ];

    # Executable from your flake, running with systemd
    serviceConfig = {
      ExecStart = "${pkgs.door-entry-management-system}/bin/door-entry-management-system-frontend";
      Restart = "always";
      RestartSec = 5;
      WorkingDirectory = "${pkgs.door-entry-management-system}/lib/frontend";
      EnvironmentFile = pkgs.writeText "door-entry-management-system-frontend-env" ''
        DE_FRONTEND_PORT=8473
      '';
    };
  };

  # # sudo journalctl -u door-entry-bluetooth-web-app -f
  # systemd.services.door-entry-bluetooth-web-app = {
  #   description = "Door Entry Management System Frontend";
  #   requires = [ "network.target" ];

  #   # Ensure the service is started at boot
  #   wantedBy = [ "multi-user.target" ];

  #   # Executable from your flake, running with systemd
  #   serviceConfig = {
  #     ExecStart = "${pkgs.door-entry-bluetooth-web-app}/bin/door-entry-bluetooth-web-app";
  #     Restart = "always";
  #     RestartSec = 5;
  #     WorkingDirectory = "${pkgs.door-entry-bluetooth-web-app}/lib/frontend";
  #     EnvironmentFile = pkgs.writeText "door-entry-bluetooth-web-app-env" ''
  #       BLE_FRONTEND_PORT=8474
  #     '';
  #   };
  # };

  services.nginx.virtualHosts = {
    "api-doors.int.leighhack.org" = {
      serverAliases = [ "api-doors.leighhack.org" ];
      useACMEHost = "leighhack.org";
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:8472";
        recommendedProxySettings = true;
        # extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };

    "doors.int.leighhack.org" = {
      serverAliases = [ "doors.leighhack.org" ];
      useACMEHost = "leighhack.org";
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:8473";
        recommendedProxySettings = true;
        # extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };

    # "app.int.leighhack.org" = {
    #   serverAliases = [ "app.leighhack.org" ];
    #   useACMEHost = "leighhack.org";
    #   forceSSL = true;
    #   locations."/" = {
    #     proxyPass = "http://127.0.0.1:8474";
    #     recommendedProxySettings = true;
    #     # extraConfig = CONFIG.LOCAL_NETWORK;
    #   };
    # };
  };
}
