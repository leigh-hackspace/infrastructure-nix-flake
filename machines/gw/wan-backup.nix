# The backup WAN: the USB Wi-Fi adapter — and, since 2026-11-08, a USB-tethered
# phone — that keeps the network on the internet when the AAISP path is dead, and
# the small amount of policy routing that makes it a *fallback* rather than a
# second primary.
#
# TWO PATHS, ONE POLICY.  The hotspot is 2.4 GHz only, so it caps the whole
# network at about 14 Mbit/s however good the phone's cellular link is
# (`docs/wan-backup-uplink.md`, "Why it is slow"), and its association came and
# went with the USB stick's radio rather than with the phone.  Plugging the phone
# in and switching USB tethering on removes the radio from the path: the phone
# presents an ordinary Ethernet netdev (RNDIS, CDC-ECM, or an iPhone's CDC-NCM)
# and serves DHCP over it.  So both are configured the same way and differ only in
# the preference order `./constants.nix` owns — cable first, radio second — and
# the route script below picks whichever is actually up with a lease.  Nothing
# here needs a phone to be present: with no cable the Wi-Fi path behaves exactly
# as it did on 2026-11-07.
#
# WHY THIS EXISTS.  On 2026-11-07 the AAISP tunnel went down and the hub's own
# uplink was dead behind a live link (`ping 192.168.20.1` answered,
# `ping -I router-uplink 1.1.1.1` did not), so the network had no way off it.
# A hand-built hotspot link had IPv4 working again inside an hour:
# `wpa_supplicant -c /run/wpa-backup.conf`, a `.network` file in
# `/run/systemd/network`, a default route typed at the console, a `masquerade`
# rule added with `nft`, and two `ip rule`s sending both VLAN marks at a table
# 300.  Every one of those lives in `/run` or in the live kernel, so a reboot
# would have ended the lifeline and a `switch` would have reloaded nftables and
# deleted the NAT rule out from under the LAN.  This module is that setup made
# declarative, plus the two things a hand-built one cannot do: bring the
# association back by itself, and follow the DHCP lease when the hotspot hands
# out a different subnet the next time it associates (measured on 2026-11-07:
# 10.112.100.0/24 at 13:48, 10.120.243.0/24 at 14:53 — the route typed at 13:11
# was stale by 14:53).
#
# THE POLICY, and why it is not simply "the wwan always wins": the wwan is a
# metered hotspot and should carry the network only when the primary path is not
# *carrying packets* — which is a different question from whether its link is up.
# That health is already observable without a new daemon: the tunnel's default
# exists in table 100 exactly while ppp0 is dialled, and while that session is
# dialled *over the hub* it is proof the hub works.  Since 2026-11-07 it can also
# be dialled over the wwan (below), and then it proves nothing about the hub, so
# the shortcut is used only when the L2TP endpoint's route is through the hub;
# otherwise `wan-backup-route` probes the hub through its own interface (three
# echoes, so one lost packet is not an outage).  Failover is one bad probe;
# returning to the primary takes two good ones, so a marginal link cannot flap
# the egress on every tick (a flap changes the NAT source address of every open
# connection).  So:
#
#   * tunnel dialled over the hub → nothing to do: VLAN 225 uses table 100, every
#                                  other VLAN the hub's table 200, wwan idle;
#   * tunnel down, hub answers   → the hub is the fallback (AAISP's LNS is the
#                                  broken part), wwan idle;
#   * tunnel down, hub silent    → `wan-backup-route` promotes the wwan's DHCP
#                                  default into the main table *and* table 200, so
#                                  both marks and the router's own traffic leave
#                                  by it — **and routes the L2TP endpoint out the
#                                  wwan too**, so xl2tpd's next redial carries the
#                                  AAISP session over the hotspot and the public
#                                  `/29`, the delegated `/48` and the tailnet
#                                  control plane keep working.  That is the only
#                                  local way to provide public services while the
#                                  hub is dead; it is also the only reason the
#                                  hotspot is expected to carry VLAN 225's traffic.
#
# WHAT IT DOES NOT DO: IPv6 and inbound public services **without the tunnel**.
# The hotspot alone hands out a link-local and nothing else, and `81.187.195.17/29`
# is routed to the AAISP line, so a network whose only egress is the hotspot has no
# IPv6 and no front door.  What restores both is the `/32` above: the session
# arrives through AAISP exactly as before, just carried over LTE instead of the
# hub.  It also takes no opinion about which VLANs may reach the internet (that is
# `./firewall.nix`'s ACL, and it is unchanged).
{
  config,
  lib,
  pkgs,
  INFRA,
  ...
}: let
  NICS = import ./constants.nix;

  # The backup paths in preference order (`./constants.nix` owns the list, because
  # `./firewall.nix` NATs on the same names): a USB-tethered phone first, the
  # Wi-Fi hotspot second.  Both are plain DHCP clients — the two `systemd.network`
  # files below — so whichever one holds a lease's default route is the fallback
  # egress and the other sits idle.  The cable is preferred because it is not a
  # 1×1 2.4 GHz radio: the same phone delivered 1.75 MB/s over the stick and the
  # hotspot is 2.4 GHz only, which is what made 2026-11-07 a network at 14 Mbit/s
  # (`docs/wan-backup-uplink.md`, "Why it is slow").
  wwans = NICS.WWAN_FALLBACK_NICS;

  # The Wi-Fi association is still run even when the cable is in: it costs nothing
  # while idle and it is the path that works when nobody has plugged a phone in.
  wwan = NICS.WWAN_NIC;

  # The SSID is a fact about the organisation's hotspot, not a secret; the
  # passphrase is `wan_backup_wifi_psk` in sops and never appears in this file.
  ssid = "Leighhack WAN Backup";

  # The metric the promoted fallback gets.  It has to beat the hub's static 300
  # in the main table and its 200 in table 200; the wwan devices' own DHCP routes
  # are left at 880 (USB) and 900 (Wi-Fi) by systemd-networkd and are therefore
  # never selected by themselves.
  fallbackMetric = 100;
  # The L2TP endpoint's static route through the hub (./networking.nix) is metric
  # 10; the fallback copy has to beat it, and be deleted to hand the endpoint back.
  l2tpMetric = 5;
  # The table `./networking.nix`'s MARK_HUB rule reads: every VLAN but 225.
  hubTable = 200;

  # How many consecutive 10 s ticks the *fallback* may spend not answering its
  # own gateway before this script resets that path (see "is the fallback actually
  # carrying the network?" below).  12 ticks is two minutes: long enough that a
  # re-association, a DHCP renew or one lost probe never triggers it, short enough
  # that a wedged link is repaired while somebody is still looking at the page.
  kickTicks = 12;

  # The decision, in four `ip` commands.  Shell rather than a `crates/` binary on
  # purpose: this is the same shape as `./uplink.nix`'s `ip-up`/`ip-down`, and
  # AGENTS.md rule 1 calls that small glue.  A daemon would have to do exactly
  # this on exactly the same events.
  routeScript = pkgs.writeShellScript "wan-backup-route" ''
    set -eu
    export PATH="${
      lib.makeBinPath [
        pkgs.coreutils
        pkgs.gnugrep
        pkgs.gnused
        pkgs.iproute2
        pkgs.iputils
        pkgs.systemd
      ]
    }"

    # A space-separated list, so `for iface in $wwans` below splits it: this is
    # `escapeShellArg` of the joined string, *not* `escapeShellArgs` of the list —
    # the latter renders `wwans=a b` and the shell then tries to run `b` as a
    # command.  That is not hypothetical: it is what failed the activation on
    # 2026-11-07 and left this unit dead while the network was on the link it
    # reconciles.
    wwans=${lib.escapeShellArg (lib.concatStringsSep " " wwans)}
    wifi_nic=${lib.escapeShellArg NICS.WWAN_NIC}
    hub=${lib.escapeShellArg NICS.UPLINK_NIC}
    endpoint=${lib.escapeShellArg NICS.L2TP_ENDPOINT}
    metric=${toString fallbackMetric}
    l2tp_metric=${toString l2tpMetric}
    table=${toString hubTable}
    kick_ticks=${toString kickTicks}
    state_file=/run/wan-backup/route.state
    kick_file=/run/wan-backup/fallback.failures

    mkdir -p /run/wan-backup

    # Delete only the routes this script installs, and delete them on *every*
    # candidate rather than on the one about to be used: the fallback can be the
    # cable on one tick and the hotspot on the next, and a promoted route left
    # behind on the interface that just went away keeps its traffic with a
    # gateway that is gone.  The devices' own DHCP defaults (systemd-networkd's,
    # at 880/900) and the hub's static defaults belong to somebody else, and
    # deleting those would remove the next run's inputs.  The loop variable is
    # not `dev`: `clear_ours` runs after the choice below, and clobbering `dev`
    # here would install the route on whichever candidate sorted last.
    clear_ours() {
      for iface in $wwans; do
        ip -4 route del default dev "$iface" metric "$metric" 2>/dev/null || true
        ip -4 route del default dev "$iface" table "$table" metric "$metric" 2>/dev/null || true
        ip -4 route del "$endpoint/32" dev "$iface" metric "$l2tp_metric" 2>/dev/null || true
      done
    }

    # Which backup path is actually carrying something, and through which gateway:
    # the first candidate holding a DHCP default wins, so the list order in
    # ./constants.nix is the preference (cable before radio).  The gateway is
    # systemd-networkd's, not ours — read the route its DHCP client installed so
    # that a lease which moves the hotspot to another /24 (measured on 2026-11-07:
    # 10.112.100.0/24 at 13:48, 10.120.243.0/24 at 14:53) or the phone's tether to
    # another subnet is followed instead of a hand-typed gateway going stale.
    # `proto dhcp` is what stops this reading back the very route this script
    # installs.
    dev=
    gateway=
    for candidate in $wwans; do
      g="$(ip -4 route show default dev "$candidate" proto dhcp 2>/dev/null \
        | sed -n 's/.* via \([0-9.]*\).*/\1/p' | head -n 1)"
      if [ -n "$g" ]; then
        dev=$candidate
        gateway=$g
        break
      fi
    done

    # Is the primary path *carrying packets*, which is not the same question as
    # whether its link is up — 2026-11-07 was a live hub with a dead upstream, and
    # the link state, the DHCP lease and the static route all looked healthy.
    #
    # A dialled tunnel is proof the hub works **only while it is dialled over the
    # hub**.  Since 2026-11-07 it can also be dialled over the wwan (`/32` below),
    # and then the tunnel says nothing about the hub at all: treating it as proof
    # would demote the wwan and send every non-225 VLAN into a dead hub.  So the
    # tunnel is a shortcut only when the L2TP endpoint's route is through the hub;
    # otherwise probe the hub through its own interface (SO_BINDTODEVICE, so
    # neither the wwan route nor anything else can answer): three echoes, because
    # one lost packet is not an outage and promoting the wwan on it changes the NAT
    # source address of every open connection.
    tunnel_up=0
    if ip -4 route show table 100 default 2>/dev/null | grep -q .; then tunnel_up=1; fi
    lns_dev="$(ip -4 route get "$endpoint" 2>/dev/null | sed -n 's/.* dev \([^ ]*\).*/\1/p' | head -n 1)"
    if [ -z "$lns_dev" ]; then lns_dev=none; fi
    if [ "$tunnel_up" = 1 ] && [ "$lns_dev" = "$hub" ]; then
      probe=up
    elif ping -c 3 -i 0.3 -W 1 -n -q -I "$hub" 1.1.1.1 >/dev/null 2>&1; then
      probe=up
    else
      probe=down
    fi

    # Hysteresis, asymmetric on purpose.  Failing over takes one bad probe: the
    # network is already losing packets on the primary, and another 10 s of it is
    # worse than a route change.  Failing *back* takes two consecutive good ones,
    # so a marginal link cannot make the egress flap on every tick.
    prev_state=""
    prev_streak=0
    if [ -r "$state_file" ]; then
      read -r prev_state prev_streak _ < "$state_file" || true
    fi
    if [ -z "$prev_streak" ]; then prev_streak=0; fi
    case "$prev_streak" in
      *[!0-9]*) prev_streak=0 ;;
    esac

    if [ "$probe" = up ]; then
      if [ "$prev_state" = wwan ]; then
        streak=$((prev_streak + 1))
        if [ "$streak" -ge 2 ]; then
          state=primary
          streak=0
        else
          state=wwan
        fi
      else
        state=primary
        streak=0
      fi
    else
      streak=0
      if [ -n "$gateway" ]; then state=wwan; else state=nowhere; fi
    fi

    # A wwan decision without a lease is no decision at all: a link is up (the
    # association, or the cable) but nothing has handed out an address yet, and
    # there is nothing to route to.  Say so rather than installing a route with an empty gateway.
    if [ "$state" = wwan ] && [ -z "$gateway" ]; then state=nowhere; fi

    if [ "$state" = wwan ]; then
      # Clear both candidates first.  `replace` matches on prefix and metric, not
      # on the device, so a swap from the hotspot to the cable would otherwise
      # leave the hotspot's promoted default in place for the length of one `ip`
      # call — and in table 200 with nothing under it to fall back on.
      clear_ours
      ip -4 route replace default via "$gateway" dev "$dev" metric "$metric"
      ip -4 route replace default via "$gateway" dev "$dev" table "$table" metric "$metric"
      # The L2TP endpoint follows the same egress, so xl2tpd's next dial goes out
      # the hotspot (or the phone) and the public /29, the delegated /48 and the
      # tailnet control plane keep their addresses.  Metric 5 beats the static hub
      # route's 10; deleting it hands the endpoint back to the hub, and the
      # session re-dials over it (the old session's packets change source and the
      # LNS drops it).
      ip -4 route replace "$endpoint/32" via "$gateway" dev "$dev" metric "$l2tp_metric"
      ip -4 route flush cache
      detail="$dev $gateway"
    else
      clear_ours
      detail=-
    fi

    # IS THE FALLBACK ACTUALLY CARRYING THE NETWORK?  A route is not a link, and
    # this script is the thing that was missing on 2026-11-07: it kept reporting
    # `egress 'wwan'` all morning while the 2.4 GHz association sat there losing
    # beacons (225 `CTRL-EVENT-BEACON-LOSS` events before lunch, `iw link` still
    # saying "Connected", RTT to the hotspot's own gateway 262 ms), so the LAN had
    # a default route with a dead radio behind it — box healthy, network offline,
    # and the only way back was a human running `wan-backup-emergency.sh`.  So once
    # a fallback is the egress it gets probed too, and a path that has not answered
    # for `kick_ticks` ticks is reset:
    #
    #   * **the gateway, not the internet.**  A phone with no cellular still answers
    #     its own tether/hotspot address, and restarting the association cannot mend
    #     that; a gateway that stops answering is the radio/driver/wedge case that a
    #     reset *does* mend.  It is one echo every 10 s, so it is free on a metered
    #     link.
    #   * **the fallback is *needed* only when the hub is not usable**, which is
    #     `wwan` (chosen but not passing traffic) or `nowhere` with the hub probe
    #     down (nothing to choose, which is where a wedged supplicant in the BSSID
    #     ignore-list loop leaves the box).  While the primary is healthy nothing
    #     here fires, so a deliberately-off hotspot is never probed or reset.
    #   * the reset is the same one the runbook does by hand: restart the
    #     association for the stick, bounce the link and re-run DHCP for the cable.
    need_fallback=0
    if [ "$state" = wwan ]; then need_fallback=1; fi
    if [ "$state" = nowhere ] && [ "$probe" = down ]; then need_fallback=1; fi

    kick=0
    if [ -r "$kick_file" ]; then
      kick="$(cat "$kick_file" 2>/dev/null || echo 0)"
    fi
    case "$kick" in
      [0-9]*) : ;;
      *) kick=0 ;;
    esac

    if [ "$need_fallback" = 1 ]; then
      if [ -n "$gateway" ] && ping -c 1 -W 1 -n -q -I "$dev" "$gateway" >/dev/null 2>&1; then
        kick=0
      else
        kick=$((kick + 1))
      fi
    else
      kick=0
    fi

    if [ "$kick" -ge "$kick_ticks" ]; then
      echo "wan-backup: '$dev' has not answered its gateway (''${gateway:--}) for $kick ticks while the hub is unusable — resetting that path"
      if [ "$dev" = "$wifi_nic" ] || [ -z "$dev" ]; then
        systemctl restart wan-backup-wifi.service || true
      else
        ip link set "$dev" down || true
        sleep 1
        ip link set "$dev" up || true
        networkctl reconfigure "$dev" || true
      fi
      kick=0
    fi
    printf '%s\n' "$kick" > "$kick_file"

    # Log on transitions only: the timer runs every 10 s, and a line every tick is
    # a journal nobody reads.  The state file is in /run, so the first run after a
    # boot always speaks.
    new="$state $streak $detail"
    if [ "$new" != "$(cat "$state_file" 2>/dev/null || true)" ]; then
      printf '%s\n' "$new" > "$state_file"
      echo "wan-backup: egress '$state' (hub probe $probe, recovery streak $streak, via $detail, L2TP path $lns_dev)"
    fi
  '';
