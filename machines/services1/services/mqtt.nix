{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  MQTT_UID = 1883;
in
{
  users.users.mqtt = {
    uid = MQTT_UID;
    group = "mqtt";
    isNormalUser = true;
  };

  users.groups.mqtt = {
    gid = MQTT_UID;
  };

  virtualisation.oci-containers.containers.mqtt = {
    hostname = "mqtt";
    image = "docker.io/eclipse-mosquitto";
    autoStart = true;
    ports = [
      "1883:1883"
    ];
    volumes = [
      "/srv/mosquitto:/mosquitto"
    ];
    environment = {
      TZ = "Europe/London";
    };
    extraOptions = [
      "--user=${toString MQTT_UID}:${toString MQTT_UID}"
    ];
  };
}
