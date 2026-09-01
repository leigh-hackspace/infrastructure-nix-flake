{
  config,
  lib,
  pkgs,
  ...
}:

# Prometheus + Grafana monitoring server for this machine (for now — the
# scrape configs are structured so other hosts can be added later, e.g.
# aibox at 10.3.1.32). Dashboards are declarative: see
# monitoring-dashboards.nix; edit there and `just switch`.
#
#   https://grafana.int.leighhack.org      (anonymous view; admin via sops)
#   https://prometheus.int.leighhack.org   (LAN/tailnet only)
#
# Ports: 9091 for Prometheus (9090 is taken by cockpit), 3000 for Grafana,
# exporters on 9100 (node), 9558 (systemd), 9633 (smartctl).

let
  CONFIG = import ../config.nix;

  PROM_PORT = 9091;

  dashboards = import ./monitoring-dashboards.nix { inherit lib; };

  # One JSON file per dashboard, provisioned into Grafana at boot.
  dashboardsDir =
    let
      files = lib.mapAttrs (
        uid: dashboard: pkgs.writeText "${uid}.json" (builtins.toJSON dashboard)
      ) dashboards;
    in
    pkgs.runCommand "grafana-dashboards" { } (
      ''
        mkdir -p $out
        ${lib.concatStrings (lib.attrValues (lib.mapAttrs (uid: f: "cp ${f} $out/${uid}.json\n") files))}
      ''
    );

  # Alert rules. No alertmanager is wired up yet (gatus already pings the
  # HTTP endpoints); these light up the Prometheus "Alerts" tab and are
  # ready for alertmanager/slack later.
  rulesFile =
    pkgs.writeText "prometheus-rules.yaml"
    (
      builtins.toJSON
      {
        groups = [
          {
            name = "system_health";
            interval = "30s";
            rules = [
              {
                alert = "HostDown";
                expr = "up{job=\"node\"} == 0";
                for = "2m";
                labels.severity = "critical";
                annotations.summary = "Node exporter on {{ $labels.instance }} is down";
                annotations.description = "No metrics from {{ $labels.instance }} for 2 minutes";
              }
              {
                alert = "ExporterDown";
                expr = "up{job!=\"node\"} == 0";
                for = "5m";
                labels.severity = "warning";
                annotations.summary = "{{ $labels.job }} exporter on {{ $labels.instance }} is unreachable";
              }
              {
                alert = "HighCPU";
                expr = "100 - (avg by (instance) (irate(node_cpu_seconds_total{mode=\"idle\"}[5m])) * 100) > 80";
                for = "10m";
                labels.severity = "warning";
                annotations.summary = "High CPU usage on {{ $labels.instance }}";
              }
              {
                alert = "LowMemory";
                expr = "(node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes) * 100 < 10";
                for = "5m";
                labels.severity = "warning";
                annotations.summary = "Low available memory on {{ $labels.instance }}";
              }
              {
                alert = "DiskSpaceLow";
                expr = "(node_filesystem_avail_bytes{fstype!~\"tmpfs|overlay|squashfs\"} / node_filesystem_size_bytes{fstype!~\"tmpfs|overlay|squashfs\"}) * 100 < 10";
                for = "5m";
                labels.severity = "critical";
                annotations.summary = "Disk space low on {{ $labels.instance }}";
                annotations.description = "Only {{ $value | humanize }}% free on {{ $labels.mountpoint }}";
              }
              {
                alert = "HighLoad";
                expr = "node_load5 > (count by (instance) (node_cpu_seconds_total{mode=\"idle\"}) * 2)";
                for = "10m";
                labels.severity = "warning";
                annotations.summary = "High 5m load on {{ $labels.instance }}";
              }
              {
                alert = "ServiceFailed";
                expr = "systemd_unit_state{state=\"failed\"} == 1";
                for = "5m";
                labels.severity = "critical";
                annotations.summary = "Unit {{ $labels.name }} is failed on {{ $labels.instance }}";
              }
            ];
          }
        ];
      }
    );
in
{
  # --- Prometheus -----------------------------------------------------
  services.prometheus = {
    enable = true;
    port = PROM_PORT;
    retentionTime = "30d";

    scrapeConfigs = [
      {
        job_name = "prometheus";
        static_configs = [ { targets = [ "127.0.0.1:${toString PROM_PORT}" ]; } ];
      }
      # services1 itself for now; add more targets here (e.g.
      # "10.3.1.32:9100") as other machines get node exporters.
      {
        job_name = "node";
        static_configs = [ { targets = [ "127.0.0.1:9100" ]; } ];
      }
      {
        job_name = "systemd";
        static_configs = [ { targets = [ "127.0.0.1:9558" ]; } ];
      }
      {
        job_name = "smartctl";
        static_configs = [ { targets = [ "127.0.0.1:9633" ]; } ];
        # SMART queries can be slow on large/degraded disks.
        scrape_interval = "5m";
        scrape_timeout = "30s";
      }
    ];

    ruleFiles = [ rulesFile ];
  };

  services.prometheus.exporters = {
    node = {
      enable = true;
      port = 9100;
      enabledCollectors = [ "systemd" ];
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

  # --- Grafana ---------------------------------------------------------
  services.grafana = {
    enable = true;
    settings = {
      server = {
        http_port = 3000;
        root_url = "https://grafana.int.leighhack.org";
      };
      security = {
        admin_user = "admin";
        admin_password = lib.strings.trim (
          builtins.readFile (config.sopsSecretText "grafana_admin_password")
        );
        # Required since NixOS 26.05 (no default value anymore).
        secret_key = lib.strings.trim (
          builtins.readFile (config.sopsSecretText "grafana_secret_key")
        );
      };
      # "Publicly viewable": anyone who can reach the vhost (LAN/tailnet,
      # see LOCAL_NETWORK ACL below) can view dashboards without logging
      # in; the admin account above is for making changes.
      # Grafana section is [auth.anonymous]; the dot is part of the key.
      "auth.anonymous" = {
        enabled = true;
        org_name = "Main Org.";
        org_role = "Viewer";
      };
    };

    provision = {
      enable = true;
      datasources.settings.datasources = [
        {
          name = "Prometheus";
          uid = "prometheus";
          type = "prometheus";
          access = "proxy";
          url = "http://127.0.0.1:${toString PROM_PORT}";
          isDefault = true;
          editable = true;
        }
      ];
      dashboards.settings.providers = [
        {
          name = "Services1";
          type = "file";
          disableDeletion = false;
          updateExistingItems = true;
          foldersFromFiles = false;
          options.path = "${dashboardsDir}";
        }
      ];
    };
  };

  # --- nginx -----------------------------------------------------------
  services.nginx.virtualHosts = {
    "prometheus.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://127.0.0.1:${toString PROM_PORT}";
        recommendedProxySettings = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };

    # Replaces the old grafana.int vhost in http.nix which proxied to the
    # (now superseded) Grafana on 10.3.1.30.
    "grafana.int.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://127.0.0.1:3000";
        recommendedProxySettings = true;
        proxyWebsockets = true;
        extraConfig = CONFIG.LOCAL_NETWORK;
      };
    };
  };
}