in {
  # The association.  wpa_supplicant is the one part of this that is not
  # configurable by declaring a file — it holds a session — so it runs in the
  # foreground under systemd (`-B` would daemonise away from the supervisor) and
  # the never-give-up policy covers both "the stick is out" and "the hotspot is
  # off": the process exits or its start fails, systemd retries forever, and the
  # link comes back when the AP does.
  systemd.services.wan-backup-wifi = INFRA.mkNeverGiveUp {
    description = "Backup WAN Wi-Fi association (${wwan} → ${ssid})";
    wantedBy = ["multi-user.target"];
    # Ordering only: the unit may start before USB enumeration has created the
    # link, and that is fine — its restart loop is the retry.
    after = ["sys-subsystem-net-devices-${wwan}.device"];
    path = [
      pkgs.coreutils
      pkgs.gnugrep
      pkgs.gnused
      pkgs.procps
      pkgs.wpa_supplicant
    ];
    preStart = ''
      set -eu
      mkdir -p /run/wan-backup
      chmod 0700 /run/wan-backup

      # One interface, one supplicant.  A `wpa_supplicant` started by hand on this
      # interface holds the driver, and a second copy cannot associate with it —
      # stop it here rather than failing every 10 s until someone notices.  This is
      # also what took the box over from the hand-built 2026-11-07 setup.
      pkill -f "wpa_supplicant.*${wwan}" || true

      # The passphrase comes from sops at start and is turned into the PMK
      # wpa_supplicant actually keys with.  `wpa_passphrase` reads it on stdin when
      # it is not given on the command line, so it appears neither in the store nor
      # in a process listing; the derived file is mode 0600 under /run.
      umask 077
      psk="$(cat /run/secrets/wan_backup_wifi_psk)"
      pmk="$(printf '%s\n' "$psk" | wpa_passphrase ${lib.escapeShellArg ssid} | sed -n 's/^[[:space:]]*psk=//p')"

      cat > /run/wan-backup/wpa.conf <<EOF
      country=GB
      network={
        ssid="${ssid}"
        psk=$pmk
        key_mgmt=WPA-PSK
        scan_ssid=1
      }
      EOF
    '';
    serviceConfig = {
      Type = "simple";
      # No `-f`: its default is stdout, which is the journal.  `-f` would write a
      # file nobody reads and lose the association's own diagnostics.
      ExecStart = "${pkgs.wpa_supplicant}/bin/wpa_supplicant -i ${wwan} -c /run/wan-backup/wpa.conf -D nl80211,wext";
      RestartSec = "10s";
    };
  };

  # The Wi-Fi backup path's address.  `UseRoutes = true` is load-bearing and is
  # the opposite of what the hand-built `/run` file did: the default has to come
  # from the DHCP server, because the hotspot moves between /24s, and a gateway
  # written down is a gateway that stops matching.  Its metric keeps it out of the
  # way — 900 is worse than every other default on this box — and
  # `wan-backup-route` is what promotes it, only when nothing better is usable.
  # No DNS: the network's resolver is dnsmasq on this box, and a hotspot's idea of
  # a resolver must not become a second authority (`./dns.nix` says the same about
  # the hub).
  systemd.network.networks."20-wwan" = {
    matchConfig.Name = wwan;
    networkConfig = {
      DHCP = "ipv4";
      IPv6AcceptRA = false;
    };
    dhcpV4Config = {
      UseDNS = false;
      UseRoutes = true;
      RouteMetric = 900;
    };
  };

  # The USB-tethered phone's address, the same shape and the same reasons: DHCP
  # from the phone, which hands out a private /24 with itself as the gateway
  # (Android: 192.168.42.0/24; an iPhone: 172.20.10.x — either way it is the
  # server's, so nothing here writes it down), no DNS, and a metric one step
  # better than the Wi-Fi's 900 so that the kernel's own choice agrees with the
  # script's preference order when both paths are up.  880 still loses to the
  # hub's static 300 and to the promoted 100, so it is never selected by itself.
  systemd.network.networks."21-wwan-usb" = {
    matchConfig.Name = NICS.WWAN_USB_NIC;
    networkConfig = {
      DHCP = "ipv4";
      IPv6AcceptRA = false;
    };
    dhcpV4Config = {
      UseDNS = false;
      UseRoutes = true;
      RouteMetric = 880;
    };
  };

  # The policy.  A oneshot, not a daemon: its whole job is to read the current
  # state and reconcile two routes, so a 10 s timer is both the failover latency
  # and the self-healing mechanism (a lease that moved the hotspot to another
  # subnet is corrected on the next tick, and a route deleted by hand comes back).
  systemd.services.wan-backup-route = {
    description = "Reconcile the backup WAN's default routes";
    wantedBy = ["multi-user.target"];
    after = ["systemd-networkd.service" "wan-backup-wifi.service"];
    wants = ["systemd-networkd.service"];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = routeScript;
    };
  };

  systemd.timers.wan-backup-route = {
    description = "Keep the backup WAN's routes following the primary path";
    wantedBy = ["timers.target"];
    timerConfig = {
      OnBootSec = "20s";
      OnUnitActiveSec = "10s";
      AccuracySec = "1s";
      Unit = "wan-backup-route.service";
    };
  };

  # BREAK GLASS.  The script is in the generation so that it exists at a stable
  # path on the box (`/etc/wan-backup-emergency.sh`) without depending on the
  # repository checkout, and so that the same bytes are reviewed as the config
  # they are a fallback for.  It is deliberately imperative: it has to work when
  # the switch, these units, or networkd's generated config is what broke.  Read
  # its header before changing it, and keep it executable — `source` copies the
  # mode of the file in this repository.
  environment.etc."wan-backup-emergency.sh" = {
    source = ./wan-backup-emergency.sh;
    mode = "0755";
  };

  # The supplicant in the system profile, for two reasons: the break-glass script
  # needs `wpa_supplicant` + `wpa_passphrase` to exist somewhere that is not the
  # unit's own store path, and a console debug of the association should not need
  # a `nix-shell` (which with no internet is exactly when it is needed).  It is
  # already in the closure through the unit above; this only links it into
  # /run/current-system/sw/bin.  `iw` is here for the same reason: reading the
  # association's band, signal and negotiated rate (`iw dev <iface> link`) is the
  # first question when the backup is slow, and `wpa_cli` needs a ctrl socket the
  # foreground supplicant is deliberately not given.
  environment.systemPackages = [pkgs.wpa_supplicant pkgs.iw];
}
