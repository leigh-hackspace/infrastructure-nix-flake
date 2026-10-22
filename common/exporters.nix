# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
# Metrics exporters: the per-box half of the monitoring setup.
#
# Every machine runs these; Prometheus and Grafana run on services1 (see
# machines/services1/services/observability.nix).  The exporters stay on every
# box so each one is still measured from the box that scrapes it: services1's
# Prometheus scrapes all three by fixed IP out of common/hosts.nix, which is why
# they bind 0.0.0.0 — a scrape from another box cannot reach a loopback bind.
#
# What the move costs: since 2026-10-17 the scraper and services1's own exporters
# live on the same box, so a dead services1 stops reporting itself (see
# machines/services1/services/gatus.nix for the trade-off).
#
# Exporters bind 0.0.0.0 by default, which is what makes cross-machine scraping
# possible — the scrape targets in machines/services1/services/observability.nix
# are fixed IPs from common/hosts.nix, so monitoring does not depend on the
# resolver.
#
# Ports: 9100 (node), 9558 (systemd), 9633 (smartctl).
{
  services.prometheus.exporters = {
    node = {
      enable = true;
      port = 9100;
      # The dashboards show unit state, so the systemd collector is on.
      enabledCollectors = ["systemd"];
    };
    systemd = {
      enable = true;
      port = 9558;
    };
    smartctl = {
      enable = true;
      port = 9633;
    };
  };
}
