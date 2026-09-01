# Metrics exporters scraped by Prometheus on services1 (10.3.1.20, port
# 9091). aibox has no firewall, so the default 0.0.0.0 bind is fine.
{
  services.prometheus.exporters = {
    node = {
      enable = true;
      port = 9100;
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
