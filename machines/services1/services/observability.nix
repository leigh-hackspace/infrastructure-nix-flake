# Prometheus + Grafana — the observability half of the network, on services1.
#
# They used to live on services2, and the reason given for that was that the box
# answering "is this thing down" must not be the box being asked.  That reason
# stopped holding on 2026-10-17: services2 has 30 GB of RAM and strata's Coder
# engine runs at ~28 GiB RSS against it, so anything else living there is what
# takes the monitor down under memory pressure (see machines/services2/README.md
# and machines/services2/STRATA.md).  services1 has the headroom and already
# terminates TLS for these two vhosts, so the apps moved to it and the vhosts
# stayed where they were.
#
# What that costs is the one check that used to be worth having: gatus and
# Prometheus now run on the box they watch, so a dead services1 goes down with
# its own monitor and reports nothing.  What still works from here is everything
# that measures the *other* boxes — the exporters, the scrape of services2 and
# aibox, and gatus's ping of 10.3.1.21 — and services2's own status dashboard,
# which is the thing that keeps running while strata is thrashing.
#
# Dashboards are declarative: see ./monitoring-dashboards.nix; edit there and
# `just switch` on services1.
#
#   https://grafana.int.leighhack.org      (anonymous view; admin via sops)
#   https://prometheus.int.leighhack.org   (LAN/tailnet only)
#
# Both vhosts are on this box now (machines/services1/services/monitoring.nix),
# which is also where the ACME cert and nginx-sso live, so they proxy to a
# loopback bind rather than across the LAN.  The exporters stay on every machine
# (common/exporters.nix): this file is only the scraper, the rules and the
# dashboards.
#
# Ports: 9091 for Prometheus (9090 is taken by cockpit on this box), 3000 for
# Grafana, exporters on 9100 (node), 9558 (systemd), 9633 (smartctl).  None of
# the three collide with anything else on services1 — checked against the
# rendered units before the move.
{
  config,
  lib,
  pkgs,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  HOSTS = import ../../../common/hosts.nix;

  # Printer state enums (klippy / print_stats) come from the exporter's own
  # arrays, so the alert numbers below are the exporter's, not a copy.
  PRINTER = import ../../../common/printer-states.nix;

  PROM_PORT = 9091;

  dashboards = import ./monitoring-dashboards.nix {inherit lib;};

  # One JSON file per dashboard, provisioned into Grafana at boot.
  dashboardsDir = let
    files =
      lib.mapAttrs (
        uid: dashboard: pkgs.writeText "${uid}.json" (builtins.toJSON dashboard)
      )
      dashboards;
  in
    pkgs.runCommand "grafana-dashboards" {} ''
      mkdir -p $out
      ${lib.concatStrings (lib.attrValues (lib.mapAttrs (uid: f: "cp ${f} $out/${uid}.json\n") files))}
    '';

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
              {
                alert = "RouterUnreachable";
                expr = "router_up == 0";
                for = "2m";
                labels.severity = "critical";
                annotations.summary = "Router (10.3.1.1) unreachable from network-status";
              }
              {
                alert = "PrinterMoonrakerDown";
                expr = "moonraker_up == 0";
                for = "5m";
                labels.severity = "warning";
                annotations.summary = "Moonraker unreachable on {{ $labels.printer }}";
                annotations.description = "No response from the Moonraker API on {{ $labels.printer }} for 5 minutes";
              }
              {
                alert = "KlipperError";
                expr = "moonraker_klippy_state{state=\"error\"} == ${builtins.toString PRINTER.klippy.code.error}";
                for = "10m";
                labels.severity = "warning";
                annotations.summary = "Klipper is in error state on {{ $labels.printer }}";
              }
              {
                alert = "KlipperShutdown";
                expr = "moonraker_klippy_state{state=\"shutdown\"} == ${builtins.toString PRINTER.klippy.code.shutdown}";
                for = "5m";
                labels.severity = "critical";
                annotations.summary = "Klipper is in shutdown state on {{ $labels.printer }}";
                annotations.description = "A print was aborted (e.g. MCU communication lost). Check the Klipper log on {{ $labels.printer }}.";
              }
              {
                alert = "PrinterMcuDisconnected";
                expr = "moonraker_mcu_connected == 0";
                for = "5m";
                labels.severity = "warning";
                annotations.summary = "Klipper is not attached to the MCU on {{ $labels.printer }}";
                annotations.description = "No MCU stats for 5 minutes - the CH340/USB-serial link or the printer's power is down.";
              }
              {
                alert = "PrinterMcuRetransmits";
                expr = "rate(moonraker_mcu_bytes_retransmit_total[10m]) > 1";
                for = "10m";
                labels.severity = "warning";
                annotations.summary = "Klipper serial retransmits are high on {{ $labels.printer }}";
                annotations.description = "The USB-serial link to the MCU is erroring ({{ $value | humanize }} B/s retransmitted); a print may be aborted soon.";
              }
              {
                alert = "PrinterMcuInvalidBytes";
                expr = "rate(moonraker_mcu_bytes_invalid_total[5m]) > 1";
                for = "5m";
                labels.severity = "warning";
                annotations.summary = "Invalid bytes on the Klipper MCU link on {{ $labels.printer }}";
              }
            ];
          }
        ];
      }
    );
