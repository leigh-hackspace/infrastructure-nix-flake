# Status dashboard shared by all machines (Rust source: ../status-dashboard,
# zero external crates per house style).
#
# Each machine enables it and sets its own parameters:
#
#   services1:
#     services.status-dashboard = { enable = true; title = "services1"; };
#
#   aibox:
#     services.status-dashboard = {
#       enable = true;
#       title = "aibox";
#       mounts = [ "/mnt/filestore" "/mnt/ds-photos" ];
#       bind = "0.0.0.0";  # services1 reverse-proxies to it
#     };
#
# services1 is the SSL-terminating reverse proxy for every machine's
# dashboard (see machines/services1/services/status.nix).
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}:

let
  cfg = config.services.status-dashboard;

  statusDashboard = CRANE.cached {
    pname = "status-dashboard";
    version = "0.1.0";
    src = ../status-dashboard;
    cargoLock = CRANE.lockFile ../status-dashboard/Cargo.lock;
    sharedCrates = ["web"];
  };
in
{
  options.services.status-dashboard = {
    enable = lib.mkEnableOption "the systemd status dashboard";

    # NAS mount points the dashboard should probe on this host.
    mounts = lib.mkOption {
      type = lib.types.listOf lib.types.path;
      default = [
        "/mnt/cameras"
        "/mnt/filestore"
        "/mnt/backups"
      ];
      description = "NAS mount points to show in the dashboard's NAS card.";
    };

    # 127.0.0.1 unless services1 needs to reach this machine directly.
    bind = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Address the dashboard listens on.";
    };

    # Shared secret that the LAN-only (*.int) nginx vhost injects as
    # X-Status-Token, allowing restarts without SSO sign-in. Empty (the
    # default) means restarts require SSO only.
    #
    # Note: the token is embedded in the generated unit (and in the nginx
    # vhost's header), so it ends up in the world-readable nix store.  Treat it
    # as a LAN-only convenience credential; keep anything genuinely secret in
    # CONFIG.ENV_FILE (/run/secrets/env_file, root-only at runtime).
    restartToken = lib.mkOption {
      type = lib.types.str;
      default = "";
      apply = v: builtins.replaceStrings [ "\n" "\r" ] [ "" "" ] v;
      description = ''
        Token accepted by POST /api/restart in place of X-WEBAUTH-USER.
        Newlines are stripped: a token read from a secret file would otherwise
        carry yq's trailing newline into an nginx header and into the
        comparison, where it can never match.
      '';
    };

    # The live hostname is shown in the header regardless.
    title = lib.mkOption {
      type = lib.types.str;
      default = "status";
      description = "Page title.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.status-dashboard = INFRA.mkNeverGiveUp {
      description = "Systemd status dashboard";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      # Tools the dashboard shells out to (findmnt / ping / uptime / hostname).
      path = [
        pkgs.util-linux
        pkgs.iputils
        pkgs.procps
      ];
      serviceConfig = {
        Type = "simple";
        ExecStart = lib.concatStringsSep " " (
          [
            "${statusDashboard}/bin/status-dashboard"
            "--bind" cfg.bind
            "--port" "8088"
            "--mounts" (lib.concatStringsSep "," cfg.mounts)
            "--title" cfg.title
          ]
          # Only passed when a LAN token is configured.
          ++ lib.optional (cfg.restartToken != "")
          ("--restart-token ${cfg.restartToken}")
        );
      };
    };
  };
}
