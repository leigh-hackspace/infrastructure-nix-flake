# authentik — the network's identity provider (OIDC, plus the LDAP and RADIUS
# outposts), as a podman stack on this box.
#
# **This box has been the IdP since 2026-10-28.**  The stack was first deployed in
# 2026-10-15 as a *shadow* — a copy of the appliance's database running beside it on a
# LAN-only hostname, the `unifi-os-server.nix` trick — and was cut over on 2026-10-28,
# which is the interesting part of the story: the appliance at **10.3.1.36** was retired
# by moving *that address* onto this box as a second address on `vlan225`
# (./networking.nix) rather than by reconfiguring anything that dials it.  Two groups of
# clients name the address or the name it answers, not this box, and neither could be
# changed:
#
#   * the ProCurve switches' `radius-server host 10.3.1.36` — switch NVRAM is not
#     written unless asked (AGENTS.md rule 16), and `docs/procurve-switches.md` is the
#     reason nobody trusts that config to a redeploy;
#   * every LDAP client — aibox's sssd (`machines/aibox/sso.nix`) and the pi-room
#     netboot image — dials `ldaps://authentik.int.leighhack.org`, and that name is
#     answered from `common/dns-records.nix`'s `authentik` record, i.e. with `.36`.
#
# So the address came here and the clients never moved.  `10.3.1.36` is a *second*
# address on this box, deliberately not the primary: everything else this box does
# still sources from `10.3.1.20`, and podman's published ports are netavark DNAT, which
# un-NATs the reply, so a RADIUS request sent to `.36` is answered *as* `.36` — which is
# the whole trick, and the reason the outpost ports below are published on `0.0.0.0`
# rather than loopback.  Read that comment before "tidying" them.
#
# The procedure, the receipts of what was verified and what is still open are in
# `docs/authentik-container-migration.md`; the reasons for every pin below are the same
# doc.  The copy of the live database is made by `authentik-migrate` (see
# ./authentik-migrate.sh), which is read-only against the source.
#
# What runs, mirroring the appliance's compose file:
#
#   authentik-postgres  postgres:16-alpine   the IdP database (its own cluster)
#   authentik-redis     redis:7.2-alpine     cache + celery broker
#   authentik-server    server               the web app (port 9000, loopback)
#   authentik-worker    worker               scheduled tasks, email, outposts
#
# Deliberately *not* the host PostgreSQL: the appliance was on postgres 16 and
# services1 is on 17, and keeping the same major version meant the migration was a
# same-version restore (dumpable straight back onto the appliance if the cutover were
# reversed) and could not hit the `template1` collation-version mismatch a cross-major
# restore invites (docs/work-log-2026-10.md).  That reason expired with the appliance —
# the dump has been restored into 16.15 and cannot go back to a 16.9 box that is now
# stopped — but the container stays, because the other half of the reason is permanent:
# it keeps authentik's connection pool off the shared cluster that outline,
# gocardless-dashboard, switch-gui and the door system live on.
#
# --- staging mode ----------------------------------------------------------
#
# `staged` is the deploy-time gate the cutover was done behind.  It is `false` now, so
# this box *is* the IdP; set it back to `true` only deliberately, because it withdraws
# four things at once — the vhost, the two outposts, the worker and the nightly dump
# timer.  What it does **not** withdraw is `10.3.1.36`, which lives in ./networking.nix:
# re-staging this box while it still holds the switches' RADIUS address means RADIUS
# answers stop arriving with no unit to stop.
#
# `staged = true` runs the stack *without claiming the network*, which is what let a
# copy of the live database be taken, restored and checked on 2026-10-28 while the real
# IdP was still answering.  The four things it holds back:
#
#   * no vhost at all.  `machines/services1/int-dns.nix` asserts that this box's vhosts
#     are exactly the `upstream = "origin"` rows of `common/front-door.nix`, so a vhost
#     for `id.int` cannot exist here before the front-door row says so — the evaluation
#     refuses, which is the shape AGENTS.md rule 24 wants.  (The 2026-10-15 staging run
#     had its own `id-shadow.int` name and vhost; that name is gone from both tables and
#     verifying a staged copy needs no name: `curl -H 'Host: id.int.leighhack.org'
#     http://127.0.0.1:9000/…` reaches the app with no DNS and no door involved, which is
#     how the 2026-10-28 staging was checked.)
#   * the worker is not started at boot — it is the half that runs authentik's
#     scheduler, sends mail and drives outposts, so a second one would be a
#     second copy of every scheduled job against a copy of the real data;
#   * the outposts are not run at all: two containers registering the same outpost
#     token against two authentik instances is a split brain on the switch logins;
#   * the nightly database dump is not on a timer (a copy of a copy in the
#     backups share would only ever confuse a restore).
#
# Flipping it to `false` is the cutover.  `10.3.1.36` is the fifth thing that moves and
# it is *not* behind this flag, because interfaces are ./networking.nix's business; the
# two edits have to land in the same commit — an origin that answers RADIUS on `.20`
# only is a network of switches that time out.
#
# --- pins ------------------------------------------------------------------
#
# Every tag is the tag the appliance ran, verified by image ID: the server image here is
# `2c9dd0d4db11…`, byte-identical to the container `authentik-server-1` ran (re-checked
# both sides on 2026-10-28, the day of the cutover — `authentik-migrate check` dies on an
# image-ID mismatch, so this is not a pin that can quietly drift).  A database restored
# into a *different* authentik version is migrated on first boot, which is fine going
# forward and impossible to undo, so upgrades start here and never involve the retired
# appliance: the reason the same-major rule mattered was the rollback path, and that path
# closed when the VM was stopped.
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
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};

  # The deploy-time gate described in the header.  `true` = the stack runs, the network
  # is not claimed; `false` = this box *is* the IdP.  It was `true` for the staging
  # deploy of 2026-10-28 (466ba0d, the un-parking commit) and `false` from the cutover on;
  # read the staging-mode section and docs/authentik-container-migration.md before changing
  # it.
  staged = false;

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

  # The outposts are the network's RADIUS and LDAP servers, so they run only when this
  # box is the IdP (`staged = false`) — and their tokens have to be in sops first, since
  # a sops key that does not exist fails activation.  `authentik-migrate outpost-tokens`
  # writes the two of them out of the migrated database for exactly that step.
  enableOutposts = !staged;

  # The appliance's worker runs as root and mounts /var/run/docker.sock so
  # authentik can create its own outpost containers.  Neither happens here:
  # podman's docker-compatible API is not what authentik's docker controller
  # was written against, and handing a container the socket that controls every
  # other container on the box is a bigger grant than the feature is worth.
  # The outposts are declared below instead.
  outpostEnv = {
    AUTHENTIK_INSECURE = "true";
    # The API an outpost registers itself with and reads its own config from.
    #
    # ⚠️  It has to be a URL **with a scheme**.  The outpost SDK parses this value as a
    # URL, so a bare `authentik-server:9000` arrives as the *scheme* `authentik-server`
    # and the container sits in a retry loop reading
    # `Get "authentik-server://api/v3/outposts/instances/": unsupported protocol scheme "authentik-server"`.
    # That is exactly how the first minutes of the 2026-10-28 cutover went: unit
    # `active`, ports published, nothing bound, no RADIUS answer.  Read
    # `podman logs authentik-outpost-radius`, not `systemctl is-active`.
    #
    # What the appliance really used — read out of the migrated outpost row
    # (`authentik_outposts_outpost._config`) rather than assumed, because the generated
    # compose file that held it is on a VM that no longer answers the address it owns:
    # `authentik_host = "https://id.int.leighhack.org/"`, `authentik_host_browser = ""`,
    # `authentik_host_insecure = false`.  So the switch logins reached authentik's API the
    # long way round — out of the box, through the front door, and back — which made
    # RADIUS depend on the door being up.  Naming the container on the podman network is
    # shorter, and honest about the dependency: an outpost needs authentik, not the
    # internet.
    AUTHENTIK_HOST = "http://${serverContainer}:9000";
    AUTHENTIK_HOST_BROWSER = "http://${serverContainer}:9000";
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
        # Staged: see the header.  The unit still exists, so
        # `systemctl start podman-authentik-worker` proves the worker works
        # without making it a boot-time second scheduler.
        autoStart = !staged;
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
      # The appliance ran two outposts that authentik created for it: `Radius
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
      # They run only when this box is the IdP (`enableOutposts` above), for the
      # reason the header gives: two containers registering the same outpost token
      # against two different authentik instances is a split brain on the switch
      # logins.  Measured on the appliance rather than assumed: **the LDAPS serving
      # certificate is a database row** (`authentik_crypto_certificatekeypair`, name
      # `authentik.int.leighhack.org`, sha256 `6E:B0:69:1B…F1:58`, expires 2035-11-15,
      # and byte-identical to what `.36:636` served on 2026-10-28) while the appliance's
      # `/srv/authentik/certs` was **empty**.  So it migrates with the dump and the
      # outposts need no certificate mount — aibox's sssd runs `ldap_tls_reqcert =
      # demand`, so a regenerated cert would lock the box and the netbooted pi-room
      # clients out, and a future `certs` mount here would not fix that, only move it.
      #
      # ⚠️  THE PORTS BELOW ARE PUBLISHED ON `0.0.0.0` ON PURPOSE.  The clients dial
      # `10.3.1.36`, which since the cutover is this box's *second* address, and a
      # RADIUS client's socket is connected: an Access-Challenge sourced from
      # `10.3.1.20` is dropped by the switch in silence and the symptom is an auth
      # timeout.  What makes the reply come back as `.36` is that netavark publishes
      # with iptables `NETAVARK-HOSTPORT-DNAT` and conntrack therefore un-DNATs the
      # reply to the address that was dialled — true for `0.0.0.0` rules, and the reason
      # not to "harden" these to `127.0.0.1:…` or to re-bind them to `.20`.  Binding
      # them literally to `10.3.1.36:…` would work too and adds a boot-order coupling
      # (the address has to exist before netavark installs the rule) for no extra
      # safety: RADIUS and LDAP are UDP/TCP ports on a box whose firewall is off, and
      # the appliance published them on `0.0.0.0` for a decade, postgres included.
      authentik-outpost-radius = {
        hostname = "authentik-outpost-radius";
        image = "ghcr.io/goauthentik/radius:2026.8.1";
        autoStart = true;
        dependsOn = [serverContainer];
        # 1812 only: the appliance published no 1813 accounting port either, so there is
        # nothing here to mirror beyond authentication.
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
  # An **origin**, not a TLS vhost: since `gw` became the front door (2026-10-24) this
  # box terminates nothing — no certificate, no ACL, no gate.  `gw` proxies
  # `id.int.leighhack.org` here over plain HTTP on :80, and `mkOriginVhost` is what puts
  # `CONFIG.ROUTER_ONLY` at server level so the router is the only client that can
  # answer for it.  The pair of guards that keeps the two halves honest is
  # `machines/services1/int-dns.nix` (this box's vhosts == the table's `origin` rows)
  # and `machines/gw/int-dns.nix`; the row itself is `common/front-door.nix`.
  #
  # Which is also why this vhost cannot exist while staged: the row says `origin` in the
  # same commit that clears the flag, and a vhost the table does not grant is a refused
  # evaluation rather than a second config for one name (AGENTS.md rule 24).
  services.nginx.virtualHosts = lib.mkIf (!staged) {
    # `id.leighhack.org` and `authentik.int.leighhack.org` come across as aliases exactly
    # as the door has always served them (the wildcard cert covers both, and authentik's
    # tenant `base_url` — `https://id.int.leighhack.org`, a row in the migrated database —
    # is what absolute URLs are generated on).
    #
    # `authentik.int` is *also* in `common/int-dns.nix`'s `exempt`, and stays there: that
    # exemption is about the DNS *answer* (the name resolves to `10.3.1.36`, from
    # `common/dns-records.nix`, because RADIUS and LDAP dial it and cannot be re-pointed)
    # and it is not in tension with serving the same name here — the door answers HTTPS
    # for it, the outpost answers LDAPS, and since the cutover both land on this box.
    "id.int.leighhack.org" = mkOriginVhost {
      serverAliases = [
        "id.leighhack.org"
        "authentik.int.leighhack.org"
      ];
      proxyPass = "http://127.0.0.1:9000";
      websockets = true;
      proxyBuffering = false;
      # authentik builds absolute URLs from the request, so it is the one origin on this
      # box that cannot tolerate the hop rewriting `X-Forwarded-Proto`: it emitted
      # `base: "http://id.leighhack.org/"` into an HTTPS page and the browser blocked
      # `/api/v3/core/users/me/` as mixed content. It is also the app whose event log is
      # worthless if `X-Real-IP` says `10.3.1.1` for every login. Measured before flipping:
      # no GeoIP policies and no expression policy reads the address, so no policy decision
      # moves — see `common/nginx/origin-vhost.nix` before copying this to another origin,
      # where it is correct but not free.
      preserveForwardedHeaders = true;
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

  systemd.timers.authentik-db-dump = lib.mkIf (!staged) {
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
