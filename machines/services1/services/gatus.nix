{
  lib,
  config,
  ...
}:
# Gatus — the uptime/health status page. It replaces Uptime Kuma (which ran
# out-of-band on apps1, 10.3.1.30) and now owns the whole monitoring list.
#
# The endpoints below are a 1:1 port of the *active* Uptime Kuma monitors
# (read from /srv/uptimekuma/kuma.db on apps1). Kuma's disabled monitors
# (Laser-1, Filestore Web, Kubernetes Lab, CNC-1, Cam 7) were intentionally
# left out. Type mapping:
#   kuma http  -> gatus HTTP endpoint  (url + [STATUS] condition)
#   kuma ping  -> gatus ICMP endpoint  (icmp: {} + [RESPONSE_TIME])
#   kuma port  -> gatus TCP endpoint   (tcp: {} + [RESPONSE_TIME])
# Kuma's three groups (Fabrication, Cameras, Network Switches) are kept via
# gatus's `group` field. Alerting goes to the same #infra-alerts Slack
# channel kuma used (alerting.slack.webhook-url below), firing after 2
# consecutive failures and on recovery.
#
#   https://gatus.int.leighhack.org   (LAN/tailnet only, like kuma was)
let
  CONFIG = import ../config.nix;
  mkIntVhost = import ../lib/nginx-int-vhost-helper.nix {inherit lib;};
  intVhostNames = import ../lib/int-vhost-names.nix {inherit lib;};

  # Host of an https URL; null for http://, icmp://, tcp:// and friends.  Only
  # https is checked: those are the vhosts this box terminates TLS for.  The
  # http:// *.int endpoints (3d-*, woodwork) are dnsmasq hosts on the hackspace
  # LAN, not vhosts here.
  httpsHost = url: let
    m = builtins.match "https://([^/:]+).*" url;
  in
    if m == null
    then null
    else builtins.head m;

  # https *.int endpoints that are not names this nginx serves; the assertion at
  # the bottom of this file requires this to be empty.
  unbackedIntEndpoints = let
    hosts =
      builtins.filter (h: h != null && lib.hasSuffix ".int.leighhack.org" h)
      (map (e: httpsHost (e.url or "")) (config.services.gatus.settings.endpoints or []));
  in
    lib.subtractLists (intVhostNames config.services.nginx.virtualHosts) hosts;

  # Standard Slack alert, attached to every endpoint.
  # ALERT_COUNT is the number of consecutive failed checks for that endpoint.
  slackAlert = {
    type = "slack";
    description = "down";
    condition = "ALERT_COUNT == 2";
    send-on-resolved = true;
    snooze = "10m";
  };

  # HTTP check. `status` is the status-code condition (default: any 2xx).
  # `insecure` skips TLS verification (kuma "ignore TLS").
  mkHttp = {
    name,
    url,
    status ? "[STATUS] >= 200 && [STATUS] < 300",
    group ? null,
    insecure ? false,
  }:
    (lib.optionalAttrs (group != null) {inherit group;})
    // {
      inherit name url;
      method = "GET";
      interval = "60s";
      timeout = "10s";
      conditions = [
        status
        "[RESPONSE_TIME] < 5000"
      ];
      alerts = [slackAlert];
    }
    // (lib.optionalAttrs insecure {
      client."insecure-skip-verify" = true;
    });

  # ICMP (ping) check. `url` is the host (name or IP) to ping; `max` is the
  # response-time ceiling in ms (LAN defaults to 1000, internet is looser).
  # The icmp:// scheme is required for gatus to treat it as a ping.
  mkPing = {
    name,
    url,
    group ? null,
    max ? "1000",
  }:
    (lib.optionalAttrs (group != null) {inherit group;})
    // {
      inherit name;
      url = "icmp://${url}";
      icmp = {};
      interval = "60s";
      timeout = "10s";
      conditions = ["[RESPONSE_TIME] < ${max}"];
      alerts = [slackAlert];
    };

  # TCP port check. `url` is host:port (the tcp:// scheme is required for
  # gatus to recognise the endpoint as a TCP check).
  mkTcp = {
    name,
    url,
    group ? null,
  }:
    (lib.optionalAttrs (group != null) {inherit group;})
    // {
      inherit name;
      url = "tcp://${url}";
      tcp = {};
      interval = "60s";
      timeout = "10s";
      conditions = ["[RESPONSE_TIME] < 1000"];
      alerts = [slackAlert];
    };
