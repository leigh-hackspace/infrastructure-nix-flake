{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

{
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
  };
}
