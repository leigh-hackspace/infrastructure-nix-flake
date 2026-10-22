# PARKED — 2026-10-15 (0784799) — docs/authentik-container-migration.md owns this one,
# end to end: what was migrated, what the checklist is, and what blocks the cutover
# (docs/follow-ups.md). 10.3.1.36 is the live IdP; this is the shadow.
#
# Re-verify with: uncomment it in ./default.nix, `just check`, then
# `sudo authentik-migrate all --force` — the state under /srv/authentik is stale.
# authentik — the network's identity provider (OIDC, plus the LDAP and RADIUS
# outposts), as a podman stack on this box.
#
# PARKED since 2026-10-15: this module is not imported (see the note in
# ./default.nix), so nothing here is deployed.  It was built, migrated from the
# appliance and verified end to end — `docs/authentik-container-migration.md`
# records what was checked — and then switched off pending the cutover session.
# Everything below is written for the state it was verified in.
#
# This is a *shadow* install, deliberately run alongside the appliance at
# 10.3.1.36 rather than in its place (the same trick `unifi-os-server.nix`
# used): the stack here is fed a copy of the live database, is served on its
# own LAN-only hostname, and runs the half of authentik that cannot touch
# anything shared.  Nothing in this file talks to 10.3.1.36 — the copy is made
# by `authentik-migrate` (see ./authentik-migrate.sh), and the whole procedure,
# the cutover checklist and the reasons for every pin are in
# docs/authentik-container-migration.md.  Rollback is `nixos-rebuild switch`
# without this file imported.
#
# What runs, mirroring the appliance's compose file:
#
#   authentik-postgres  postgres:16-alpine   the IdP database (its own cluster)
#   authentik-redis     redis:7.2-alpine     cache + celery broker
#   authentik-server    server               the web app (port 9000, loopback)
#   authentik-worker    worker               scheduled tasks, email, outposts
#
# Deliberately *not* the host PostgreSQL: the appliance is on 16 and services1
# is on 17, and keeping the same major version means the migration is a
# same-version restore (so it can be dumped straight back onto the appliance if
# the cutover is reversed) and cannot hit the `template1` collation-version
# mismatch that a cross-major restore invites (docs/work-log-2026-10.md).  It
# also keeps authentik's connection pool off the shared cluster that outline,
# gocardless-dashboard, switch-gui and the door system live on.
#
# --- shadow mode -----------------------------------------------------------
#
# `shadow = true` is the state this box is in now.  It changes three things,
# all of them about *not* being the real IdP twice:
#
#   * the vhost is `id-shadow.int.leighhack.org` (LAN ACL) instead of
#     `id.int.leighhack.org` + its aliases, so no client can reach it by
#     accident;
#   * the worker is not started at boot — it is the half that runs authentik's
#     scheduler, sends mail and drives outposts, so a second one would be a
#     second copy of every scheduled job against a copy of the real data;
#   * the nightly database dump is not on a timer (a copy of a copy in the
#     backups share would only ever confuse a restore).
#
# Flipping it to `false` is the cutover: it moves the vhost onto the real
# names and turns the worker and the backup timer on.  Read the cutover
# checklist in docs/authentik-container-migration.md first — the RADIUS and
# LDAP outposts and the DNS records are not solved by that flag.
#
# --- pins ------------------------------------------------------------------
#
# Every tag is the tag the appliance runs today, verified by image ID: the
# server image here is `2c9dd0d4db11…`, byte-identical to the container
# `authentik-server-1` runs.  A database restored into a *different* authentik
# version is migrated on first boot, which is fine going forward and impossible
# to undo, so both halves of the pin move together with the appliance.
#
# The two `latest`-style tags the appliance uses (`postgres:16-alpine`,
# `redis:alpine`) are left as-is for postgres — 16-alpine is the appliance's
# own tag and the dump is from that same major — and pinned to 7.2 for redis,
# because `redis:alpine` on the appliance resolves to 7.2.2 and a redis major
# bump would be an untested change in the middle of a migration.
{
  config,
  lib,
  pkgs,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  mkIntVhost = import ../../../common/nginx/int-vhost.nix {inherit lib;};

  shadow = true;

  authentikImage = "ghcr.io/goauthentik/server:2026.8.1";
  postgresImage = "docker.io/library/postgres:16-alpine";
  redisImage = "docker.io/library/redis:7.2-alpine";

  stateDir = "/srv/authentik";

  # The one secret file.  Passed to podman with `--env-file`, which — unlike
  # systemd's EnvironmentFile — does **not** strip quotes and does **not**
  # expand `$VAR`: `FOO="bar"` arrives as `"bar"` including the quotes (tested
  # against this box's podman 5.8.8).  So `secrets.yaml`'s `authentik_env_file`
  # is unquoted `KEY=value` lines, which is *not* the style of the shared
  # `env_file` secret; do not copy values between the two by hand.
  envFile = CONFIG.AUTHENTIK_ENV_FILE;

  # The containers address each other by name over the default podman network
  # (`dns_enabled` is on for the box in common/containers.nix).  Neither the
  # database nor redis publishes a host port, so they are reachable from the
  # authentik containers only.
  pgContainer = "authentik-postgres";
  redisContainer = "authentik-redis";
  serverContainer = "authentik-server";

  # Non-secret authentik configuration.  Everything secret lives in `envFile`;
  # the two sets are deliberately disjoint rather than relying on podman's
  # `--env` / `--env-file` precedence.
  akEnv = {
    AUTHENTIK_POSTGRESQL__HOST = pgContainer;
    AUTHENTIK_POSTGRESQL__USER = "authentik";
    AUTHENTIK_POSTGRESQL__NAME = "authentik";
    AUTHENTIK_REDIS__HOST = redisContainer;
    AUTHENTIK_LOG_LEVEL = "info";
    # The IdP does not phone home, and the update check would report this
    # shadow instance to goauthentik as if it were the deployment.
    AUTHENTIK_ERROR_REPORTING__ENABLE = "false";
    AUTHENTIK_DISABLE_UPDATE_CHECK = "true";
  };

  # The image runs as uid 1000 (its own default; the appliance relies on it)
  # and needs to write /media, so the bind mounts are owned by that uid.  1000
  # is unassigned on this box — leigh-admin is 1234 — and is given a name here
  # so nothing else can claim it.
  akUid = 1000;

  # The outposts are off until their tokens are in sops (see the note on the
  # definitions below): a sops key that does not exist fails activation, and
  # nothing can be tested here while the real outposts serve the network.
  enableOutposts = false;

  # The appliance's worker runs as root and mounts /var/run/docker.sock so
  # authentik can create its own outpost containers.  Neither happens here:
  # podman's docker-compatible API is not what authentik's docker controller
  # was written against, and handing a container the socket that controls every
  # other container on the box is a bigger grant than the feature is worth.
  # The outposts are declared below instead.
  outpostEnv = {
    AUTHENTIK_INSECURE = "true";
    # Reachable on the podman network by container name; the appliance used
    # `10.3.1.36:9000` for the same purpose.
    AUTHENTIK_HOST = "${serverContainer}:9000";
    AUTHENTIK_HOST_BROWSER = "${serverContainer}:9000";
  };

  dumpDir = "/mnt/backups/services1.int.leighhack.org/authentik";

  # Nightly custom-format dump of the IdP database.  Not `postgresqlBackup`
  # (that only knows the host cluster) and deliberately not `pg_dumpall`: roles
  # are not part of the migration either, because the target's `authentik` role
  # is created by the postgres image's entrypoint.
  dbDump = pkgs.writeShellApplication {
    name = "authentik-db-dump";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.findutils
      pkgs.podman
    ];
    text = ''
      set -euo pipefail
      umask 077

      dir="${dumpDir}"
      file="$dir/authentik-$(date -u +%Y%m%dT%H%M%SZ).dump"

      mkdir -p "$dir"
      # Dump to a temp name and rename: a truncated dump must never look like a
      # completed one.
      podman exec ${pgContainer} pg_dump -U authentik --format=custom --compress=6 authentik >"$file.part"
      mv "$file.part" "$file"

      # Two weeks of nightly dumps; the appliance's own snapshots are separate.
      find "$dir" -maxdepth 1 -name 'authentik-*.dump' -mtime +14 -delete
    '';
  };
