# Prometheus exporter for the hackspace 3D-print servers.
#
# 3d-blue (10.3.14.62) and 3d-lime (10.3.14.61) are Raspberry Pi 3 print
# servers running Klipper + Moonraker. Moonraker exposes no /metrics
# endpoint, so the `moonraker-exporter` tool (Rust source:
# ../../../moonraker-exporter, zero external crates) polls each printer's
# Moonraker API on :7125 and re-exports the metrics for the local Prometheus
# (scrape job "moonraker", configured in monitoring.nix) to scrape.
#
# Targets are fixed IPs on purpose so the exporter never depends on DNS.
# Historically this was required because the two Pis were cloned from one SD
# card, so they shared a /etc/machine-id and therefore a NetworkManager
# DHCPv6 DUID, and the 3d-* records advertised each other's IPv6 addresses.
# Lime's machine-id and SSH host keys were regenerated on 2026-10-03 (blue
# still carries the old identity); fixed IPv4 is deliberately kept.
{
  pkgs,
  lib,
  CRANE,
  INFRA,
  ...
}: let
  exporter = CRANE.cached {
    pname = "moonraker-exporter";
    version = "0.1.0";
    src = ../../../moonraker-exporter;
    cargoLock = CRANE.lockFile ../../../moonraker-exporter/Cargo.lock;
    # `common-json` path dependency (see common-rs/ and common/crane.nix).
    sharedCrates = ["json"];
  };

  printers = [
    {
      name = "blue";
      url = "http://10.3.14.62:7125";
    }
    {
      name = "lime";
      url = "http://10.3.14.61:7125";
    }
  ];

  printerArgs = lib.concatStringsSep " " (
    map (p: "--printer ${p.name}=${p.url}") printers
  );
in {
  systemd.services.moonraker-exporter = INFRA.mkNeverGiveUp {
    description = "Prometheus exporter for the 3D-print servers' Moonraker APIs";
    wantedBy = ["multi-user.target"];
    after = ["network-online.target"];
    wants = ["network-online.target"];

    serviceConfig = {
      ExecStart = "${exporter}/bin/moonraker-exporter --listen 127.0.0.1:9701 ${printerArgs}";
      DynamicUser = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      NoNewPrivileges = true;
    };
  };
}
