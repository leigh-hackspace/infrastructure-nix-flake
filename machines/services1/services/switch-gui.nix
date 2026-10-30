# switch-gui: ports and VLANs on the network's ProCurve switches, in a browser.
#
# Rust source in ../../../crates/switch-gui.  It reads each switch's configuration over
# Telnet — merging the five `show` tables that split a port's name, link state and
# VLAN membership across them — and serves a Dioxus SPA (compiled to wasm here
# from ../../../crates/switch-gui/frontend and embedded by build.rs — no bundle is
# committed) at switch.int.leighhack.org on 127.0.0.1:8097.
#
# ## Where the page's data comes from
#
# Nothing here reads a switch while a request is being served.  A background sync
# (`src/sync.rs`) re-reads each switch on an interval into a **shadow copy in
# Postgres** and drains a **queue** of wanted changes in the same database, which
# is also the audit trail; the SPA (`GET /api/switch`) reads the shadow copy and
# appends to the queue.  That is why the unit needs Postgres, why a page load is
# fast, and why a dead switch costs a stale page and a visible error rather than a
# spinner or a timeout in the browser.  It is also why the write path is a row
# rather than a Telnet session: the operator sees *queued*, then *applied*, then
# what the switch said, and another operator's change is visible in the same table.
#
# The database is `switch_gui` (role and database created by `postgres.nix`).  The
# DSN defaults to the **unix socket** (`postgresql://switch_gui@/switch_gui?host=/run/postgresql`),
# which `CONFIG.PG_AUTH` lets any local user in as (`local all all trust`) — so
# there is no database credential to store, and `--db-url` is not passed.
#
# Three switches, from `common/hosts.nix`: switch1 (10.3.1.2), switch2
# (10.3.1.3) and switch3 (10.3.1.4).  The first two are the same model (J9280A
# 2510G-48, Y.11.44); switch3 is a J4903A 2824 on I.10.107, which is why the
# parsers read their column layout off the header rather than assuming it, and
# why the client answers its `Press any key to continue` gate before typing a
# login name (see switch-gui/README.md).  Each switch is read on its own blocking
# task, so the 2824's slow login cannot hold up the other two.
#
# One login serves all three: the same `agent` account is a Manager on each, so
# there is one credential pair and one Telnet session per read — the CLI is one
# console per device.
#
# The credentials live in one sops secret, a KEY=value file (not a template),
# created on 2026-10-12 — see `switch_gui_env_file` in ../sops.nix.  Keys:
#
#   SWITCH_GUI_LOGIN=<switch login>
#   SWITCH_GUI_PASSWORD=<switch password>
#   SWITCH_GUI_OIDC_CLIENT_ID=<authentik client id>
#   SWITCH_GUI_OIDC_CLIENT_SECRET=<authentik client secret>
#   SWITCH_GUI_ENABLE_LOGIN=<optional Manager login; defaults to SWITCH_GUI_LOGIN>
#   SWITCH_GUI_ENABLE_PASSWORD=<optional Manager password; defaults to the password>
#   SWITCH_GUI_SWITCHES=<name=address list; set below, not in the secret>
#   SWITCH_GUI_DB_URL=<optional DSN; the default is the local socket>
#
# Without `SWITCH_GUI_LOGIN`/`_PASSWORD` the service exits at startup naming the
# missing key (an empty password would otherwise fail to authenticate on every
# pass, which reads like a wrong password and not at all like the missing secret it
# is); without the OIDC pair it refuses to serve anything but /healthz on a
# non-loopback bind.  The authentik side is provider pk 35 ("switch-gui"), which
# uses the authorization-code + S256 PKCE contract in `common-rs/oidc` and
# redirects to https://switch.int.leighhack.org/auth/callback; it advertises the
# `groups` scope, without which every login is refused as "not in Infra".  Read
# docs/authentik-oidc-notes.md before touching it — including how to add a second
# consumer of the groups scope without tripping the ORM.  **The redirect URI and
# the required group are compiled in** (`src/main.rs`): they used to be
# `SWITCH_GUI_OIDC_REDIRECT` / `SWITCH_GUI_REQUIRED_GROUP` env vars, and moving
# them was a deliberate trade for one config path, so changing either is a code
# change and a deploy.
#
# Why the switch account is not enough on its own: the switch authenticates
# against RADIUS, so a browser login here would either be a second, weaker door
# to the same device or a shared password.  Sign-in is authentik gated on the
# `Infra` group instead, and the switch's own credentials stay on the server.
#
# Writes need the Manager context, and `agent` has it: `enable` is a second
# login on this firmware and the same credentials answer it, so the enable pair
# defaults to the login pair.  Writes are **on** here: they were disabled on a
# diagnosis that turned out to be wrong (the console did echo configuration
# commands; the prompt matcher could not see a prompt painted on the same row as
# the echo, so every write "timed out" after applying).  Proven end to end on a
# dead port of switch1 on 2026-10-13 — see switch-gui/README.md, "The write path".
# Configuration applies inside config mode (`Switch::apply` enters it), and nothing
# here saves to NVRAM, so a change is in RAM until someone writes it.  `--read-only`
# is still the escape hatch if the write path ever misbehaves: dropping that flag
# is a one-line change and a switch.
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  HOSTS = import ../../../common/hosts.nix;
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};

  port = 8097;
  hostname = "switch.int.leighhack.org";

  # How often the sync re-reads each switch's whole configuration.
  #
  # A full read is six-plus Telnet round-trips against a 1990s CPU that pages its
  # own output — and on switch3 every table pages, so a read is nearer a second
  # than a tenth.  It is also a **login**, and the switch tries RADIUS first
  # (authentik refuses `agent` today — see docs/procurve-switches.md — and the
  # switch falls back to its local database), so every read logs two `login_failed`
  # events in authentik.  That is the reason this is 60s rather than the binary's
  # 30s default; when the RADIUS password is fixed, dropping it back is fine.
  # The SPA's Refresh button asks for an immediate re-read either way.
  syncIntervalSecs = 60;

  # The switches as `name=address` pairs.  The names are the switches' own
  # `System Name` (which is also what the GUI's URLs use), so this table, the
  # gatus checks and the switches' own `System Name` agree.
  switches = [
    "switch1=${HOSTS.switch1}"
    "switch2=${HOSTS.switch2}"
    "switch3=${HOSTS.switch3}"
  ];

  # The SPA and the binary that embeds it — `CRANE.spaApp`, the one shape all four
  # browser-served apps share (common/crane.nix has the recipe, the pinned
  # wasm-bindgen-cli and why the bundle is a build-hook input rather than a cargo
  # dependency; common/frontend-build-spa.sh is the one recipe the flake and
  # `just build-frontend` both run).  What is specific to this app: `dto` is a real
  # directory inside the crate which the SPA reaches through a git-ignored symlink, so it
  # is named as a local crate; and the binary reads `common-json` (the `show` table
  # readers), `common-web` (the HTTP plumbing), `common-oidc` (authentik login and the
  # Infra gate), `common-pg` (the pool and the sea-query→sqlx glue) and
  # `common-build-spa` (build.rs) — with no entry needed for `dto`.
  switchGui =
    (CRANE.spaApp {
      name = "switch-gui";
      version = "0.1.0";
      localCrates = ["dto"];
      sharedCrates = ["json" "web" "oidc" "pg" "service" "build-spa"];
      # `ui` is not a Rust dependency of the binary: it is a manifest path dependency of
      # the *frontend* (`common-rs/ui` through the git-ignored symlink), so it has to be
      # copied into the frontend's source tree — `spaSharedCrates`, not `sharedCrates`.
      # In the source tree the symlink makes it work; in a flake source a symlink cannot
      # travel, so the copy is the build (same shape as filestore's `preview`).
      spaSharedCrates = ["ui"];
    }).binary;