in {
  users.users.authentik = {
    uid = akUid;
    group = "users";
    isSystemUser = true;
    description = "authentik (container)";
  };

  # The state directories, mirroring the appliance's layout so `authentik-migrate
  # files` is a straight rsync of the same three trees.  `database` and `redis`
  # are left root-owned: both images start as root and drop to their own user,
  # and their entrypoints chown the mount themselves.
  system.activationScripts.authentik = ''
    mkdir -p ${stateDir}/database
    mkdir -p ${stateDir}/redis
    mkdir -p ${stateDir}/media
    mkdir -p ${stateDir}/custom-templates
    mkdir -p ${stateDir}/certs
    mkdir -p ${stateDir}/migration
    chmod 700 ${stateDir}/database ${stateDir}/migration
    # The server image runs as uid 1000 and writes uploads under /media.
    chown ${toString akUid}:users ${stateDir}/media ${stateDir}/custom-templates ${stateDir}/certs
    chmod 750 ${stateDir}/media ${stateDir}/custom-templates ${stateDir}/certs
  '';

  # --- the stack -----------------------------------------------------------

  virtualisation.oci-containers.containers =
    {
      ${pgContainer} = {
        hostname = pgContainer;
        image = postgresImage;
        autoStart = true;
        volumes = ["${stateDir}/database:/var/lib/postgresql/data"];
        # No published port: only the authentik containers on the podman network
        # can reach it.
        environment = {
          POSTGRES_USER = "authentik";
          POSTGRES_DB = "authentik";
        };
        environmentFiles = [envFile]; # POSTGRES_PASSWORD
      };

      ${redisContainer} = {
        hostname = redisContainer;
        image = redisImage;
        autoStart = true;
        volumes = ["${stateDir}/redis:/data"];
        # The appliance's own redis command line.
        cmd = [
          "redis-server"
          "--save"
          "60"
          "1"
          "--loglevel"
          "warning"
        ];
      };

      ${serverContainer} = {
        hostname = serverContainer;
        image = authentikImage;
        cmd = ["server"];
        autoStart = true;
        dependsOn = [
          pgContainer
          redisContainer
        ];
        volumes = [
          "${stateDir}/media:/media"
          "${stateDir}/custom-templates:/templates"
          "${stateDir}/certs:/certs"
        ];
        # Loopback only: nginx is the only client, exactly as with the appliance,
        # and a second way in to the IdP that skips the TLS terminator is not one
        # anyone should have to remember to firewall.
        ports = [
          "127.0.0.1:9000:9000"
          "127.0.0.1:9443:9443"
        ];
        environment = akEnv;
        environmentFiles = [envFile]; # AUTHENTIK_SECRET_KEY, AUTHENTIK_POSTGRESQL__PASSWORD
      };

      authentik-worker = {
        hostname = "authentik-worker";
        image = authentikImage;
        cmd = ["worker"];
        # Shadow: see the header.  The unit still exists, so
        # `systemctl start podman-authentik-worker` proves the worker works
        # without making it a boot-time second scheduler.
        autoStart = !shadow;
        dependsOn = [
          pgContainer
          redisContainer
          serverContainer
        ];
        volumes = [
          "${stateDir}/media:/media"
          "${stateDir}/custom-templates:/templates"
          "${stateDir}/certs:/certs"
        ];
        environment = akEnv;
        environmentFiles = [envFile];
      };
    }
    // lib.optionalAttrs enableOutposts {
      # The appliance runs two outposts that authentik created for it: `Radius
      # Outpost` (the ProCurve/AP logins, docs/procurve-switches.md) and `LDAP
      # Outpost` (aibox's sssd, machines/aibox/sso.nix).  Both are rows in the
      # database, so they migrate with it; what does not migrate is the
      # *container*, which authentik created over the docker socket.
      #
      # These two definitions are the replacement: the same images, the same port
      # mapping the appliance has, and the outpost's own credential —
      # `AUTHENTIK_TOKEN`, the same variable the appliance's containers get —
      # from a sops secret.  Each outpost authenticates with an API token named
      # `ak-outpost-<outpost uuid>-api`, and `authentik-migrate outpost-tokens`
      # reads the two of them out of the migrated database into a root-only file
      # for exactly this.
      #
      # They are off until that secret exists, because a missing sops key fails
      # activation, and untested because nothing here can be tested while the
      # real outposts serve the network: two containers registering the same
      # outpost token against two different authentik instances is a split brain
      # on the switch logins.  Read the cutover checklist in
      # docs/authentik-container-migration.md before turning this on.
      authentik-outpost-radius = {
        hostname = "authentik-outpost-radius";
        image = "ghcr.io/goauthentik/radius:2026.8.1";
        autoStart = true;
        dependsOn = [serverContainer];
        ports = ["1812:1812/udp"];
        environment = outpostEnv;
        environmentFiles = [CONFIG.AUTHENTIK_RADIUS_OUTPOST_ENV_FILE];
      };

      authentik-outpost-ldap = {
        hostname = "authentik-outpost-ldap";
        image = "ghcr.io/goauthentik/ldap:2026.8.1";
        autoStart = true;
        dependsOn = [serverContainer];
        # The outpost binds the unprivileged 3389/6636 inside the container; the
        # host publishes the real LDAP ports, exactly as on the appliance.
        ports = [
          "389:3389"
          "636:6636"
        ];
        environment = outpostEnv;
        environmentFiles = [CONFIG.AUTHENTIK_LDAP_OUTPOST_ENV_FILE];
      };
    };

  sops.secrets = lib.mkIf enableOutposts {
    # Each is a one-line `AUTHENTIK_TOKEN=<outpost API token>` file; the
    # token is per outpost and lives in the database (`authentik-migrate
    # outpost-tokens` extracts them into a root-only file — never print one).
    authentik_radius_outpost_env = {
      group = "secrets";
      mode = "0440";
    };
    authentik_ldap_outpost_env = {
      group = "secrets";
      mode = "0440";
    };
  };

  # --- vhost ---------------------------------------------------------------
  #
  # One name at a time: whichever box answers `id.int.leighhack.org` is the IdP
  # every client on the network uses, so the two halves below are mutually
  # exclusive by construction rather than by remembering to delete one.
  services.nginx.virtualHosts =
    {
      # The real names, taken from http.nix at cutover: `id.int.leighhack.org`
      # with `id.leighhack.org` and `authentik.int.leighhack.org` as aliases,
      # which is what the ACME cert, the DNS records and every client already
      # use.  Not LAN-ACL'd, because `id.leighhack.org` is a public name — the
      # current vhost in http.nix is `acl = ""` for the same reason.
      "id.int.leighhack.org" = lib.mkIf (!shadow) (mkIntVhost {
        serverAliases = [
          "id.leighhack.org"
          "authentik.int.leighhack.org"
        ];
        proxyPass = "http://127.0.0.1:9000";
        websockets = true;
        proxyBuffering = false;
        acl = "";
      });
    }
    // lib.optionalAttrs shadow {
      # The shadow instance's own name.  LAN-only (the default ACL), and
      # `common/int-dns.nix` names it, so the router answers it as soon as the
      # router is deployed — and this module is parked, so nothing asks yet.  authentik's tenant base_url is https://id.int.leighhack.org
      # and comes across with the database, so absolute URLs authentik
      # generates still name the real host; the UI and the OIDC endpoints work
      # over this name because everything else is relative.
      "id-shadow.int.leighhack.org" = mkIntVhost {
        proxyPass = "http://127.0.0.1:9000";
        websockets = true;
        proxyBuffering = false;
      };
    };

  # --- backup --------------------------------------------------------------

  systemd.services.authentik-db-dump = {
    description = "Nightly authentik database dump";
    after = [
      "mnt-backups.mount"
      "podman-${pgContainer}.service"
    ];
    requires = ["mnt-backups.mount"];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${dbDump}/bin/authentik-db-dump";
    };
  };

  systemd.timers.authentik-db-dump = lib.mkIf (!shadow) {
    description = "Nightly authentik database dump";
    wantedBy = ["timers.target"];
    timerConfig = {
      Unit = "authentik-db-dump.service";
      OnCalendar = "*-*-* 01:45:00";
      Persistent = true;
    };
  };

  # --- operator tooling ----------------------------------------------------

  environment.systemPackages = [
    (pkgs.writeShellApplication {
      name = "authentik-migrate";
      runtimeInputs = with pkgs; [
        coreutils
        gnugrep
        openssh
        podman
        rsync
        util-linux
      ];
      text = builtins.readFile ./authentik-migrate.sh;
    })
  ];
}
