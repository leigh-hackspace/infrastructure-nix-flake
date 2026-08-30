# network-status: real-time web dashboard for the OPNsense router (10.3.1.1).
#
# Two parts, built together here:
#
#   1. frontend/ — a SolidJS + TypeScript SPA. It polls the backend's JSON API
#      every few seconds and renders per-interface bandwidth, firewall
#      connection (pf state) counts, CPU/memory/load and potential issues on
#      hand-rolled canvas charts. Bundled with esbuild + esbuild-plugin-solid
#      (both unscoped — @vitejs/* is unreachable from this network's registry)
#      into dist/ (bundle.js, bundle.css, index.html). Dependencies are kept
#      minimal: only solid-js at runtime, no router/state/chart libraries.
#
#   2. The Rust binary (../../network-status, zero external crates — house
#      style, see status-dashboard/). It ssh's to the router every few seconds
#      with the machine-hop key, keeps a rolling in-memory history, and serves
#      the built SPA (via --static-dir) plus the JSON API. When no static dir
#      is given it falls back to a minimal embedded page, so it still runs
#      standalone for local dev.
#
# Exposed at network-info.int.leighhack.org (nginx vhost in http.nix,
# ACL-restricted to the local network).
{
  config,
  lib,
  pkgs,
  ...
}:

let
  # Prebuilt SolidJS/TypeScript SPA. The bundle (dist/) is committed to the
  # git tree and copied into the store here, so the build is fully hermetic:
  # Nix builds run offline and the npm deps (esbuild, babel, ...) are not in
  # nixpkgs's vendored npm archive, so buildNpmPackage can't fetch them
  # deterministically. Regenerate dist/ locally with `npm install && npm run
  # build` in network-status/frontend after editing the SPA source.
  frontend = pkgs.stdenv.mkDerivation {
    pname = "network-status-frontend";
    version = "0.1.0";
    src = ../../network-status/frontend;
    dontConfigure = true;
    dontBuild = true;
    installPhase = ''
      mkdir -p $out/share/network-status
      cp -r dist/. $out/share/network-status/frontend
    '';
  };

  networkStatus = pkgs.rustPlatform.buildRustPackage {
    pname = "network-status";
    version = "0.1.0";
    src = ../../network-status;
    cargoLock.lockFile = ../../network-status/Cargo.lock;
    doCheck = false;
  };
in
{
  systemd.services.network-status = {
    description = "Router network status dashboard (network-info.int.leighhack.org)";
    wantedBy = [ "multi-user.target" ];
    after = [ "network.target" ];
    # Runs as leigh-admin so it can read the machine-hop key directly.
    serviceConfig = {
      Type = "simple";
      User = "leigh-admin";
      # ssh client for probing the router (note: systemd 260 removed the
      # Path= unit option, so PATH is set explicitly).
      Environment = [
        "HOME=/home/leigh-admin"
        "PATH=${pkgs.openssh}/bin"
      ];
      ExecStart = lib.concatStringsSep " " [
        "${networkStatus}/bin/network-status"
        "--bind" "127.0.0.1"
        "--port" "8091"
        "--static-dir" "${frontend}/share/network-status/frontend"
        "--router" "root@10.3.1.1"
        "--ssh-key" "/home/leigh-admin/.ssh/agent-hop-key"
        "--interval" "5"
        "--wan" "em0"
        "--title"
        # Quoted: the em dash + spaces would otherwise split the argv.
        "\"Network — router\""
      ];
      Restart = "always";
      RestartSec = "5s";
    };
    # Never give up.
    startLimitIntervalSec = 0;
  };
}
