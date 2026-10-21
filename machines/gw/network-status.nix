# network-status: the router's own network dashboard, hosted on gw.
#
# This is gw's /proc and /sys rendered as charts: per-interface bandwidth, the
# nftables conntrack table, CPU/memory/load and "potential issues", plus the
# same data as Prometheus metrics.  The crate is `crates/network-status` (zero
# external crates); the SPA in its `frontend/` is prebuilt and its `dist/` is
# committed (see `crates/network-status/frontend/README.md`).
#
# It used to run on services1 and ssh to gw as `root`, first talking to the
# OPNsense box and then briefly to gw.  Hosting it here is the last step of that
# port: the probe is `--local`, so this unit is a DynamicUser reading
# world-readable kernel files — no login, no ssh key, no host-key pin.  It binds
# the LAN address only (`10.3.1.1:8091`); `./firewall.nix` lets services1 reach
# it, and services1's nginx is what serves `network-info.int.leighhack.org`
# (TLS and the ACL stay there) while Prometheus on services1 scrapes `/metrics`.
#
# Consequence for deploying this box: its system now contains a Rust crate, so
# it must be built elsewhere.  `just switch-gw` builds on services1/aibox and
# pushes; do not build it here (AGENTS.md, "Never build on the router").
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  # Prebuilt SolidJS/TypeScript SPA.  dist/ is committed to the git tree (Nix
  # builds run offline and the npm deps are not nixpkgs-cached), so this only
  # copies it into the store.  Rebuild with `just network-status-frontend`.
  frontend = pkgs.stdenv.mkDerivation {
    pname = "network-status-frontend";
    version = "0.1.0";
    src = ../../crates/network-status/frontend;
    dontConfigure = true;
    dontBuild = true;
    installPhase = ''
      mkdir -p $out/share/network-status
      cp -r dist/. $out/share/network-status/frontend
    '';
  };

  networkStatus = CRANE.crate {
    pname = "network-status";
    version = "0.1.0";
    member = "network-status";
    sharedCrates = ["json" "web"];
  };
in {
  systemd.services.network-status = INFRA.mkNeverGiveUp {
    description = "Router network status dashboard (network-info.int.leighhack.org)";
    wantedBy = ["multi-user.target"];
    # Bind the LAN address rather than 0.0.0.0: the only clients are the nginx
    # proxy and Prometheus on services1, so there is no reason to listen on
    # ppp0 or tailscale0 too.  The address comes from networkd, hence the
    # online wait; the never-give-up policy covers a lost race anyway.
    after = ["network.target" "network-online.target"];
    wants = ["network-online.target"];
    # The probe is POSIX sh; the commands it pipes through must be on the unit's
    # PATH rather than inherited from whatever the operator's shell had.
    path = [
      pkgs.bash
      pkgs.coreutils
      pkgs.gnugrep
      pkgs.gawk
    ];

    serviceConfig = {
      Type = "simple";
      # A router dashboard has no business owning a login, and every file the
      # probe reads is world-readable.
      DynamicUser = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      NoNewPrivileges = true;
      ExecStart = lib.concatStringsSep " " [
        "${networkStatus}/bin/network-status"
        "--bind"
        "10.3.1.1"
        "--port"
        "8091"
        "--static-dir"
        "${frontend}/share/network-status/frontend"
        "--local"
        "--interval"
        "5"
        "--wan"
        # The AAISP L2TP tunnel (./constants.nix `UPLINK_TUNNEL`), not the hub
        # handoff NIC: ppp0 only exists while the tunnel is up, which is what
        # makes its disappearance the WAN-down signal.
        "ppp0"
        "--title"
        # Quoted: the em dash + spaces would otherwise split the argv.
        "\"Network — router\""
      ];
    };
  };
}
