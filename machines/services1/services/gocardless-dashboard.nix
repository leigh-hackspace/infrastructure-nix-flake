# gocardless-dashboard: GoCardless Pro sync + operator web UI (Rust; source
# in ../../../gocardless-dashboard).
#
# A Rust binary that every sync interval (default 15 min) pulls customers,
# mandates, subscriptions, payments, refunds and payouts from the GoCardless
# Pro API into a local PostgreSQL database (gocardless_dashboard, created by
# postgres.nix), and serves a Dioxus SPA (compiled to wasm here from
# ../../../gocardless-dashboard/frontend and embedded by build.rs — no bundle is
# committed) on 127.0.0.1:8095.
#
# Login is OIDC against authentik restricted to the `Infra` group; the
# client id/secret live in the shared env-file sops secret.  LAN-only,
# fronted by nginx as gocardless.int.leighhack.org (int record synced by
# dns-sync from the vhost list).
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  CONFIG = import ../config.nix;
  mkIntVhost = import ../lib/nginx-int-vhost-helper.nix {inherit lib;};

  # The Dioxus SPA compiled to wasm (pinned wasm-bindgen-cli and the wasm build
  # recipe live in common/crane.nix).  Its gdash-dto path dependency is a
  # symlink in the source tree, so materialise a real copy for the sandbox.
  frontendSrc = pkgs.runCommand "gocardless-dashboard-frontend-src" {} ''
    cp -r ${../../../gocardless-dashboard/frontend} $out
    # Store paths are read-only and cp -r preserves the mode, so make the
    # copy writable before adding the dto crate (the gdash-dto path
    # dependency; a symlink in the working tree, materialised here).
    chmod -R u+rwX $out
    rm -rf $out/dto
    cp -r ${../../../gocardless-dashboard/dto} $out/dto
  '';
  frontendDist = CRANE.wasmSpa {
    pname = "gocardless-dashboard-web";
    version = "0.1.0";
    src = frontendSrc;
    cargoLock = CRANE.lockFile ../../../gocardless-dashboard/frontend/Cargo.lock;
    wasmName = "gocardless_dashboard_web";
  };

  # The binary.  Its build.rs embeds the SPA, so the SPA is a build-hook input
  # rather than a cargo dependency: the args used for the cached deps are kept
  # clean so a SPA-only change does not rebuild the binary's dependency tree.
  dashboardArgs = {
    pname = "gocardless-dashboard";
    version = "0.1.0";
    src = ../../../gocardless-dashboard;
    cargoLock = CRANE.lockFile ../../../gocardless-dashboard/Cargo.lock;
    # `common-oidc` + `common-build-spa` path dependencies (see common-rs/ and
    # common/crane.nix).
    sharedCrates = ["oidc" "build-spa"];
  };
  dashboard = CRANE.cached (dashboardArgs
    // {
      cargoArtifacts = CRANE.deps dashboardArgs;
      # Point build.rs at the nix-built bundle instead of the (uncommitted)
      # frontend/dist in the source tree.
      preBuild = "export GOCARDLESS_DASHBOARD_DIST=${frontendDist}";
    });
in {
  # Runs as root (like the other CONFIG.ENV_FILE consumers): /run/secrets is
  # root:keys 0710, so only root can reach the secret files inside it.
  systemd.services.gocardless-dashboard = INFRA.mkNeverGiveUp {
    description = "GoCardless Pro sync + dashboard";
    wantedBy = ["multi-user.target"];
    after = ["postgresql.service" "network-online.target"];
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
  services.nginx.virtualHosts."gocardless.int.leighhack.org" = mkIntVhost {
    proxyPass = "http://127.0.0.1:8095";
  };
}
