# frigate-monitor: persistent scene-change monitor for the main_space camera
# (Rust source: ../frigate-monitor, single external crate: `image`).
#
# Grabs a snapshot of the Frigate RTSP substream on services1 every N
# seconds, diffs it against a slowly adapting background model, and records
# an event (before/after + zoom, changed regions outlined) when a change
# persists across several snapshots — so moving people are ignored but a
# pair of scissors left on a table, or a chair moved, is caught.
#
# aibox has no nginx of its own; services1 reverse-proxies
# frigate-monitor.int.leighhack.org to 10.3.1.32:8090 (see
# machines/services1/services/frigate-monitor.nix).
{ config, lib, pkgs, ... }:

let
  cfg = config.services.frigate-monitor;

  frigateMonitor = pkgs.rustPlatform.buildRustPackage {
    pname = "frigate-monitor";
    version = "0.1.0";
    src = ../../frigate-monitor;
    cargoLock.lockFile = ../../frigate-monitor/Cargo.lock;
    doCheck = false;
  };
in
{
  options.services.frigate-monitor = {
    enable = lib.mkEnableOption "the main_space scene-change monitor";

    rtsp = lib.mkOption {
      type = lib.types.str;
      default = "rtsp://10.3.1.20:8554/main_space";
      description = "RTSP stream to monitor (Frigate on services1).";
    };

    bind = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0";
      description = "Address the web UI listens on.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 8090;
      description = "Port the web UI listens on.";
    };

    intervalSec = lib.mkOption {
      type = lib.types.int;
      default = 10;
      description = "Seconds between snapshots.";
    };

    # A change must persist this many consecutive snapshots to be recorded
    # (moving people never do). 4 x 10 s = 40 s.
    persist = lib.mkOption {
      type = lib.types.int;
      default = 4;
      description = "Consecutive differing snapshots required before an event is recorded.";
    };
  };

  config = lib.mkMerge [
    # This machine-specific module exists for the service; on by default.
    { services.frigate-monitor.enable = true; }

    (lib.mkIf cfg.enable {
      systemd.services.frigate-monitor = {
        description = "Frigate main_space scene-change monitor";
        wantedBy = [ "multi-user.target" ];
        after = [ "network-online.target" ];
        wants = [ "network-online.target" ];
        # The RTSP source is on services1; keep trying forever (infra policy).
        serviceConfig = {
          Type = "simple";
          DynamicUser = true;
          StateDirectory = "frigate-monitor";
          ExecStart = lib.concatStringsSep " " [
            "${frigateMonitor}/bin/frigate-monitor"
            "--bind" cfg.bind
            "--port" (toString cfg.port)
            "--rtsp" cfg.rtsp
            "--interval" (toString cfg.intervalSec)
            "--persist" (toString cfg.persist)
            "--data-dir" "/var/lib/frigate-monitor"
            "--ffmpeg" "${pkgs.ffmpeg}/bin/ffmpeg"
          ];
          Restart = "always";
          RestartSec = "5s";
        };
        # Never give up.
        startLimitIntervalSec = 0;
      };
    })
  ];
}
