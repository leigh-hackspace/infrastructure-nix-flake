{ lib }:

# Declarative Grafana dashboards for the services1 monitoring stack.
#
# Each top-level attribute is one dashboard (attribute name = dashboard
# uid). The whole set is rendered to JSON and provisioned into Grafana by
# monitoring.nix on every build — edit here, `just switch`, done.
#
# Panel helpers below keep the individual dashboards compact; everything
# else is plain Grafana dashboard JSON.

let
  DS = { type = "prometheus"; uid = "prometheus"; };

  # Timeseries panel in the Prometheus datasource.
  ts =
    { id, x, y, w ? 12, h ? 8, title, unit ? "short", legendFormat ? "auto", targets }:
    {
      inherit id x y w h title;
      type = "timeseries";
      datasource = DS;
      gridPos = { inherit x y w h; };
      # Targets are expressions, or { expr, legendFormat } pairs for
      # panels where each line needs its own legend.
      targets = lib.map (
        t:
        if builtins.isAttrs t then
          t // { range = true; }
        else
          { expr = t; legendFormat = legendFormat; range = true; }
      ) targets;
      fieldConfig = {
        defaults = {
          inherit unit;
          custom = {
            drawStyle = "line";
            lineWidth = 2;
            pointSize = 5;
            spanNulls = true;
            fillOpacity = 10;
          };
        };
        overrides = [ ];
      };
      options = {
        legend = { displayMode = "list"; placement = "bottom"; calcs = [ ]; };
        tooltip = { mode = "multi"; sort = "none"; };
      };
    };

  # Single-value stat panel.
  st =
    { id, x, y, w ? 4, h ? 4, title, unit ? "short", thresholds, targets, legendFormat ? "auto" }:
    {
      inherit id x y w h title;
      type = "stat";
      datasource = DS;
      gridPos = { inherit x y w h; };
      targets = lib.map (
        expr:
        {
          inherit expr;
          legendFormat = legendFormat;
          instant = true;
        }
      ) targets;
      fieldConfig = {
        defaults = {
          inherit unit;
          thresholds = { mode = "absolute"; steps = thresholds; };
        };
        overrides = [ ];
      };
      options = {
        colorMode = "background";
        graphMode = "area";
        reduceOptions = { calcs = [ "lastNotNull" ]; };
      };
    };

  green = "green";
  orange = "orange";
  red = "red";

  # Stat tile that renders a numeric state code as text. `mapping` is
  # { code = "label" } — keep the tables in sync with the exporter
  # (moonraker-exporter/src/main.rs).
  stateStat =
    { id, x, y, w ? 4, h ? 4, title, mapping, target, thresholds ? [ { color = green; value = null; } ] }:
    let
      options = lib.listToAttrs (
        lib.imap0 (i: code: {
          name = code;
          # `index` selects the threshold step that colours this mapping
          # (Grafana value-mapping schema); with the default single-step
          # thresholds every state lands on step 0.
          value = { index = i; text = mapping.${code}; };
        }) (lib.attrNames mapping)
      );
    in
    {
      inherit id x y w h title;
      type = "stat";
      datasource = DS;
      gridPos = { inherit x y w h; };
      targets = [
        { inherit target; instant = true; legendFormat = "auto"; }
      ];
      fieldConfig.defaults = {
        mappings = [ { type = "value"; inherit options; } ];
        thresholds = {
          mode = "absolute";
          steps = thresholds;
        };
      };
      options = {
        colorMode = "background";
        graphMode = "area";
        reduceOptions = { calcs = [ "lastNotNull" ]; };
        textMode = "value";
      };
    };
