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
{ config, lib, pkgs, CRANE, INFRA, ... }:

let
  CONFIG = import ../config.nix;
  mkIntVhost = import ../lib/nginx-int-vhost-helper.nix { inherit lib; };

  # The upload limit, in the two places it has to agree: the backend's
  # --max-upload and nginx's client_max_body_size.  Written as bytes so the
  # nginx value is derived from it rather than hand-copied.
  maxUploadBytes = 2 * 1024 * 1024 * 1024; # 2G

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
  # service must wait for the NAS (common/nas.nix) and keep restarting forever
  # (the "never give up" infra policy, common/systemd.nix).
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
        "--max-upload" "${toString maxUploadBytes}"
      ];
    };
  };

  # LAN-only vhost (the binary only listens on 127.0.0.1).
  services.nginx.virtualHosts."filestore.int.leighhack.org" = mkIntVhost {
    proxyPass = "http://127.0.0.1:8096";
    # Uploads are raw request bodies, so the vhost's client_max_body_size must
    # match the --max-upload above: nginx's default here is 10m, which 413'd
    # every drag-in upload bigger than that (the popup the UI shows is nginx's
    # error page, not the app's).
    bodySize = "${toString (maxUploadBytes / (1024 * 1024))}M";
  };
}
