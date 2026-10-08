# frigate-monitor: persistent scene-change monitor for the main_space camera
# (Rust source: ../frigate-monitor, single external crate: `image`; the web
# UI is a Dioxus SPA, compiled to wasm here from ../frigate-monitor/frontend
# and embedded into the binary by its build.rs — no bundle is committed).
#
# Grabs a snapshot of the Frigate RTSP substream on services1 every N
# seconds and diffs it against a slowly adapting background model.  Events
# only fire once the affected area has *settled*: the region must differ
# from the background and be completely still for `persist` consecutive
# snapshots, with the whole frame still too — moving people are dismissed,
# but a pencil left on / taken off a table or a chair moved to a new spot is
# recorded once everything has gone quiet.  The "before" image is taken from
# just before the change began.  Each event stores boxed before/after/diff
# images, zoom crops and a thumbnail; objects already recorded are absorbed
# into the background so their later removal is itself recorded.
#
# aibox has no nginx of its own; services1 reverse-proxies
# frigate-monitor.int.leighhack.org to 10.3.1.32:8090 (see
# machines/services1/services/frigate-monitor.nix).
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  cfg = config.services.frigate-monitor;

  # The Dioxus SPA compiled to wasm (this replaces the frontend/dist that used to
  # be committed to the git tree).  The pinned wasm-bindgen-cli and the wasm build
  # recipe live in common/crane.nix.
  frontendSrc = ../../frigate-monitor/frontend;
  frontendDist = CRANE.wasmSpa {
    pname = "frigate-monitor-web";
    version = "0.1.0";
    src = frontendSrc;
    cargoLock = CRANE.lockFile ../../frigate-monitor/frontend/Cargo.lock;
    wasmName = "frigate_monitor_web";
  };

  # The binary.  Its build.rs embeds the SPA, so the SPA is a build-hook input
  # rather than a cargo dependency: the args used for the cached deps are kept
  # clean so a SPA-only change does not rebuild the binary's dependency tree.
  frigateMonitorArgs = {
    pname = "frigate-monitor";
    version = "0.2.0";
    src = ../../frigate-monitor;
    cargoLock = CRANE.lockFile ../../frigate-monitor/Cargo.lock;
    # `common-build-spa` path dependency (see common-rs/ and common/crane.nix).
    sharedCrates = ["build-spa"];
  };
  frigateMonitor = CRANE.cached (frigateMonitorArgs
    // {
      cargoArtifacts = CRANE.deps frigateMonitorArgs;
      # Point build.rs at the nix-built bundle instead of the (uncommitted)
      # frontend/dist in the source tree.
      preBuild = "export FRIGATE_MONITOR_DIST=${frontendDist}";
    });
in {
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

    # A change must be foreground *and completely still* for this many
    # consecutive snapshots before it is recorded (moving people never are).
    # 4 x 10 s = 40 s of stillness after the motion stops.
    persist = lib.mkOption {
      type = lib.types.int;
      default = 4;
      description = "Consecutive still snapshots required before an event is recorded.";
    };
  };

  config = lib.mkMerge [
    # This machine-specific module exists for the service; on by default.
    {services.frigate-monitor.enable = true;}

    (lib.mkIf cfg.enable {
      systemd.services.frigate-monitor = INFRA.mkNeverGiveUp {
        description = "Frigate main_space scene-change monitor";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target"];
        wants = ["network-online.target"];
        # The RTSP source is on services1; keep trying forever (infra policy).
        serviceConfig = {
          Type = "simple";
          DynamicUser = true;
          StateDirectory = "frigate-monitor";
          ExecStart = lib.concatStringsSep " " [
            "${frigateMonitor}/bin/frigate-monitor"
            "--bind"
            cfg.bind
            "--port"
            (toString cfg.port)
            "--rtsp"
            cfg.rtsp
            "--interval"
            (toString cfg.intervalSec)
            "--persist"
            (toString cfg.persist)
            "--data-dir"
            "/var/lib/frigate-monitor"
            "--ffmpeg"
            "${pkgs.ffmpeg}/bin/ffmpeg"
          ];
        };
      };
    })
  ];
}
