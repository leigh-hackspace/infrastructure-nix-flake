# gocardless-dashboard: GoCardless Pro sync + operator web UI (Rust; source
# in ../../../crates/gocardless-dashboard).
#
# A Rust binary that every sync interval (default 15 min) pulls customers,
# mandates, subscriptions, payments, refunds and payouts from the GoCardless
# Pro API into a local PostgreSQL database (gocardless_dashboard, created by
# postgres.nix), and serves a Dioxus SPA (compiled to wasm here from
# ../../../crates/gocardless-dashboard/frontend and embedded by build.rs — no bundle is
# committed) on 127.0.0.1:8095.
#
# Login is OIDC against authentik restricted to the `Infra` group; the
# client id/secret live in the shared env-file sops secret.  LAN-only,
# fronted by nginx as gocardless.int.leighhack.org, answered by the router from
# common/int-dns.nix (which this vhost name has to be in — see
# machines/services1/int-dns.nix).
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};

  # The SPA and the binary that embeds it — `CRANE.spaApp`, the one recipe (see
  # common/crane.nix for the shape and for why the bundle is a build-hook input rather
  # than a cargo dependency).  What is specific to this app: `gdash-dto` is a real
  # directory inside the crate which the SPA reaches through a git-ignored symlink, so it
  # is named as a local crate; the binary reads `common-oidc`, `common-service`,
  # `common-build-spa` and `common-pg` (crates/common-rs/), and needs no entry for `dto`.
  dashboard =
    (CRANE.spaApp {
      name = "gocardless-dashboard";
      version = "0.1.0";
      localCrates = ["dto"];
      sharedCrates = ["oidc" "service" "build-spa" "pg"];
    }).binary;
in {
  # Runs as root (like the other CONFIG.ENV_FILE consumers): /run/secrets is
  # root:keys 0710, so only root can reach the secret files inside it.
  systemd.services.gocardless-dashboard = INFRA.mkNeverGiveUp {
    description = "GoCardless Pro sync + dashboard";
    wantedBy = ["multi-user.target"];
    after = ["postgresql.service" "network-online.target"];
    wants = ["network-online.target"];
    requires = ["postgresql.service"];
    serviceConfig = {
      Type = "simple";
      StateDirectory = "gocardless-dashboard";
      ExecStart = lib.concatStringsSep " " [
        "${dashboard}/bin/gocardless-dashboard"
        "--env-file"
        CONFIG.ENV_FILE
        "--port"
        "8095"
      ];
    };
  };

  # Daily authentik `Members` group sync — the Rust replacement for the old
  # Python gocardless-tools job (active_members_to_authentik.py).  A oneshot
  # running the same binary in --authentik-sync mode: it reads the local DB
  # the daemon keeps fresh (no GoCardless token needed) and writes its audit
  # log to Postgres (visible in the GUI under “Members sync”).
  #
  # sudo systemctl start gocardless-authentik-sync
  # journalctl -u gocardless-authentik-sync -f
  systemd.services.gocardless-authentik-sync = {
    description = "GoCardless -> authentik Members group sync";
    after = ["postgresql.service" "network-online.target"];
    wants = ["network-online.target"];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = lib.concatStringsSep " " [
        "${dashboard}/bin/gocardless-dashboard"
        "--env-file"
        CONFIG.ENV_FILE
        "--authentik-sync"
      ];
    };
  };

  systemd.timers.gocardless-authentik-sync = {
    description = "Daily GoCardless -> authentik Members group sync";
    timerConfig = {
      Unit = "gocardless-authentik-sync.service";
      OnCalendar = "*-*-* 01:00:00";
    };
    wantedBy = ["timers.target"];
  };

  # LAN-only vhost (the binary only listens on 127.0.0.1).
  services.nginx.virtualHosts."gocardless.int.leighhack.org" = mkOriginVhost {
    proxyPass = "http://127.0.0.1:8095";
  };
}
