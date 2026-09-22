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
{ config, lib, pkgs, ... }:

let
  cfg = config.services.frigate-monitor;

  # crates.io refuses the python-requests default user agent that nixpkgs'
  # cargo vendor fetcher uses (403 from our network); source the patch
  # script in the vendor derivation's preBuild so it downloads with a
  # cargo-style UA.  Only affects first-time vendor fetches (they are
  # cached as fixed-output derivations afterwards).
  cargoVendor = { pname, version, src, hash }:
    pkgs.rustPlatform.fetchCargoVendor {
      inherit pname version src hash;
      preBuild = "source ${../../frigate-monitor/cargo-vendor-ua-patch.sh}";
    };

  # wasm-bindgen-cli pinned to 0.2.128 — the exact wasm-bindgen version the
  # SPA is compiled against (frontend/Cargo.lock); the cli and the wasm-bindgen
  # runtime lib in the wasm must match or the generated JS glue is
  # incompatible.  nixpkgs only ships older versions, so build this one here.
  wasmBindgenCliSrc = pkgs.fetchurl {
    name = "wasm-bindgen-cli-0.2.128.tar.gz";
    # crates.io's API download endpoint is blocked from our network; static
    # is fine.
    url = "https://static.crates.io/crates/wasm-bindgen-cli/wasm-bindgen-cli-0.2.128.crate";
    hash = "sha256-LikUDAToGDKQK3Dl03uc4b+oEcj+RWO+oI9234OIzyA=";
  };
  wasmBindgenCli = pkgs.buildWasmBindgenCli {
    version = "0.2.128";
    src = wasmBindgenCliSrc;
    cargoDeps = cargoVendor {
      pname = "wasm-bindgen-cli";
      version = "0.2.128";
      src = wasmBindgenCliSrc;
      hash = "sha256-R1Tas33Ursy8kqsxguAkG0ZhNed2n5uFTAhw1l2qlLY=";
    };
  };

  # The Dioxus SPA compiled to wasm (this replaces the frontend/dist that
  # used to be committed to the git tree).  Unlike network-status's npm
  # bundle it is pure cargo, so it builds hermetically here under cargoDeps.
  frontendSrc = ../../frigate-monitor/frontend;
  frontendDist = pkgs.rustPlatform.buildRustPackage {
    pname = "frigate-monitor-web";
    version = "0.1.0";
    src = frontendSrc;
    cargoDeps = cargoVendor {
      pname = "frigate-monitor-web";
      version = "0.1.0";
      src = frontendSrc;
      hash = "sha256-hRkS6WOLLeezK4dddHxShuQG2BGkvbpNxWPf4VVSfRA=";
    };
    nativeBuildInputs = [ wasmBindgenCli pkgs.lld ];
    doCheck = false;
    # Wasm-only build.  Override the default phase because cargoBuildHook
    # always adds the host target as well.  nixpkgs' rustc ships no wasm
    # linker (rustup's does), so use wasm-ld from pkgs.lld.
    buildPhase = ''
      runHook preBuild
      export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld
      cargo build --release --target wasm32-unknown-unknown --offline
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      wasm-bindgen --target web --out-dir $out --no-typescript \
        target/wasm32-unknown-unknown/release/frigate_monitor_web.wasm
      cp index.html $out/index.html
      runHook postInstall
    '';
  };

  frigateMonitor = pkgs.rustPlatform.buildRustPackage {
    pname = "frigate-monitor";
    version = "0.2.0";
    src = ../../frigate-monitor;
    cargoLock.lockFile = ../../frigate-monitor/Cargo.lock;
    doCheck = false;
    # build.rs embeds the SPA; point it at the nix-built bundle instead of
    # the (uncommitted) frontend/dist in the source tree.
    preBuild = "export FRIGATE_MONITOR_DIST=${frontendDist}";
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
