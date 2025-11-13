{
  config,
  pkgs,
  lib,
  ...
}:

let
  CONFIG = import ../config.nix;
  UPLOAD_LOCATION = "/srv/affine/storage";
  CONFIG_LOCATION = "/srv/affine/config";
  DB_DATABASE = "affine";
  DB_USERNAME = "affine";
  DB_PASSWORD = CONFIG.PG_PASS;
in
{
  users.users.affine = {
    uid = 8300;
    isNormalUser = true;
    description = "Affine User";
  };

  systemd.services.podman-affine-server.wants = [ "podman-affine-migration.service" ];
  systemd.services.podman-affine-server.after = [ "podman-affine-migration.service" ];

  virtualisation.oci-containers.containers.affine-server = {
    hostname = "affine-server";
    image = "ghcr.io/toeverything/affine:stable";
    autoStart = true;
    volumes = [
      "${UPLOAD_LOCATION}:/root/.affine/storage"
      "${CONFIG_LOCATION}:/root/.affine/config"
    ];
    ports = [
      "8300:3010"
    ];
    environment = {
      TZ = "Europe/London";
      REDIS_SERVER_HOST = "10.88.0.1";
      REDIS_SERVER_PORT = "8301";
      DATABASE_URL = "postgresql://${DB_USERNAME}:${DB_PASSWORD}@10.88.0.1:5432/${DB_DATABASE}";
      AFFINE_INDEXER_ENABLED = "false";
    };
    extraOptions = [
      # "--user=8300:100"
    ];
  };

  systemd.services.podman-affine-migration.serviceConfig.Restart = lib.mkForce "on-failure";

  virtualisation.oci-containers.containers.affine-migration = {
    hostname = "affine-migration";
    image = "ghcr.io/toeverything/affine:stable";
    autoStart = true;
    volumes = [
      "${UPLOAD_LOCATION}:/root/.affine/storage"
      "${CONFIG_LOCATION}:/root/.affine/config"
    ];
    environment = {
      TZ = "Europe/London";
      REDIS_SERVER_HOST = "10.88.0.1";
      REDIS_SERVER_PORT = "8301";
      DATABASE_URL = "postgresql://${DB_USERNAME}:${DB_PASSWORD}@10.88.0.1:5432/${DB_DATABASE}";
      AFFINE_INDEXER_ENABLED = "false";
    };
    extraOptions = [
      # "--user=8300:100"
    ];
    cmd = [
      "sh"
      "-c"
      "node ./scripts/self-host-predeploy.js"
    ];
  };

  services.nginx.virtualHosts = {
    "affine.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:8300";
        recommendedProxySettings = true;
        proxyWebsockets = true;
      };
    };
  };
}
