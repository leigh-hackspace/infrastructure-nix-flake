# HELPER — function of {lib} returning the gatus endpoint tables; the data itself lives in the `let` below.
# The gatus endpoint list — the hand-maintained half of "is this thing up".
#
# Kept as plain data (no `config` dependency) so the two readers of it stay
# independent: gatus (machines/services1/services/gatus.nix) imports the list and
# the drift guard (machines/services1/services/monitoring.nix) reads the vhost
# side.  It used to matter *why* it could not be read off the box being checked —
# gatus ran on services2 and the vhosts were on services1 — and since 2026-10-17
# they are the same box, but the list stays hand-maintained data for the reason
# below, not that one: generating it from `services.nginx.virtualHosts` is
# deliberately *not* done (repo audit §3.7): it mixes nginx-served apps with ICMP/TCP
# targets that are dnsmasq hosts rather than vhosts (cameras, switches, the printers,
# the NAS), it watches public names too, and every entry carries its own group and
# status condition.  Auto-generating would invent checks for things that are not
# meant to be watched and quietly change what alerts fire.
#
# The drift guard: every https `*.int.leighhack.org` endpoint here must be a name
# services1's nginx serves, or a renamed/removed app leaves gatus alerting on a
# name that no longer exists, forever.
#
# Generating this list from `services.nginx.virtualHosts` is deliberately *not*
# done (repo audit §3.7): it mixes nginx-served apps with ICMP/TCP targets that
# are dnsmasq hosts rather than vhosts (cameras, switches, the printers, the NAS),
# it watches public names too, and every entry carries its own group and status
# condition.  Auto-generating would invent checks for things that are not meant to
# be watched and quietly change what alerts fire.
#
#   endpoints = import common/monitoring-endpoints.nix { inherit lib; };
#   endpoints.hosts   # https hosts checked, for the drift guard
#   endpoints.list    # the gatus `endpoints` setting
{lib}: let
  HOSTS = import ./hosts.nix;

  # Host of an https URL; null for http://, icmp://, tcp:// and friends.  Only
  # https is checked: those are the names `gw`'s front door answers.  The
  # http:// *.int endpoints (3d-*, woodwork) are dnsmasq hosts on the hackspace
  # LAN, not vhosts here.
  httpsHost = url: let
    m = builtins.match "https://([^/:]+).*" url;
  in
    if m == null
    then null
    else builtins.head m;

  # Standard Slack alert, attached to every endpoint.
  #
  # The two knobs that decide when a message goes out are failure-threshold
  # (consecutive failed checks needed to fire) and success-threshold (consecutive
  # successes needed to mark the incident resolved, which send-on-resolved then
  # announces). gatus defaults them to 3 and 2; they are spelled out here so the
  # debounce is explicit rather than inherited.
  #
  # There is no per-alert `condition` and no `snooze` in gatus 5.36.0 (see
  # alerting/alert/alert.go) — yaml.v3 ignores unknown keys, so the
  # `condition: ALERT_COUNT == 2` this file used to carry was silently dead config
  # and the effective threshold was the default 3. Debouncing means the thresholds.
  slackAlert = {
    type = "slack";
    description = "down";
    failure-threshold = 3;
    success-threshold = 2;
    send-on-resolved = true;
  };

  # Same alert, debounced for sources that flap: the WiFi cameras. Measured over a
  # week of gatus's own check log on the previous config, Cam 1 - Rack fired 14
  # times and Cam 9 - Social Space 7, and nearly all of it was single-check blips
  # (225 of Cam 1's 276 failure runs were 1 check long, 34 were 2). At
  # failure-threshold 5 a blip never fires while a camera genuinely off for 5
  # minutes still does, and success-threshold 3 stops the "resolved" message for a
  # momentary bounce back that never reached anyone in the first place.
  flappingSlackAlert =
    slackAlert
    // {
      failure-threshold = 5;
      success-threshold = 3;
    };

  # Same alert, debounced for an endpoint whose *startup* is the outage window.
  #
  # The three inference engines answer 502 / connection-refused while they load, and that is
  # normal: `strata.service` on services2 measured **55 s** from systemd start to `ready:` on
  # 2026-11-15 (12:39:08 → 12:40:03), and its own banner warns "1-3 MINUTES" for the part
  # that migrates ~30 GB of experts into RAM on a cold page cache — during which the box may
  # look unresponsive by design. A check at the default failure-threshold 3 therefore pages
  # on a routine engine restart, which is exactly the alert that teaches you to ignore
  # inference alerts. At 5 failing points (~5 min at this interval) a cold start never fires
  # and an engine that is actually dead still does inside six minutes. Same reasoning as
  # `flappingSlackAlert` above, different cause: there the blip was radio, here it is load.
  slowStartSlackAlert =
    slackAlert
    // {
      failure-threshold = 5;
      success-threshold = 3;
    };

  # HTTP check. `status` is the status-code condition (default: any 2xx).
  # `insecure` skips TLS verification (kuma "ignore TLS").
  mkHttp = {
    name,
    url,
    status ? "[STATUS] >= 200 && [STATUS] < 300",
    group ? null,
    alert ? slackAlert,
    insecure ? false,
    # Extra checks on the response, appended after `status` and before the
    # response-time ceiling.  Added for the lease table, which is the one endpoint
    # where a **200 is not enough**: `/api/leases` answers 200 with
    # `"stale":"all-expired"` when dnsmasq's file has been replaced under it and
    # nobody is writing the path any more (`docs/router-and-dns.md`, 2026-10-28).
    extraConditions ? [],
  }:
    (lib.optionalAttrs (group != null) {inherit group;})
    // {
      inherit name url;
      method = "GET";
      interval = "60s";
      timeout = "10s";
      conditions =
        [status]
        ++ extraConditions
        ++ ["[RESPONSE_TIME] < 5000"];
      alerts = [alert];
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
    alert ? slackAlert,
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
      alerts = [alert];
    };

  # TCP port check. `url` is host:port (the tcp:// scheme is required for gatus to
  # recognise the endpoint as a TCP check).
  mkTcp = {
    name,
    url,
    group ? null,
    alert ? slackAlert,
  }:
    (lib.optionalAttrs (group != null) {inherit group;})
    // {
      inherit name;
      url = "tcp://${url}";
      tcp = {};
      interval = "60s";
      timeout = "10s";
      conditions = ["[RESPONSE_TIME] < 1000"];
      alerts = [alert];
    };

  list = [
    # --- Infrastructure ----------------------------------------------
    # The machines themselves.  gatus now runs on services1, so the check that is
    # worth having is the *other* box: services2, whose RAM is strata's and whose
    # own status dashboard is what survives the engine thrashing (see
    # machines/services1/services/gatus.nix for the trade-off).  A ping of the box
    # gatus lives on can only ever catch a hung nginx, never a dead box, so the
    # Services1 entry that used to be here is gone rather than kept as a check
    # that can never fail.
    (mkPing {
      name = "Services2";
      url = HOSTS.services2;
      group = "Infrastructure";
    })
    (mkPing {
      name = "Aibox";
      url = HOSTS.aibox;
      group = "Infrastructure";
    })
    (mkPing {
      name = "Router";
      url = HOSTS.router;
      group = "Infrastructure";
    })

    # The router's DHCP lease table, because the page can be broken while every name
    # on the board is green: `network-status` answers 503 on `/api/leases` with the
    # file's own errno when it cannot read `/var/lib/dnsmasq/dnsmasq.leases`, and that
    # is a **permission on one file**, invisible to a check of the vhost or of the
    # process.  Measured 2026-11-21: a `systemctl restart dnsmasq` chowned the file
    # `root:root`, the re-grant's inotify trigger was missed, and the page was 503
    # for 3 h 30 min — the box's own `lease-reader-watch` was red the whole time and
    # nothing told anyone, because Prometheus's `ServiceFailed` rule has no receiver
    # (`machines/services1/services/observability.nix`).  So this row is the receiver
    # for that one fault, over the channel that already exists.
    #
    # failure-threshold 6 (~6 min at 60 s) on purpose: `machines/gw/lease-reader.nix`
    # now re-grants the group from its own 5-minute check, so a single lost event
    # heals by itself and must not page — what pages is a channel still broken after
    # that, which is the case nobody can fix without being told.
    #
    # The second condition is the other half, and the reason this is not just an HTTP
    # check: a **200 with a dead writer** is the 2026-10-28 shape (the file replaced
    # under dnsmasq's open descriptor, every row lapsed, the page plausible). `stale`
    # is `network-status`'s own verdict on that (`crates/network-status/src/leases.rs`
    # is its only definition) and `null` is the healthy value; `pat()` is gatus's
    # pattern function (`[BODY] == pat(*…*)`, `config/endpoint`'s `pat(` prefix).
    (mkHttp {
      name = "Router lease table (network-info)";
      url = "http://${HOSTS.router}:8091/api/leases";
      group = "Infrastructure";
      extraConditions = [
        ''[BODY] == pat(*"stale":null*)''
      ];
      alert =
        slackAlert
        // {
          failure-threshold = 6;
          success-threshold = 2;
        };
    })
    (mkPing {
      name = "NAS2";
      url = HOSTS.nas;
      group = "Infrastructure";
    })

    # --- Inference --------------------------------------------------
    # Three engines, three boxes, three GPU backends — and until 2026-11-15 none of them
    # had a check here, which is why two of them went down during a whole-network upgrade
    # and nothing said so. Both failures had **zero failed units**: activation restarted
    # the engine into new NVIDIA user-space while the old kernel module was still loaded,
    # so the unit neither crashed nor reported — it just could not reach the card, and the
    # endpoint 502'd until a reboot (`docs/work-log-2026-11.md`; the unit-level guard is
    # `common/gpu-driver-match.nix`). The lesson is that for an inference box the *endpoint*
    # is the only thing that can tell you the engine is serving: a unit state, a port
    # listening, or a healthy generation all fail to distinguish "loading" from "cannot see
    # the GPU". `/health` on each, not `/`: llama.cpp's UI is gzipped into the binary and
    # `/` answers oddly to a client that sends no `Accept-Encoding: gzip`
    # (`machines/services3/README.md`).
    #
    # `ai.int.leighhack.org` / `ai.leighhack.org` are deliberately **absent**. They proxy to
    # `aibox`'s llama.cpp fork, and `machines/aibox/strata.nix` clears that unit's
    # `wantedBy` because its ~33 GB of GTT and strata's arena cannot be resident at once —
    # so those two names answer 502 as the designed steady state while strata owns the box.
    # A check that is red by design is worse than no check: it trains everyone to ignore the
    # board. Adding them means first giving the check a stated expectation; that is open in
    # `docs/follow-ups.md`.
    (mkHttp {
      name = "Strata (services2, CUDA)";
      url = "https://strata.int.leighhack.org/health";
      group = "Inference";
      alert = slowStartSlackAlert;
    })
    (mkHttp {
      name = "Strata2 (aibox, HIP)";
      url = "https://strata2.int.leighhack.org/health";
      group = "Inference";
      alert = slowStartSlackAlert;
    })
    (mkHttp {
      name = "Llama (services3, Vulkan)";
      url = "https://llama.int.leighhack.org/health";
      group = "Inference";
      alert = slowStartSlackAlert;
    })
    # Whisper is on the voice path, so its absence from the board was the gap that
    # mattered: `voice-assistant` dials `aibox:8082` for every utterance and had no check,
    # so "the assistant stopped understanding me" was a journal read rather than a red
    # row.  `/health` is a cheap static answer, and the model is already loaded at start.
    (mkHttp {
      name = "Whisper (aibox, CPU)";
      url = "https://whisper.int.leighhack.org/health";
      group = "Inference";
    })
    # The voice gateway's own `/health` is one JSON line and answers even with the bot's
    # ask socket down — which is the useful distinction: **deaf** (whisper unreachable,
    # this row red via the Whisper row above) and **mute** (the agent socket down, this row
    # green and `errors` climbing in `/status.json`) are different incidents with different
    # fixes, and a check that conflates them tells you neither.
    (mkHttp {
      name = "Voice assistant (services1)";
      url = "https://voice.int.leighhack.org/health";
    })

    # --- Top-level ---------------------------------------------------
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
      url = "${HOSTS.nas}:22";
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
      # services1's own dashboard, on its LAN name. It used to be
      # `https://status.leighhack.org`, a check that had never once been green: the
      # vhost existed on the origin and the door, the DigitalOcean record never did,
      # so gatus dialled a name that is NXDOMAIN for every resolver on the internet
      # (`docs/follow-ups.md` had it as "red since the day it was written", and the
      # two vhosts and this URL were deleted together on 2026-11-16).
      name = "Status Dashboard";
      url = "https://services1-status.int.leighhack.org";
    })
    (mkHttp {
      name = "Gatus";
      url = "https://gatus.int.leighhack.org";
    })
    (mkHttp {
      name = "Prometheus";
      url = "https://prometheus.int.leighhack.org";
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
      url = "https://${HOSTS.services1}:8443";
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
    # Frigate is a service on services1, so it keeps the standard alert;
    # everything below is a camera on WiFi and uses the debounced one.
    (mkHttp {
      name = "Frigate";
      url = "https://frigate.int.leighhack.org";
      group = "Cameras";
    })
    (mkPing {
      name = "Cam 1 - Rack";
      url = "cam1.int.leighhack.org";
      group = "Cameras";
      alert = flappingSlackAlert;
    })
    (mkPing {
      name = "Cam 9 - Social Space";
      url = "cam9.int.leighhack.org";
      group = "Cameras";
      alert = flappingSlackAlert;
    })
    (mkPing {
      name = "Main Space";
      url = "main_space.int.leighhack.org";
      group = "Cameras";
      alert = flappingSlackAlert;
    })
    (mkPing {
      name = "Cam 5 - Pi Room";
      url = "cam5.int.leighhack.org";
      group = "Cameras";
      alert = flappingSlackAlert;
    })
    (mkPing {
      name = "Workshop";
      url = "workshop.int.leighhack.org";
      group = "Cameras";
      alert = flappingSlackAlert;
    })
    (mkHttp {
      name = "Woodwork";
      url = "http://woodwork.int.leighhack.org";
      group = "Cameras";
      alert = flappingSlackAlert;
    })

    # --- Network Switches ------------------------------------------
    # The ProCurve core switch's GUI (switch-gui).  Checked on `/healthz`, not
    # `/`: an unauthenticated request to the app is a 302 into authentik, which
    # the default "any 2xx" condition would read as down.  `/healthz` is the
    # one route that is public by design.
    (mkHttp {
      name = "Core Switch GUI";
      url = "https://switch.int.leighhack.org/healthz";
      group = "Network Switches";
    })
    (mkPing {
      name = "Switch1";
      url = HOSTS.switch1;
      group = "Network Switches";
    })
    (mkPing {
      name = "Switch2";
      url = HOSTS.switch2;
      group = "Network Switches";
    })
    (mkPing {
      name = "Switch3";
      url = HOSTS.switch3;
      group = "Network Switches";
    })
  ];
in {
  inherit httpsHost;
  inherit list;
}