in {
  systemd.services.switch-gui = INFRA.mkNeverGiveUp {
    description = "switch-gui: port and VLAN management for the ProCurve switches";
    wantedBy = ["multi-user.target"];
    after = ["network-online.target" "postgresql.service"];
    wants = ["network-online.target"];
    # The shadow copy and the queue are the application's state: without
    # Postgres there is nothing to read and nowhere to queue a change, so this
    # is `requires`, not `wants`.  It also starts *after* Postgres, which matters
    # because the schema is created at startup (`shadow::migrate`,
    # `changes::migrate`) and a race with the socket would be a crash loop.
    requires = ["postgresql.service"];

    # Non-secret wiring.  The switch addresses come from common/hosts.nix so the
    # GUI, the gatus checks and the switches' own `System Name` agree, and they
    # are addresses rather than names on purpose: this is a tool for debugging the
    # network, so it must not resolve anything.
    environment = {
      SWITCH_GUI_SWITCHES = lib.strings.concatStringsSep "," switches;
    };

    serviceConfig = {
      Type = "simple";
      ExecStart = lib.concatStringsSep " " [
        "${switchGui}/bin/switch-gui"
        "--bind"
        "127.0.0.1:${toString port}"
        "--interval"
        (toString syncIntervalSecs)
        # No --db-url: the default DSN is the local unix socket, which
        # CONFIG.PG_AUTH trusts for local connections (see the header).
        #
        # No --read-only: writes are enabled, because they work.
      ];

      # The switch login, its password and the OIDC client pair.  systemd reads
      # this as PID 1 before dropping privileges, so the service itself never
      # needs to open the secret file — which is why it can be an unprivileged
      # dynamic user rather than root (the pattern filestore/gocardless avoid by
      # reading CONFIG.ENV_FILE themselves; a LAN-facing web app should not run
      # as a privileged account — the same rule that moved network-status to a
      # DynamicUser on gw).
      #
      # The database needs no credential either: `/run/postgresql` is world-executable
      # and `local all all trust`, so a DynamicUser connects as the `switch_gui`
      # role over the socket.
      EnvironmentFile = config.sops.secrets.switch_gui_env_file.path;

      # No files of its own: the snapshot, the queue and the audit log are all in
      # Postgres, so there is no StateDirectory to leave behind and nothing to
      # lose on a restart but an in-flight console conversation (which
      # `changes::recover_stale` puts back in the queue at startup).
      DynamicUser = true;
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      PrivateDevices = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectControlGroups = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      # Outbound: Telnet to the switches, HTTPS to authentik (through reqwest +
      # rustls, so no curl and no CA bundle of its own), and the unix socket to
      # Postgres.  Nothing listens on anything but the loopback listener, which is
      # AF_INET/AF_INET6 too.
      RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
      RestrictNamespaces = true;
      CapabilityBoundingSet = "";
    };
  };

  # LAN-only vhost (the binary only listens on 127.0.0.1).  Its `*.int` record
  # comes from common/int-dns.nix, which the drift check in
  # machines/services1/int-dns.nix keeps equal to the table's origin rows —
  # deploying services1 and gw is all `switch.int.leighhack.org` needs.
  #
  # The SPA is a single document with client-side routes (`/switch/switch2` is a
  # route, not a file), so the vhost must not add a `location` block that would
  # swallow `/api/*`: `mkOriginVhost` proxies everything, which is what the app
  # expects.  Uploads are not a thing here, so the default body limit is fine.
  services.nginx.virtualHosts.${hostname} = mkOriginVhost {
    proxyPass = "http://127.0.0.1:${toString port}";
  };
}
