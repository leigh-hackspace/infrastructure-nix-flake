# MODULE — the IPv6 router advertisements, watched, and repaired when they stop.
#
# dnsmasq advertises a default route on every VLAN that has an `ra` range in
# common/vlans.nix (`./dns.nix`'s `enable-ra`, the generated `ra-param` lines in
# `./lib/network-tables.nix`).  Its **unsolicited** advertisement timer dies when
# `router-lan` loses carrier — the flap itself, not the cable — and then nothing
# on this box is broken in any way a unit, a log line or a dashboard can see:
#
#   * dnsmasq is still running, still answering DNS and DHCP, still replying to a
#     router solicitation, and still logging;
#   * the clients keep their addresses, because the prefix's valid lifetime is a
#     day, so every box still looks IPv6-configured;
#   * what expires is the *default route*, whose lifetime `ra-param` sets to
#     1200 s — so ~20 minutes after the last advertisement the wired LAN quietly
#     becomes a network with no off-link IPv6, and every `*.int` name still
#     answers (they are all `2001:8b0:1d14:225::1`, on-link), which is why this
#     hides until somebody pings something outside the /64.
#
# Measured occurrences: 2026-10-21 (the cutover's re-cabling), 2026-11-19 14:17
# (found on 2026-11-21, two days of no off-link IPv6 on services1 *and*
# services2), and 2026-11-26 11:08–11:13 (a five-minute `router-lan` outage;
# services1, services2 and services3 all had `expires …` prefix routes and no
# default at 15:55).  `systemctl restart dnsmasq` fixed it every time inside a
# second, and nobody was watching any time.  This module is the watcher.
#
# ## What it measures, and why not the counter everyone reached for first
#
# `docs/follow-ups.md` proposed watching `nstat Ip6OutMcastPkts` on this box for
# "sitting still longer than the interval".   **Measured 2026-11-26, that meter
# cannot answer the question**: over 150 s it moved 36379 → 36394 (+15) while only
# three router advertisements left this box on `vlan225` (captured, 51 s apart), so
# roughly 40 % of the movement is other IPv6 multicast (MLD, DHCPv6, mDNS).  A
# dead RA timer would leave that counter *moving* and the check would stay green —
# a guard that cannot fail, which is worse than none.
#
# So the check counts **the advertisements themselves**: one `tcpdump` per
# RA-advertising VLAN, kernel-filtered to ICMPv6 type 134, `-p` so the NIC is not
# put in promiscuous mode, over a window of `window` seconds; every existing
# interface must produce one.  `machines/gw/README.md` used to say tcpdump "never
# sees this box's RAs"; it does — measured 2026-11-26, 0 type-134 frames in 70 s on
# `vlan225` during the fault and three in 150 s after the restart, 51 s apart.  The
# earlier conclusion was jitter in the timer, not invisible packets (see that
# file's correction).
#
# A solicited advertisement would also satisfy the capture, and dnsmasq answers
# those while the timer is dead — but a client only solicits at boot or after a
# carrier event, so at worst one window is masked and the next is not.
#
# ## What it does about it
#
# The repair is the one an operator performs, so it performs it: `systemctl restart
# dnsmasq`, then a second, shorter capture.  Two things make that safe on the box
# that answers DNS and DHCP for the building:#
#   * a restart is a **routine event here** — every activation that changes the
#     DNS tables restarts dnsmasq, and it reloads its lease file, so no client
#     loses its lease to it;
#   * it is **rate-limited to once per 15 min** by a stamp file in `/run`.  A fault
#     the restart does not fix (a bad `enable-ra`, a dnsmasq that will not start)
#     must not become a restart loop on the router; after the first attempt this
#     unit only reports.
#
# And it still **fails**, even when the repair worked — the house rule from
# `./lease-reader.nix`: a repair means the event-driven path that should have done
# it was missing, and a red unit is how that stays visible.  Red on *this* box is
# not seen by anyone (`machines/services1/services/observability.nix` has no
# alertmanager receiver), so the same sentence also goes to Slack through the
# webhook `./sops.nix` renders to `/run/secrets/slack_url`.  That is the whole
# point of this file: the last three occurrences were found by a person trying to
# ping something.
#
# ## What it does not do
#
# It does not touch the clients, and it does not shorten or lengthen any lifetime:
# the 1200 s router lifetime *is* the grace period that makes a poll-based repair
# sufficient (worst case here is a 150 s window plus the 2 min idle before the
# next run, against 1200 s of grace).  It does not check IPv4, DNS, DHCP or the
# tunnel — each of those has its own check and its own signal.  A partial fault
# (one VLAN's advertisements stopping while the others continue) *is* caught,
# because every interface is required to produce one.
#
# ## Running it by hand, and the one knob
#
# `sudo systemctl start ra-watch` is the real check.  The script also takes
# `<window> [<recheck>]` in seconds, because the only way to exercise the repair
# path on a working box is to ask it a question it cannot answer in time:
#
#     sudo $(systemctl cat ra-watch | sed -n 's/^ExecStart=+//p') 1
#
# measures for one second, finds nothing, **restarts dnsmasq for real**, and posts
# to Slack — which is exactly the path that must not be discovered broken at 03:00.
# The default (150 s) is what the timer runs with.
{
  lib,
  pkgs,
  ...
}: let
  VLANS = import ../../common/vlans.nix;
  TABLES = import ./lib/network-tables.nix;

  # Derived from the table dnsmasq's own `ra-param` lines are generated from, so a
  # VLAN that gains or loses an `ra` range cannot be watched in one place and
  # advertised in another.
  devices = map TABLES.vlanDevice (builtins.filter (v: v.ra != []) VLANS);

  # `ra-param` is 60 s (`./lib/network-tables.nix`) and the measured cadence is
  # 51 ± 1 s per interface; 150 s is three chances, and with the timer's 2 min idle
  # it puts a repair inside a quarter of the 1200 s lifetime the clients are given.
  window = 150;
  # What the restart gets to prove itself: dnsmasq advertises as it starts, so this
  # does not have to wait for a full interval.
  recheck = 90;
  # One restart per quarter hour, whatever is wrong.
  restartCooldown = 900;
  stateDir = "/run/ra-watch";
  stamp = "/run/ra-watch.last-restart";
  slackSecret = "/run/secrets/slack_url";

  watch = pkgs.writeShellScript "ra-watch" ''
    set -u
    # Every command below is an absolute store path, because a unit's `PATH` is not
    # the login shell's (measured on this box: no `PATH` in a unit's environment at
    # all), and 0600 for the one file that holds the webhook URL.
    umask 077
    CO=${pkgs.coreutils}/bin

    devs="${lib.concatStringsSep " " devices}"
    window=''${1:-${toString window}}
    recheck=''${2:-${toString recheck}}
    cooldown=${toString restartCooldown}
    state=${stateDir}
    stamp=${stamp}

    fail() { echo "ra-watch: $*" >&2; exit 1; }

    # The alarm that reaches a person.  Never fatal: a webhook that cannot be read
    # or dialled must not turn "IPv6 is fine" into a red unit, and the message is
    # written into the journal first by the caller for exactly that reason.
    slack() {
      msg=$1
      [ -r "${slackSecret}" ] || { echo "ra-watch: note — ${slackSecret} is unreadable, so no Slack message was sent" >&2; return 0; }
      body=$state/slack.json
      conf=$state/curl.conf
      printf '{"text":"%s"}' "$msg" > "$body"
      printf 'url = "%s"\n' "$($CO/cat "${slackSecret}")" > "$conf"
      ${pkgs.curl}/bin/curl -sS -m 10 -K "$conf" \
        -H 'Content-Type: application/json' --data-binary "@$body" >/dev/null 2>&1 \
        || echo "ra-watch: note — the Slack webhook did not accept the message" >&2
      $CO/rm -f "$conf" "$body"
    }

    # One window, every interface at once.  Prints the interfaces that advertised;
    # an interface with no line of its own did not.  `-c 1` plus `timeout` is the
    # bound: tcpdump exits the moment it sees an advertisement and is killed when the
    # window closes without one, so the *file's content* is the answer — and the test
    # is the decoded line, because a killed tcpdump leaves a newline behind and an
    # emptiness test would call every window a hit.
    advertised() {
      w=$1
      $CO/rm -rf "$state"
      $CO/mkdir -p "$state"
      for d in $devs; do
        $CO/timeout "$w" ${pkgs.tcpdump}/bin/tcpdump \
          -q -n -p -c 1 -i "$d" 'icmp6 and ip6[40] == 134' \
          >"$state/$d.out" 2>/dev/null &
      done
      wait || true
      hit=""
      for d in $devs; do
        # The word, not a non-empty file: a timed-out tcpdump still leaves a 1-byte
        # newline in it (measured 2026-11-26, and a `-s` test passed a 1 s window
        # because of it — every run looked like a hit).
        ${pkgs.gnugrep}/bin/grep -q 'router advertisement' "$state/$d.out" 2>/dev/null && hit="$hit $d"
      done
      echo "''${hit# }"
    }

    # Only the interfaces that exist right now can be asked, and this unit can run
    # before they exist: a monotonic timer whose unit has never run elapses as soon
    # as it is activated (measured 2026-11-26 — the timer fired at 16:08:49, on the
    # activation that created it), which at boot means "before networkd has made the
    # VLANs".  Wait for them and for dnsmasq rather than reporting that as the fault;
    # both are bounded, and being slow is not the failure this unit is about.
    tries=0
    while :; do
      present=""
      for d in $devs; do
        [ -e "/sys/class/net/$d" ] && present="$present $d"
      done
      present="''${present# }"
      [ -n "$present" ] && break
      tries=$((tries + 1))
      [ "$tries" -lt 24 ] || fail "none of the router-advertising VLAN devices ($devs) exists on this box after 120 s, so nothing is being advertised at all — check systemd-networkd and ./networking.nix"
      $CO/sleep 5
    done
    devs=$present

    # dnsmasq is the advertiser; measuring while it is still starting would report a
    # boot ordering rather than a fault.
    tries=0
    while ! ${pkgs.systemd}/bin/systemctl is-active --quiet dnsmasq; do
      tries=$((tries + 1))
      [ "$tries" -lt 24 ] || fail "dnsmasq is not running, so nothing is advertising IPv6 on $devs — journalctl -u dnsmasq (this box also has no DNS and no DHCP until it starts)"
      $CO/sleep 5
    done

    alive=$(advertised "$window")
    [ -n "$alive" ] || {
      # Nothing advertised: repair, then look again.
      now=$($CO/date +%s)
      if [ -e "$stamp" ] && [ $((now - $($CO/cat "$stamp"))) -lt "$cooldown" ]; then
        slack "gw: no IPv6 router advertisement for ''${window}s on $devs, and dnsmasq was already restarted less than 15 min ago — not restarting it again. Off-link IPv6 on those VLANs will lapse within 1200 s. machines/gw/ra-watch.nix"
        fail "no router advertisement left this box on $devs for ''${window}s, and a restart inside the last 15 min did not fix it, so this is not the link-flap fault — read journalctl -u dnsmasq and /etc/dnsmasq.d/network.conf for enable-ra and the ra-param lines"
      fi
      echo "$now" > "$stamp"
      ${pkgs.systemd}/bin/systemctl restart dnsmasq \
        || fail "no router advertisement for ''${window}s, and restarting dnsmasq failed — the box's DNS and DHCP are down too; journalctl -u dnsmasq"
      back=$(advertised "$recheck")
      if [ -n "$back" ]; then
        slack "gw: IPv6 router advertisements had stopped (nothing on $devs for ''${window}s) — the dead-RA-timer fault after a router-lan link flap. ra-watch restarted dnsmasq and they resumed on $back within ''${recheck}s, so no client should have lost its default route (lifetime 1200 s). machines/gw/ra-watch.nix"
        fail "no router advertisement left this box on $devs for ''${window}s; restarted dnsmasq and they resumed on $back within ''${recheck}s, so the LAN kept its IPv6. This is the router-lan link-flap fault (machines/gw/README.md) and the repair was automatic; the unit is red because the fault happened at all"
      fi
      slack "gw: no IPv6 router advertisement on $devs even after restarting dnsmasq — this is NOT the known link-flap fault and the LAN will lose off-link IPv6 within 1200 s. Check journalctl -u dnsmasq and /etc/dnsmasq.d/network.conf. machines/gw/ra-watch.nix"
      fail "no router advertisement on $devs even after a dnsmasq restart — check journalctl -u dnsmasq, that /etc/dnsmasq.d/network.conf still carries enable-ra and the ra-param lines, and that common/vlans.nix still gives those VLANs an ra range"
    }

    # A partial fault: at least one VLAN advertised and at least one did not.  Not
    # repaired here (a restart would not tell us anything a full one had not), and
    # red, because the VLAN that went quiet has lost its off-link IPv6.
    quiet=""
    for d in $devs; do
      case " $alive " in
        *" $d "*) : ;;
        *) quiet="$quiet $d" ;;
      esac
    done
    [ -z "$quiet" ] || {
      slack "gw: IPv6 router advertisements are missing on$quiet while $alive still advertises — off-link IPv6 on$quiet lapses within 1200 s. machines/gw/ra-watch.nix"
      fail "no router advertisement on$quiet for ''${window}s while $alive advertised — one VLAN's IPv6 gateway is going away even though dnsmasq is running"
    }

    exit 0
  '';
