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
{ config, lib, pkgs, ... }:

let
  CONFIG = import ../config.nix;

  # wasm-bindgen-cli pinned to 0.2.128 — the exact wasm-bindgen version the
  # SPA is compiled against (frontend/Cargo.lock); the cli and the
  # wasm-bindgen runtime lib in the wasm must match or the generated JS glue
  # is incompatible.  nixpkgs only ships older versions, so build this one
  # here (same recipe as machines/aibox/frigate-monitor.nix).
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
    cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
      pname = "wasm-bindgen-cli";
      version = "0.2.128";
      src = wasmBindgenCliSrc;
      hash = "sha256-R1Tas33Ursy8kqsxguAkG0ZhNed2n5uFTAhw1l2qlLY=";
    };
  };

  # The Dioxus SPA compiled to wasm.  Its gdash-dto path dependency is a
  # symlink in the source tree, so materialise a real copy for the sandbox.
  frontendSrc = pkgs.runCommand "gocardless-dashboard-frontend-src" { } ''
    cp -r ${../../../gocardless-dashboard/frontend} $out
    # Store paths are read-only and cp -r preserves the mode, so make the
    # copy writable before adding the dto crate (the gdash-dto path
    # dependency; a symlink in the working tree, materialised here).
    chmod -R u+rwX $out
    rm -rf $out/dto
    cp -r ${../../../gocardless-dashboard/dto} $out/dto
  '';
  frontendDist = pkgs.rustPlatform.buildRustPackage {
    pname = "gocardless-dashboard-web";
    version = "0.1.0";
    src = frontendSrc;
    cargoLock.lockFile = ../../../gocardless-dashboard/frontend/Cargo.lock;
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
        target/wasm32-unknown-unknown/release/gocardless_dashboard_web.wasm
      cp index.html $out/index.html
      runHook postInstall
    '';
  };

  dashboard = pkgs.rustPlatform.buildRustPackage {
    pname = "gocardless-dashboard";
    version = "0.1.0";
    src = ../../../gocardless-dashboard;
    cargoLock.lockFile = ../../../gocardless-dashboard/Cargo.lock;
    doCheck = false;
    # build.rs embeds the SPA; point it at the nix-built bundle instead of
    # the (uncommitted) frontend/dist in the source tree.
    preBuild = "export GOCARDLESS_DASHBOARD_DIST=${frontendDist}";
  };
in
{
  # Runs as root (like the other CONFIG.ENV_FILE consumers): /run/secrets is
  # root:keys 0710, so only root can reach the secret files inside it.
  systemd.services.gocardless-dashboard = {
    description = "GoCardless Pro sync + dashboard";
    wantedBy = [ "multi-user.target" ];
    after = [ "postgresql.service" "network-online.target" ];
    requires = [ "postgresql.service" ];
    serviceConfig = {
      Type = "simple";
      StateDirectory = "gocardless-dashboard";
      ExecStart = lib.concatStringsSep " " [
        "${dashboard}/bin/gocardless-dashboard"
        "--env-file" CONFIG.ENV_FILE
        "--port" "8095"
      ];
      # Never give up.
      Restart = "always";
      RestartSec = "5s";
    };
    startLimitIntervalSec = 0;
  };

  # LAN-only vhost (the binary only listens on 127.0.0.1).
  services.nginx.virtualHosts."gocardless.int.leighhack.org" = {
    useACMEHost = "leighhack.org";
    forceSSL = true;

    locations."/" = {
      proxyPass = "http://127.0.0.1:8095";
      recommendedProxySettings = true;
      extraConfig = CONFIG.LOCAL_NETWORK;
    };
  };
}