in {
  users.groups.secrets.members = ["grafana"];

  # --- Prometheus -----------------------------------------------------
  services.prometheus = {
    enable = true;
    port = PROM_PORT;
    retentionTime = "30d";

    scrapeConfigs = [
      {
        job_name = "prometheus";
        static_configs = [{targets = ["127.0.0.1:${toString PROM_PORT}"];}];
      }
      # Every machine, by fixed IP (common/hosts.nix). The exporters bind 0.0.0.0,
      # so this box scrapes itself the same way it scrapes the others — there is no
      # "127.0.0.1 = this machine" special case any more, which is what keeps the
      # instance labels comparable across the three.
      {
        job_name = "node";
        static_configs = [
          {targets = ["${HOSTS.services1}:9100"];}
          {targets = ["${HOSTS.services2}:9100"];}
          {targets = ["${HOSTS.aibox}:9100"];}
        ];
      }
      {
        job_name = "systemd";
        static_configs = [
          {targets = ["${HOSTS.services1}:9558"];}
          {targets = ["${HOSTS.services2}:9558"];}
          {targets = ["${HOSTS.aibox}:9558"];}
        ];
      }
      {
        job_name = "smartctl";
        static_configs = [
          {targets = ["${HOSTS.services1}:9633"];}
          {targets = ["${HOSTS.services2}:9633"];}
          {targets = ["${HOSTS.aibox}:9633"];}
        ];
        # SMART queries can be slow on large/degraded disks.
        scrape_interval = "5m";
        scrape_timeout = "30s";
      }
      # OPNsense router (10.3.1.1), re-exported by the network-status service's
      # /metrics endpoint on this box — the same data the
      # network-info.int.leighhack.org SPA shows.  network-status binds 0.0.0.0
      # rather than loopback (see machines/services1/network-status.nix); that
      # used to be required by a scrape from services2 and is now only a habit,
      # since Prometheus and network-status share this box.
      {
        job_name = "router";
        metrics_path = "/metrics";
        scrape_interval = "10s";
        static_configs = [{targets = ["${HOSTS.services1}:8091"];}];
      }
      # Klipper/Moonraker on the hackspace 3D-print servers (blue/lime),
      # re-exported by moonraker-exporter (printer-monitoring.nix). Targets
      # are fixed IPs so the exporter never depends on the 3d-* DNS records.
      {
        job_name = "moonraker";
        scrape_interval = "30s";
        scrape_timeout = "20s";
        static_configs = [{targets = ["${HOSTS.services1}:9701"];}];
      }
    ];

    ruleFiles = [rulesFile];
  };

  # The exporters themselves are in common/exporters.nix (run on every machine,
  # including this one).

  # --- Grafana ---------------------------------------------------------
  services.grafana = {
    enable = true;
    settings = {
      server = {
        http_port = 3000;
        # Loopback is enough now: the vhost is on this box (../monitoring.nix), so
        # nginx proxies here rather than across the LAN.  It was 0.0.0.0 while
        # Grafana lived on services2, because the proxy came from services1.
        http_addr = "127.0.0.1";
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
      # Sign-in via authentik (id.leighhack.org). Anonymous viewing above
      # stays enabled; on login, members of the authentik "Infra" group get
      # Grafana Admin, everyone else gets Viewer.
      "auth.generic_oauth" = {
        enabled = true;
        name = "Authentik";
        client_id = "8TMM2mYHBV2YQCovNbgGKbuEp0LJcxo2TjpqNN9s";
        client_secret = "$__file{${CONFIG.GRAFANA_OIDC_CLIENT_SECRET_FILE}}";
        scopes = "openid profile email groups";
        auth_url = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/authorize/";
        token_url = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/token/";
        api_url = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/userinfo/";
        use_pkce = true;
        use_refresh_token = true;
        allow_sign_up = true;
        login_attribute_path = "preferred_username";
        role_attribute_path = "contains(groups[*], 'Infra') && 'GrafanaAdmin' || 'Viewer'";
        allow_assign_grafana_admin = true;
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
          # Not "Services1" any more: the dashboards cover the whole network, and
          # the box serving them is not the box being measured.
          name = "Infrastructure";
          type = "file";
          disableDeletion = false;
          updateExistingItems = true;
          foldersFromFiles = false;
          options.path = "${dashboardsDir}";
        }
      ];
    };
  };
}
