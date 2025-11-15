{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  CONFIG = import ../config.nix;
  mkSSOVirtualHost = import ../lib/nginx-sso-helper.nix;
in
{
  virtualisation.oci-containers.containers.zigbee2mqtt = {
    hostname = "zigbee2mqtt";
    image = "koenkk/zigbee2mqtt:latest-dev";
    autoStart = true;
    ports = [
      "8080:8080"
    ];
    volumes = [
      "/srv/zigbee2mqtt:/app/data"
      "/run/udev:/run/udev:ro"
    ];
    environment = {
      TZ = "Europe/London";
    };
    extraOptions = [
      "--device=/dev/ttyUSB0"
    ];
  };

  services.nginx.virtualHosts = {
    "zigbee2mqtt.leighhack.org" = mkSSOVirtualHost {
      proxyPass = "http://127.0.0.1:8080";
    };

    "zigbee2mqtt.int.leighhack.org" = {
      forceSSL = true;
      useACMEHost = "leighhack.org";

      locations."/" = {
        proxyPass = "http://127.0.0.1:8080";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };
  };
}
