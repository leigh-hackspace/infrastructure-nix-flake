{ config, utils, ... }:
let
  IMMICH_UID = 2283;
  # Write compiled config file here...
  configFile = "/run/immich.json";
  # Config file which includes SOPS secrets...
  secretsReplacement = (
    utils.genJqSecretsReplacement { } {
      newVersionCheck.enable = false;
      oauth = {
        autoLaunch = true;
        autoRegister = true;
        buttonText = "Login with OAuth";
        clientId = "oxetb1oGz99wrGkQYUE96bGteFmz6HKfaYidk7ps";
        clientSecret._secret = "${config.sops.secrets.immich_oidc_client_secret.path}";
        defaultStorageQuota = null;
        enabled = true;
        issuerUrl = "https://id.leighhack.org/application/o/immich/";
        mobileOverrideEnabled = false;
        profileSigningAlgorithm = "none";
        roleClaim = "immich_role";
        scope = "openid email profile immich_role";
        signingAlgorithm = "RS256";
        storageLabelClaim = "preferred_username";
        timeout = 30000;
        tokenEndpointAuthMethod = "client_secret_post";
      };
      machineLearning = {
        urls = [ "http://10.88.0.51:3003" ];
      };
    } configFile
  );
in
{
  # journalctl -u immich-secrets -f
  systemd.services.immich-secrets = {
    description = "Immich Secrets";
    requiredBy = [ "podman-immich-server.service" ];
    before = [ "podman-immich-server.service" ];
    script = secretsReplacement.script;
  };

  users.users.immich = {
    uid = IMMICH_UID;
    group = "users";
    isNormalUser = true;
  };

  virtualisation.oci-containers.containers = {
    # sudo podman exec -ti immich-server immich-admin grant-admin
    # journalctl -u podman-immich-server -f
    immich-server = {
      hostname = "immich-server";
      image = "ghcr.io/immich-app/immich-server:v3.0.3";
      autoStart = true;
      ports = [
        "2283:2283"
      ];
      volumes = [
        "${configFile}:/config.json" # Config file. Contains OIDC settings
        "/mnt/ds-photos/immich/upload:/data" # Native data
        "/etc/localtime:/etc/localtime:ro"
      ];
      environment = {
        IMMICH_CONFIG_FILE = "/config.json"; # NOTE! Path within the container
        DB_HOSTNAME = "10.88.0.53";
        DB_USERNAME = "postgres";
        DB_PASSWORD = "postgres";
        DB_DATABASE_NAME = "immich";
        REDIS_HOSTNAME = "10.88.0.52";
        # IMMICH_LOG_LEVEL = "verbose";
      };
      extraOptions = [
        "--ip=10.88.0.50"
        "--user=${toString IMMICH_UID}:100"
      ];
      dependsOn = [
        "immich-redis"
        "immich-postgres"
      ];
    };

    # journalctl -u podman-immich-machine-learning -f
    immich-machine-learning = {
      hostname = "immich-machine-learning";
      image = "ghcr.io/immich-app/immich-machine-learning:v3.0.3";
      autoStart = true;
      volumes = [
        "/mnt/ds-photos/immich/model-cache:/cache"
      ];
      environment = {
        DB_HOSTNAME = "10.88.0.53";
        DB_USERNAME = "postgres";
        DB_PASSWORD = "postgres";
        DB_DATABASE_NAME = "immich";
        REDIS_HOSTNAME = "10.88.0.52";
      };
      extraOptions = [
        "--ip=10.88.0.51"
        # "--user=${toString IMMICH_UID}:100" # Can't run rootless (yet)
      ];
    };

    immich-redis = {
      hostname = "immich-redis";
      image = "docker.io/valkey/valkey:9@sha256:4963247afc4cd33c7d3b2d2816b9f7f8eeebab148d29056c2ca4d7cbc966f2d9";
      autoStart = true;
      extraOptions = [
        "--ip=10.88.0.52"
        "--user=${toString IMMICH_UID}:100"
      ];
    };

    immich-postgres = {
      hostname = "immich-postgres";
      image = "ghcr.io/immich-app/postgres:16-vectorchord0.5.3-pgvector0.8.1@sha256:971d18060781e929dc3a0b72b02e3f09ba9d146d4c00b2acac81a7ae837bbde5";
      autoStart = true;
      volumes = [
        "/mnt/ds-photos/immich/postgres:/var/lib/postgresql/data"
      ];
      environment = {
        POSTGRES_PASSWORD = "postgres";
        POSTGRES_USER = "postgres";
        POSTGRES_DB = "immich";
        POSTGRES_INITDB_ARGS = "--data-checksums";
      };
      extraOptions = [
        "--ip=10.88.0.53"
        "--shm-size=128mb"
        "--user=${toString IMMICH_UID}:100"
      ];
    };
  };

  # wait-for-network only pings the NAS; it does not wait for the NFS export
  # to be genuinely mounted (TrueNAS can ping before its exports are ready).
  # These containers bind data to /mnt/ds-photos, so without wait-for-nas they
  # start against a not-yet-mounted share and crash — and, without the
  # never-give-up restart policy (P0#1), get permanently stopped.  wait-for-nas
  # (machines/aibox/nfs-client.nix) only succeeds once the share is real.
  #
  # NOTE: PostgreSQL here is the immich OCI container (NAS ordering, P0#3).
  # The host PostgreSQL pg_hba `trust` rule (P0#4) is intentionally left
  # untouched per the request.
  systemd.services.podman-immich-server = {
    requires = [ "wait-for-nas.service" ];
    after = [ "wait-for-nas.service" ];
  };

  systemd.services.podman-immich-machine-learning = {
    requires = [ "wait-for-nas.service" ];
    after = [ "wait-for-nas.service" ];
  };

  systemd.services.podman-immich-postgres = {
    requires = [ "wait-for-nas.service" ];
    after = [ "wait-for-nas.service" ];
  };
}