in
{
  # ------------------------------------------------------------------
  # CPU, RAM, disk, network, TCP — the essential host vitals.
  # ------------------------------------------------------------------
  "services1-system" = {
    uid = "services1-system";
    title = "services1 · System";
    time = { from = "now-6h"; to = "now"; };
    tags = [ "services1" "node-exporter" ];
    timezone = "browser";
    schemaVersion = 39;
    version = 1;
    refresh = "1m";
    panels = [
      # --- stat row -------------------------------------------------
      (st {
        id = 1;
        x = 0;
        y = 0;
        title = "CPU usage";
        unit = "percent";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 70; }
          { color = red; value = 90; }
        ];
        targets = [
          "100 * (1 - avg(rate(node_cpu_seconds_total{mode=\"idle\"}[5m])))"
        ];
      })
      (st {
        id = 2;
        x = 4;
        y = 0;
        title = "RAM available";
        unit = "percent";
        thresholds = [
          { color = red; value = null; }
          { color = orange; value = 10; }
          { color = green; value = 20; }
        ];
        targets = [
          "(node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes) * 100"
        ];
      })
      (st {
        id = 3;
        x = 8;
        y = 0;
        title = "Root disk free";
        unit = "percent";
        thresholds = [
          { color = red; value = null; }
          { color = orange; value = 10; }
          { color = green; value = 20; }
        ];
        targets = [
          "(node_filesystem_avail_bytes{mountpoint=\"/\"} / node_filesystem_size_bytes{mountpoint=\"/\"}) * 100"
        ];
      })
      (st {
        id = 4;
        x = 12;
        y = 0;
        title = "Load (5m)";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 8; }
          { color = red; value = 16; }
        ];
        targets = [ "node_load5" ];
      })
      (st {
        id = 5;
        x = 16;
        y = 0;
        title = "Uptime";
        unit = "s";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "time() - node_boot_time_seconds" ];
      })
      (st {
        id = 6;
        x = 20;
        y = 0;
        title = "Swap used";
        unit = "percent";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 50; }
          { color = red; value = 80; }
        ];
        targets = [
          "100 * (1 - (node_memory_SwapFree_bytes / node_memory_SwapTotal_bytes))"
        ];
      })

      # --- CPU / memory ---------------------------------------------
      (ts {
        id = 7;
        x = 0;
        y = 4;
        title = "CPU usage by mode";
        unit = "percent";
        legendFormat = "{{mode}}";
        targets = [
          "100 * sum by (mode) (rate(node_cpu_seconds_total{mode!=\"idle\"}[5m]))"
        ];
      })
      (ts {
        id = 8;
        x = 12;
        y = 4;
        title = "Memory";
        unit = "bytes";
        targets = [
          { expr = "node_memory_MemTotal_bytes - node_memory_MemAvailable_bytes"; legendFormat = "used"; }
          { expr = "node_memory_Cached_bytes"; legendFormat = "cached"; }
          { expr = "node_memory_MemAvailable_bytes"; legendFormat = "available"; }
        ];
      })

      # --- network / disk -------------------------------------------
      (ts {
        id = 9;
        x = 0;
        y = 12;
        title = "Network throughput";
        unit = "Bps";
        legendFormat = "{{device}} {{direction}}";
        targets = [
          # Physical/bridge interfaces only; container veths and tunnels
          # are noise on a services box. The constant "direction" label
          # feeds the legend.
          "sum by (device, direction) (label_replace(rate(node_network_receive_bytes_total{device!~\"^(lo|veth.*|tap.*|br-.*|cni.*|podman.*|nerdctl.*|lxd.*|containerd.*|flannel.*|cali.*|kube-.*|dummy-.*|tailscale0|virbr.*|docker-.*|wg.*)\"}[5m]), \"direction\", \"rx\", \"\", \"\"))"
          "sum by (device, direction) (label_replace(rate(node_network_transmit_bytes_total{device!~\"^(lo|veth.*|tap.*|br-.*|cni.*|podman.*|nerdctl.*|lxd.*|containerd.*|flannel.*|cali.*|kube-.*|dummy-.*|tailscale0|virbr.*|docker-.*|wg.*)\"}[5m]), \"direction\", \"tx\", \"\", \"\"))"
        ];
      })
      (ts {
        id = 10;
        x = 12;
        y = 12;
        title = "Disk I/O";
        unit = "Bps";
        legendFormat = "{{device}} {{direction}}";
        targets = [
          "sum by (device, direction) (label_replace(rate(node_disk_read_bytes_total{device!~\"^(loop.*|ram.*|zram.*|sr.*|fd.*|dm-.*|md.*)\"}[5m]), \"direction\", \"read\", \"\", \"\"))"
          "sum by (device, direction) (label_replace(rate(node_disk_write_bytes_total{device!~\"^(loop.*|ram.*|zram.*|sr.*|fd.*|dm-.*|md.*)\"}[5m]), \"direction\", \"write\", \"\", \"\"))"
        ];
      })

      # --- filesystems / TCP ------------------------------------------
      (ts {
        id = 11;
        x = 0;
        y = 20;
        title = "Filesystem usage";
        unit = "percent";
        legendFormat = "{{mountpoint}}";
        targets = [
          "(1 - (node_filesystem_avail_bytes{fstype!~\"tmpfs|overlay|squashfs|efivarfs|fuse.*\"} / node_filesystem_size_bytes{fstype!~\"tmpfs|overlay|squashfs|efivarfs|fuse.*\"})) * 100"
        ];
      })
      (ts {
        id = 12;
        x = 12;
        y = 20;
        title = "TCP connections";
        legendFormat = "{{state}}";
        targets = [
          "sum by (state) (node_tcp_connection_states{state=~\"ESTABLISHED|LISTEN|TIME_WAIT|CLOSE_WAIT\"})"
        ];
      })
    ];
  };

  # ------------------------------------------------------------------
  # systemd unit health (systemd exporter) — failed/activating units,
  # i.e. the "essential system services" view.
  # ------------------------------------------------------------------
  "services1-services" = {
    uid = "services1-services";
    title = "services1 · Services";
    tags = [ "services1" "systemd" ];
    timezone = "browser";
    schemaVersion = 39;
    version = 1;
    refresh = "1m";
    time = { from = "now-6h"; to = "now"; };
    panels = [
      (st {
        id = 1;
        x = 0;
        y = 0;
        w = 6;
        title = "Failed units";
        thresholds = [
          { color = green; value = null; }
          { color = red; value = 1; }
        ];
        targets = [ "sum(systemd_unit_state{state=\"failed\"})" ];
      })
      (st {
        id = 2;
        x = 6;
        y = 0;
        w = 6;
        title = "Active units";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "sum(systemd_unit_state{state=\"active\"})" ];
      })
      (st {
        id = 3;
        x = 12;
        y = 0;
        w = 6;
        title = "Activating units";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 5; }
        ];
        targets = [ "sum(systemd_unit_state{state=\"activating\"})" ];
      })
      (st {
        id = 4;
        x = 18;
        y = 0;
        w = 6;
        title = "Dead units";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "sum(systemd_unit_state{state=~\"dead|inactive\"})" ];
      })
      (ts {
        id = 5;
        x = 0;
        y = 4;
        title = "Units by state";
        legendFormat = "{{state}}";
        targets = [ "sum by (state) (systemd_unit_state)" ];
      })
      (ts {
        id = 6;
        x = 12;
        y = 4;
        title = "Failed units";
        legendFormat = "{{name}}";
        targets = [
          "sum by (name) (systemd_unit_state{state=\"failed\"})"
        ];
      })
    ];
  };

  # ------------------------------------------------------------------
  # 3D-print servers (3d-blue/3d-lime, Klipper + Moonraker on Raspberry
  # Pi 3s). Data comes from the moonraker-exporter (scrape job
  # "moonraker"); the two printers are told apart by the `printer` label.
  # ------------------------------------------------------------------
  "3d-printers" = {
    uid = "3d-printers";
    title = "3D Printers";
    tags = [ "printers" "moonraker" "klipper" ];
    timezone = "browser";
    schemaVersion = 39;
    version = 1;
    refresh = "30s";
    time = { from = "now-6h"; to = "now"; };
    panels = [
      # --- blue: state row -------------------------------------------
      (stateStat {
        id = 1;
        x = 0;
        y = 0;
        title = "blue · Klipper";
        mapping = {
          "0" = "startup";
          "1" = "ready";
          "2" = "error";
          "3" = "shutdown";
          "4" = "disconnected";
        };
        target = "moonraker_klippy_state{printer=\"blue\"}";
      })
      (stateStat {
        id = 2;
        x = 4;
        y = 0;
        title = "blue · Job";
        mapping = {
          "0" = "standby";
          "1" = "printing";
          "2" = "paused";
          "3" = "complete";
          "4" = "cancelled";
          "5" = "error";
        };
        target = "moonraker_print_state{printer=\"blue\"}";
      })
      (st {
        id = 3;
        x = 8;
        y = 0;
        title = "blue · Progress";
        unit = "percent";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 50; }
          { color = red; value = 90; }
        ];
        targets = [ "moonraker_print_progress{printer=\"blue\"} * 100" ];
      })
      (st {
        id = 4;
        x = 12;
        y = 0;
        title = "blue · Nozzle";
        unit = "celsius";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "moonraker_heater_temperature{printer=\"blue\",heater=\"extruder\"}" ];
      })
      (st {
        id = 5;
        x = 16;
        y = 0;
        title = "blue · Bed";
        unit = "celsius";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "moonraker_heater_temperature{printer=\"blue\",heater=\"heater_bed\"}" ];
      })
      (stateStat {
        id = 6;
        x = 20;
        y = 0;
        title = "blue · Moonraker";
        mapping = {
          "0" = "down";
          "1" = "up";
        };
        target = "moonraker_up{printer=\"blue\"}";
      })

      # --- lime: state row -------------------------------------------
      (stateStat {
        id = 7;
        x = 0;
        y = 4;
        title = "lime · Klipper";
        mapping = {
          "0" = "startup";
          "1" = "ready";
          "2" = "error";
          "3" = "shutdown";
          "4" = "disconnected";
        };
        target = "moonraker_klippy_state{printer=\"lime\"}";
      })
      (stateStat {
        id = 8;
        x = 4;
        y = 4;
        title = "lime · Job";
        mapping = {
          "0" = "standby";
          "1" = "printing";
          "2" = "paused";
          "3" = "complete";
          "4" = "cancelled";
          "5" = "error";
        };
        target = "moonraker_print_state{printer=\"lime\"}";
      })
      (st {
        id = 9;
        x = 8;
        y = 4;
        title = "lime · Progress";
        unit = "percent";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 50; }
          { color = red; value = 90; }
        ];
        targets = [ "moonraker_print_progress{printer=\"lime\"} * 100" ];
      })
      (st {
        id = 10;
        x = 12;
        y = 4;
        title = "lime · Nozzle";
        unit = "celsius";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "moonraker_heater_temperature{printer=\"lime\",heater=\"extruder\"}" ];
      })
      (st {
        id = 11;
        x = 16;
        y = 4;
        title = "lime · Bed";
        unit = "celsius";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "moonraker_heater_temperature{printer=\"lime\",heater=\"heater_bed\"}" ];
      })
      (stateStat {
        id = 12;
        x = 20;
        y = 4;
        title = "lime · Moonraker";
        mapping = {
          "0" = "down";
          "1" = "up";
        };
        target = "moonraker_up{printer=\"lime\"}";
      })

      # --- temperatures -----------------------------------------------
      (ts {
        id = 13;
        x = 0;
        y = 8;
        w = 12;
        title = "Nozzle temperature";
        unit = "celsius";
        targets = [
          {
            expr = "moonraker_heater_temperature{heater=\"extruder\"}";
            legendFormat = "{{printer}} nozzle";
          }
          {
            expr = "moonraker_heater_target{heater=\"extruder\"}";
            legendFormat = "{{printer}} target";
          }
        ];
      })
      (ts {
        id = 14;
        x = 12;
        y = 8;
        w = 12;
        title = "Bed temperature";
        unit = "celsius";
        targets = [
          {
            expr = "moonraker_heater_temperature{heater=\"heater_bed\"}";
            legendFormat = "{{printer}} bed";
          }
          {
            expr = "moonraker_heater_target{heater=\"heater_bed\"}";
            legendFormat = "{{printer}} target";
          }
        ];
      })

      # --- print progress ---------------------------------------------
      (ts {
        id = 15;
        x = 0;
        y = 16;
        w = 12;
        title = "Print progress";
        unit = "percent";
        legendFormat = "{{printer}}";
        targets = [ "moonraker_print_progress * 100" ];
      })
      (ts {
        id = 16;
        x = 12;
        y = 16;
        w = 12;
        title = "Filament used";
        unit = "lengthmm";
        legendFormat = "{{printer}}";
        targets = [ "moonraker_print_filament_used_mm" ];
      })

      # --- whole-Pi system -------------------------------------------
      (ts {
        id = 17;
        x = 0;
        y = 24;
        w = 12;
        title = "Pi CPU";
        unit = "percent";
        targets = [
          {
            expr = "moonraker_host_cpu_percent";
            legendFormat = "{{printer}} system";
          }
          {
            expr = "moonraker_process_cpu_percent";
            legendFormat = "{{printer}} moonraker";
          }
        ];
      })
      (ts {
        id = 18;
        x = 12;
        y = 24;
        w = 12;
        title = "Pi memory";
        unit = "bytes";
        targets = [
          {
            expr = "moonraker_host_memory_used_bytes";
            legendFormat = "{{printer}} used";
          }
          {
            expr = "moonraker_host_memory_available_bytes";
            legendFormat = "{{printer}} available";
          }
          {
            expr = "moonraker_host_memory_total_bytes";
            legendFormat = "{{printer}} total";
          }
        ];
      })

      # --- SoC temp / moonraker process -------------------------------
      (ts {
        id = 19;
        x = 0;
        y = 32;
        w = 12;
        title = "Pi SoC temperature";
        unit = "celsius";
        legendFormat = "{{printer}}";
        targets = [ "moonraker_cpu_temperature_celsius" ];
      })
      (ts {
        id = 20;
        x = 12;
        y = 32;
        w = 12;
        title = "Moonraker process memory";
        unit = "bytes";
        legendFormat = "{{printer}}";
        targets = [ "moonraker_process_memory_bytes" ];
      })
    ];
  };

  # ------------------------------------------------------------------
  # OPNsense router (10.3.1.1) — the Grafana view of the same data as
  # network-info.int.leighhack.org. The network-status service ssh's to
  # the router every 5s and serves it at /metrics (job "router"); the
  # panels mirror the SPA: status pill, bandwidth, firewall states,
  # load, per-interface cards and potential issues.
  # ------------------------------------------------------------------
  "router" = {
    uid = "router";
    title = "Router (network-info)";
    time = { from = "now-6h"; to = "now"; };
    tags = [ "services1" "router" "opnsense" ];
    timezone = "browser";
    schemaVersion = 39;
    version = 1;
    refresh = "1m";
    panels = [
      # --- status row (header pill + meta) -----------------------------
      (stateStat {
        id = 1;
        x = 0;
        y = 0;
        title = "Router";
        mapping = {
          "0" = "down";
          "1" = "up";
        };
        thresholds = [
          { color = red; value = null; }
          { color = green; value = 1; }
        ];
        target = "router_up";
      })
      (st {
        id = 2;
        x = 4;
        y = 0;
        title = "Issues (bad)";
        thresholds = [
          { color = green; value = null; }
          { color = red; value = 1; }
        ];
        targets = [ "sum(router_issue{level=\"bad\"}) or vector(0)" ];
      })
      (st {
        id = 3;
        x = 8;
        y = 0;
        title = "Warnings";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 1; }
        ];
        targets = [ "sum(router_issue{level=\"warn\"}) or vector(0)" ];
      })
      (st {
        id = 4;
        x = 12;
        y = 0;
        title = "Uptime";
        unit = "s";
        thresholds = [ { color = green; value = null; } ];
        targets = [ "router_uptime_seconds" ];
      })
      (st {
        id = 5;
        x = 16;
        y = 0;
        title = "CPU usage";
        unit = "percent";
        thresholds = [
          { color = green; value = null; }
          { color = orange; value = 70; }
          { color = red; value = 90; }
        ];
        targets = [ "100 - router_cpu_percent{mode=\"idle\"}" ];
      })
      (st {
        id = 6;
        x = 20;
        y = 0;
        title = "Memory free";
        unit = "bytes";
        thresholds = [
          { color = green; value = null; }
          { color = red; value = 52428800; }    # < 50 MiB free
          { color = orange; value = 104857600; } # < 100 MiB free
        ];
        targets = [ "router_mem_bytes{state=\"free\"}" ];
      })

      # --- bandwidth (BwCard) ------------------------------------------
      (ts {
        id = 7;
        x = 0;
        y = 4;
        w = 12;
        h = 8;
        title = "Bandwidth · total";
        unit = "Bps";
        targets = [
          {
            expr = "sum by (direction) (rate(router_interface_bytes_total[5m]))";
            legendFormat = "{{direction}}";
          }
        ];
      })
      (ts {
        id = 8;
        x = 12;
        y = 4;
        w = 12;
        h = 8;
        title = "Bandwidth · per interface";
        unit = "Bps";
        legendFormat = "{{interface}} {{direction}}";
        targets = [
          "rate(router_interface_bytes_total[5m])"
        ];
      })

      # --- connections / load (ConnChart + LoadChart) --------------------
      (ts {
        id = 9;
        x = 0;
        y = 12;
        w = 8;
        title = "Firewall state table";
        legendFormat = "states";
        targets = [ "router_pf_states" ];
      })
      (ts {
        id = 10;
        x = 8;
        y = 12;
        w = 8;
        title = "Router TCP sockets";
        legendFormat = "sockets";
        targets = [ "router_tcp_sockets" ];
      })
      (ts {
        id = 11;
        x = 16;
        y = 12;
        w = 8;
        title = "TCP retransmits";
        unit = "ops";
        legendFormat = "retrans/s (reported)";
        targets = [ "router_retransmissions_rate_per_second" ];
      })
      (ts {
        id = 12;
        x = 0;
        y = 16;
        w = 12;
        title = "Load average";
        targets = [
          { expr = "router_load1"; legendFormat = "1m"; }
          { expr = "router_load5"; legendFormat = "5m"; }
          { expr = "router_load15"; legendFormat = "15m"; }
        ];
      })

      # --- per-interface cards ------------------------------------------
      # Instant table, one row per interface: link state and the current
      # up/down byte rate (5s scrape -> a 1m rate window is ~12 samples).
      {
        id = 13;
        x = 12;
        y = 16;
        w = 12;
        h = 5;
        title = "Interfaces";
        type = "table";
        datasource = DS;
        gridPos = { x = 12; y = 16; w = 12; h = 5; };
        targets = [
          { expr = "router_interface_up"; legendFormat = "link"; instant = true; }
          {
            expr = "sum by (interface) (rate(router_interface_bytes_total{direction=\"down\"}[1m]))";
            legendFormat = "down";
            instant = true;
          }
          {
            expr = "sum by (interface) (rate(router_interface_bytes_total{direction=\"up\"}[1m]))";
            legendFormat = "up";
            instant = true;
          }
        ];
        fieldConfig = {
          defaults = { custom = { align = "auto"; }; };
          overrides = [
            { matcher = { id = "byName"; options = "down"; }; properties = [ { id = "unit"; value = "Bps"; } { id = "decimals"; value = 1; } ]; }
            { matcher = { id = "byName"; options = "up"; }; properties = [ { id = "unit"; value = "Bps"; } { id = "decimals"; value = 1; } ]; }
          ];
        };
        options = { showHeader = true; footer = { show = false; }; };
      }

      # --- CPU / memory breakdown ----------------------------------------
      (ts {
        id = 14;
        x = 0;
        y = 21;
        w = 12;
        title = "Router CPU";
        unit = "percent";
        legendFormat = "{{mode}}";
        targets = [ "router_cpu_percent" ];
      })
      (ts {
        id = 15;
        x = 12;
        y = 21;
        w = 12;
        title = "Router memory";
        unit = "bytes";
        legendFormat = "{{state}}";
        targets = [ "router_mem_bytes" ];
      })

      # --- potential issues (Issues panel) --------------------------------
      {
        id = 16;
        x = 0;
        y = 26;
        w = 24;
        h = 6;
        title = "Potential issues";
        type = "table";
        datasource = DS;
        gridPos = { x = 0; y = 26; w = 24; h = 6; };
        targets = [
          {
            expr = "router_issue == 1";
            legendFormat = "issue";
            instant = true;
          }
        ];
        options = { showHeader = true; footer = { show = false; }; };
      }
    ];
  };
}