in {
  assertions = [
    {
      # The list is derived from the same table dnsmasq's `ra-param` lines come
      # from, so this cannot disagree with what is advertised; it can still be
      # empty, and an empty list would make the check above vacuous (the device
      # list would be empty and it would fail on that instead — this says why in a
      # sentence rather than at 03:00).
      assertion = devices != [];
      message = ''ra-watch.nix watches the VLANs that common/vlans.nix gives an `ra` range, and none has one: dnsmasq advertises nothing, so the watch has nothing to watch (and every client has no IPv6 default).'';
    }
  ];

  systemd.services.ra-watch = {
    description = "Check that gw is still advertising IPv6 router advertisements";
    # No `wantedBy`: the timer starts it.  Worth running by hand once
    # (`systemctl start ra-watch`), because a check that has never run is a check
    # whose message nobody has read.
    serviceConfig = {
      Type = "oneshot";
      # `+` for the whole script: it needs CAP_NET_RAW for the capture, and D-Bus
      # to restart dnsmasq.
      ExecStart = "+${watch}";
      # A full run is one window; the repair path adds the device/dnsmasq waits, a
      # restart and the recheck.
      TimeoutStartSec = "${toString (window + recheck + 240)}s";
    };
  };

  systemd.timers.ra-watch = {
    description = "Every few minutes, check gw is still advertising IPv6 routes";
    wantedBy = ["timers.target"];
    timerConfig = {
      # `OnUnitInactiveSec` alone, and it fires **immediately** on the activation that
      # creates it (a timer whose unit has never run has no "last inactive" to count
      # from, so the interval has already elapsed) — measured 2026-11-26 at 16:08:49.
      # At boot that is before networkd has made the VLANs and before dnsmasq is up,
      # which is why the script waits for both instead of reporting them, and why
      # there is no `OnBootSec` to pretend otherwise.
      OnUnitInactiveSec = "2min";
    };
  };
}