in {
  # --- drift guard (docs/repo-audit-2026-10-07.md §3.7) ----------------------
  #
  # Gatus, the Prometheus alert rules and the status dashboard all model "is this
  # thing up", and this endpoint list is the one part of that which is
  # hand-maintained.  Generating it from `services.nginx.virtualHosts` (the
  # attrset dns-sync already derives the expected DNS names from) is deliberately
  # *not* done: the list mixes nginx-served apps with ICMP/TCP targets that are
  # dnsmasq hosts rather than vhosts (cameras, switches, the printers, the NAS),
  # it watches public names too, and every entry carries its own group and status
  # condition.  Auto-generating would invent checks for things that are not meant
  # to be watched and quietly change what alerts fire.
  #
  # What must not drift is the other direction: an https `*.int.leighhack.org`
  # endpoint has to be a name this box's nginx actually serves.  Otherwise a
  # renamed or removed app leaves gatus alerting on a name that no longer exists
  # here, forever.  That is asserted below, on every eval.
  assertions = [
    {
      assertion = unbackedIntEndpoints == [];
      message = "gatus: these https *.int.leighhack.org endpoints are not vhosts this nginx serves: ${builtins.toJSON unbackedIntEndpoints}.  The app was renamed or removed and its gatus endpoint was left behind — remove or retarget it in machines/services1/services/gatus.nix.";
    }
  ];

  services.gatus = {
    enable = true;
    settings = {
      web.port = 8999;
      alerting.slack.webhook-url = lib.strings.trim (
        builtins.readFile (config.sopsSecretText "slack_url")
      );

      endpoints = [
        # --- Top-level -------------------------------------------------
        (mkHttp {
          name = "Leigh Hackspace Website";
          url = "https://leighhack.org";
        })
        (mkPing {
          name = "Google DNS";
          url = "8.8.8.8";
          max = "3000";
        })
        (mkHttp {
          name = "Authentik";
          url = "https://id.leighhack.org";
        })
        # kuma accepted 200 or a 302 login-redirect here. gatus follows
        # redirects by default (reports the final status), so a 302 ->
        # /login lands on 200 and this condition is correct. (gatus's
        # condition DSL has no ||, so we can't express "200 or 302" directly.)
        (mkHttp {
          name = "Home Assistant";
          url = "https://ha.int.leighhack.org";
          status = "[STATUS] == 200";
        })
        (mkTcp {
          name = "NAS2 (SSH)";
          url = "nas2.int.leighhack.org:22";
        })
        (mkHttp {
          name = "Monster Web UI";
          url = "https://monster.int.leighhack.org";
        })
        (mkHttp {
          name = "Zigbee2MQTT";
          url = "https://zigbee2mqtt.int.leighhack.org/";
        })
        (mkHttp {
          name = "User Tweaker";
          url = "https://user-tweaker.leighhack.org/metrics";
          status = "[STATUS] == 200";
        })
        (mkHttp {
          name = "Access API";
          url = "https://access-api.int.leighhack.org/health";
        })
        (mkHttp {
          name = "Hackspace API";
          url = "https://api.leighhack.org/health";
          status = "[STATUS] == 200";
        })
        (mkHttp {
          name = "Grafana";
          url = "https://grafana.int.leighhack.org";
        })
        (mkHttp {
          name = "Door Entry Management System";
          url = "https://doors.leighhack.org";
        })
        # kuma "ignore TLS" was set on this one.
        (mkHttp {
          name = "Unifi Controller";
          url = "https://10.3.1.20:8443";
          insecure = true;
        })

        # --- Fabrication -----------------------------------------------
        (mkHttp {
          name = "3D-1";
          url = "http://3d-1.int.leighhack.org";
          group = "Fabrication";
        })
        (mkHttp {
          name = "3D-2";
          url = "http://3d-2.int.leighhack.org";
          group = "Fabrication";
        })
        (mkHttp {
          name = "3D-3";
          url = "http://3d-3.int.leighhack.org";
          group = "Fabrication";
        })

        # --- Cameras ---------------------------------------------------
        (mkHttp {
          name = "Frigate";
          url = "https://frigate.int.leighhack.org";
          group = "Cameras";
        })
        (mkPing {
          name = "Cam 1 - Rack";
          url = "cam1.int.leighhack.org";
          group = "Cameras";
        })
        (mkPing {
          name = "Cam 9 - Social Space";
          url = "cam9.int.leighhack.org";
          group = "Cameras";
        })
        (mkPing {
          name = "Main Space";
          url = "main_space.int.leighhack.org";
          group = "Cameras";
        })
        (mkPing {
          name = "Cam 5 - Pi Room";
          url = "cam5.int.leighhack.org";
          group = "Cameras";
        })
        (mkPing {
          name = "Workshop";
          url = "workshop.int.leighhack.org";
          group = "Cameras";
        })
        (mkHttp {
          name = "Woodwork";
          url = "http://woodwork.int.leighhack.org";
          group = "Cameras";
        })

        # --- Network Switches ------------------------------------------
        (mkPing {
          name = "Switch1";
          url = "switch1.int.leighhack.org";
          group = "Network Switches";
        })
        (mkPing {
          name = "Switch2";
          url = "switch2.int.leighhack.org";
          group = "Network Switches";
        })
        (mkPing {
          name = "Switch3";
          url = "switch3.int.leighhack.org";
          group = "Network Switches";
        })
      ];
    };
  };

  # kuma's status page was LAN/tailnet only; keep gatus the same.
  services.nginx.virtualHosts."gatus.int.leighhack.org" = mkIntVhost {
    proxyPass = "http://localhost:8999";
  };
}
