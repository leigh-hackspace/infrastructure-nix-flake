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
{ config, lib, pkgs, crane, INFRA, ... }:

let
  CONFIG = import ../config.nix;
  CRANE = import ../../../common/crane.nix { inherit pkgs crane; };

  # The Dioxus SPA compiled to wasm (pinned wasm-bindgen-cli and the wasm build
  # recipe live in common/crane.nix).
  frontendDist = CRANE.wasmSpa {
    pname = "filestore-web";
    version = "0.1.0";
    src = ../../../filestore/frontend;
    cargoLock = CRANE.lockFile ../../../filestore/frontend/Cargo.lock;
    wasmName = "filestore_web";
  };

  # The binary.  Its build.rs embeds the SPA, so the SPA is a build-hook input
  # rather than a cargo dependency: the args used for the cached deps are kept
  # clean so a SPA-only change does not rebuild the binary's dependency tree.
  filestoreArgs = {
    pname = "filestore";
    version = "0.1.0";
    src = ../../../filestore;
    cargoLock = CRANE.lockFile ../../../filestore/Cargo.lock;
    # `common-oidc` + `common-build-spa` path dependencies (see common-rs/ and
    # common/crane.nix).
    sharedCrates = ["oidc" "build-spa"];
  };
  filestore = CRANE.cached (filestoreArgs // {
    cargoArtifacts = CRANE.deps filestoreArgs;
    # Point build.rs at the nix-built bundle instead of the (uncommitted)
    # frontend/dist in the source tree.
    preBuild = "export FILESTORE_DIST=${frontendDist}";
  });
in
{
  # The backing store is the /mnt/filestore NFS share on the NAS, so the
  # service must wait for the NAS (see nfs-client.nix) and keep restarting
  # forever (the "never give up" infra policy).
  systemd.services.filestore = INFRA.mkNeverGiveUp {
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
        "--max-upload" "2G"
      ];
    };
  };

  # LAN-only vhost (the binary only listens on 127.0.0.1).
  services.nginx.virtualHosts."filestore.int.leighhack.org" = {
    useACMEHost = "leighhack.org";
    forceSSL = true;

    locations."/" = {
      proxyPass = "http://127.0.0.1:8096";
      recommendedProxySettings = true;
      extraConfig = ''
        # Uploads are raw request bodies.  nginx's default here is 10m, which
        # 413'd every drag-in upload bigger than that (the popup the UI shows is
        # nginx's error page, not the app's), so raise it to the limit the
        # backend enforces with --max-upload 2G.
        client_max_body_size 2048M;

        ${CONFIG.LOCAL_NETWORK}
      '';
    };
  };
}
