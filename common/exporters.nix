# Metrics exporters: the per-box half of the monitoring setup.
#
# Every machine runs these; Prometheus and Grafana run on services2 only (see
# machines/services2/monitoring.nix).  The split matters: the box that decides
# "is this thing down" must not be the box being asked, otherwise a dead services1
# takes the alerting with it (see machines/services2/gatus.nix).
#
# Exporters bind 0.0.0.0 by default, which is what makes cross-machine scraping
# possible — the scrape targets in machines/services2/monitoring.nix are fixed IPs
# from common/hosts.nix, so monitoring does not depend on the resolver.
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
