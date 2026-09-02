# Prometheus exporter for the hackspace 3D-print servers.
#
# 3d-blue (10.3.14.62) and 3d-lime (10.3.14.61) are Raspberry Pi 3 print
# servers running Klipper + Moonraker. Moonraker exposes no /metrics
# endpoint, so the `moonraker-exporter` tool (Rust source:
# ../../../moonraker-exporter, zero external crates) polls each printer's
# Moonraker API on :7125 and re-exports the metrics for the local Prometheus
# (scrape job "moonraker", configured in monitoring.nix) to scrape.
#
# Targets are fixed IPs on purpose: the 3d-lime.int.leighhack.org DNS name
# also advertises 3d-blue's IPv6 addresses, so hostnames must not be used.
{
  pkgs,
  lib,
  ...
}:

let
  exporter = pkgs.rustPlatform.buildRustPackage {
    pname = "moonraker-exporter";
    version = "0.1.0";
    src = ../../../moonraker-exporter;
    cargoLock.lockFile = ../../../moonraker-exporter/Cargo.lock;
    doCheck = false;
  };

  printers = [
    { name = "blue"; url = "http://10.3.14.62:7125"; }
    { name = "lime"; url = "http://10.3.14.61:7125"; }
  ];

  printerArgs = lib.concatStringsSep " " (
    map (p: "--printer ${p.name}=${p.url}") printers
  );
in
{
  systemd.services.moonraker-exporter = {
    description = "Prometheus exporter for the 3D-print servers' Moonraker APIs";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];

    serviceConfig = {
      ExecStart = "${exporter}/bin/moonraker-exporter --listen 127.0.0.1:9701 ${printerArgs}";
      Restart = "always";
      RestartSec = 5;
      DynamicUser = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      NoNewPrivileges = true;
    };
  };
}
