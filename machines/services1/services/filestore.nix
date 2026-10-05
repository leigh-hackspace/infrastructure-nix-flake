# filestore: web file browser for /mnt/filestore (Rust; source in
# ../../../filestore).
#
# An axum binary that serves a Dioxus SPA (compiled to wasm here from
# ../../../filestore/frontend and embedded by build.rs — no bundle is
# committed) on 127.0.0.1:8096, exposing the /mnt/filestore tree: browse,
# multi-select, right-click menus, drag-and-drop upload (files and folders),
# move/copy, rename, delete, new folder/file, text+image previews, shallow/
# deep search and streaming ZIP downloads.
#
# Login is OIDC against authentik restricted to the `Infra` group (same
# recipe as gocardless-dashboard); the client id/secret live in the shared
# env-file sops secret.  LAN-only, fronted by nginx as
# filestore.int.leighhack.org (the int record is synced by dns-sync from the
# vhost list — it replaces the old commented-out vhost in http.nix).
{ config, lib, pkgs, ... }:

let
  CONFIG = import ../config.nix;

  # wasm-bindgen-cli pinned to 0.2.128 — the exact wasm-bindgen version the
  # SPA is compiled against (frontend/Cargo.lock); the cli and the
  # wasm-bindgen runtime lib in the wasm must match or the generated JS glue
  # is incompatible.  nixpkgs only ships older versions, so build it here
  # (same recipe as gocardless-dashboard.nix).
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

  # The Dioxus SPA compiled to wasm.
  frontendDist = pkgs.rustPlatform.buildRustPackage {
    pname = "filestore-web";
    version = "0.1.0";
    src = ../../../filestore/frontend;
    cargoLock.lockFile = ../../../filestore/frontend/Cargo.lock;
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
        target/wasm32-unknown-unknown/release/filestore_web.wasm
      cp index.html $out/index.html
      runHook postInstall
    '';
  };

  filestore = pkgs.rustPlatform.buildRustPackage {
    pname = "filestore";
    version = "0.1.0";
    src = ../../../filestore;
    cargoLock.lockFile = ../../../filestore/Cargo.lock;
    doCheck = false;
    # build.rs embeds the SPA; point it at the nix-built bundle instead of
    # the (uncommitted) frontend/dist in the source tree.
    preBuild = "export FILESTORE_DIST=${frontendDist}";
  };
in
{
  # The backing store is the /mnt/filestore NFS share on the NAS, so the
  # service must wait for the NAS (see nfs-client.nix) and keep restarting
  # forever (the "never give up" infra policy).
  systemd.services.filestore = {
    description = "filestore web file browser (/mnt/filestore)";
    wantedBy = [ "multi-user.target" ];
    after = [ "wait-for-nas.service" "network-online.target" ];
    requires = [ "wait-for-nas.service" ];
    serviceConfig = {
      Type = "simple";
      ExecStart = lib.concatStringsSep " " [
        "${filestore}/bin/filestore"
        "--root" "/mnt/filestore"
        "--env-file" CONFIG.ENV_FILE
        "--port" "8096"
      ];
      # Never give up.
      Restart = "always";
      RestartSec = "5s";
    };
    startLimitIntervalSec = 0;
  };

  # LAN-only vhost (the binary only listens on 127.0.0.1).
  services.nginx.virtualHosts."filestore.int.leighhack.org" = {
    useACMEHost = "leighhack.org";
    forceSSL = true;

    locations."/" = {
      proxyPass = "http://127.0.0.1:8096";
      recommendedProxySettings = true;
      extraConfig = CONFIG.LOCAL_NETWORK;
    };
  };
}
