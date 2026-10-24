{
  config,
  lib,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  ZIGBEE2MQTT_UID = 8124;
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};
in {
  users.users.zigbee2mqtt = {
    uid = ZIGBEE2MQTT_UID;
    group = "users";
    extraGroups = ["dialout"];
    isNormalUser = true;
  };

  virtualisation.oci-containers.containers.zigbee2mqtt = {
    hostname = "zigbee2mqtt";
    image = "koenkk/zigbee2mqtt:latest";
    autoStart = true;
    ports = [
      "8282:8080"
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
      "--user=${toString ZIGBEE2MQTT_UID}:100"
      "--group-add=${toString config.users.groups.dialout.gid}"
    ];
  };

  services.nginx.virtualHosts = {
    "zigbee2mqtt.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8282";
      websockets = true;
    };

    "zigbee2mqtt.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8282";
      websockets = true;
    };
  };

  system.activationScripts.zigbee2mqtt = ''
    # Create config directory
    mkdir -p /srv/zigbee2mqtt

    # Ensure correct permissions
    chown -R ${toString ZIGBEE2MQTT_UID}:users /srv/zigbee2mqtt
    chmod -R g+rw /srv/zigbee2mqtt
  '';
}
