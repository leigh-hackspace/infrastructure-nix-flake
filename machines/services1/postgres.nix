{ config, lib, pkgs, modulesPath, ... }:

let
  CONFIG = import ./config.nix;
  # Define a single data structure for DB names and passwords.
  dbs = {
    door_system = CONFIG.PG_PASS;
    door_system_dev = CONFIG.PG_PASS;
  };
in
{
  services.postgresql = {
    enable = true;
    package = pkgs.postgresql_17;
    enableTCPIP = true;

    # Define a single data structure for DB names and passwords.
    ensureDatabases = lib.attrNames dbs;
    ensureUsers = lib.mapAttrsToList (db: _: {
      name = db;
      ensureDBOwnership = true;
    }) dbs;

    settings = {
      ssl = true;
    };

    authentication = pkgs.lib.mkOverride 10 CONFIG.PG_AUTH;
  };

  systemd.services.postgres-set-password =
    let
      generateSql = db: password: ''
        DO
        $do$
        BEGIN
          IF EXISTS (
            SELECT FROM pg_catalog.pg_roles WHERE rolname = '${db}'
          ) THEN
            ALTER USER ${db} WITH PASSWORD '${password}';
            RAISE NOTICE 'Set password for user "${db}".';
          END IF;
        END
        $do$;
      '';
      fullSql = builtins.concatStringsSep "\n" (lib.mapAttrsToList generateSql dbs);
    in
    {
      script = ''
        ${pkgs.postgresql_17}/bin/psql -U postgres <<'EOF'
          ${fullSql}
        EOF
      '';
      after = [ "postgresql.service" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
      };
    };
}

## Open a PSQL shell to the database
# sudo -u postgres psql door_system

## Dump schema for the "user" table
# sudo -u postgres pg_dump -st user door_system

## First time setup of PostgreSQL certificate
# cd ~postgres
# sudo -u postgres openssl req -new -x509 -days 3650 -nodes -text -out server.crt -keyout server.key -subj "/CN=services1.int.leighhack.org"
# chmod og-rwx server.key
# systemctl restart postgresql
# journalctl -u postgresql.service
