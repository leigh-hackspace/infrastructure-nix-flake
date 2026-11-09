# The router's WAN supervisor: `crates/wan-manager`.
#
# This file used to be the Wi-Fi association, one DHCP file per backup path and a
# 10-second reconciler written as a shell script. It was replaced on 2026-11-09
# because every one of those facts was a name in the configuration: the Wi-Fi
# interface, the USB netdev, the DHCP metric, the hub's gateway. The new rule is
# in the task that created it — **the firewall and this module contain no WAN
# name, address or gateway at all**. Discovery supplies them.
#
# The daemon and what it measures/repairs: `docs/wan-manager-2026-11-09.md`.
# The break-glass script is still here and is still imperative; it has to work
# when this unit, networkd, nftables or the switch is what broke.
{
  config,
  lib,
  pkgs,
  CRANE,
  INFRA,
  ...
}: let
  VLANS = import ../../common/vlans.nix;
  CONFIG = import ../../common/config.nix;
  NICS = import ./constants.nix;

  wanManager = CRANE.crate {
    pname = "wan-manager";
    version = "0.1.0";
    member = "wan-manager";
  };

  # The one WAN *identity* in the flake, not a WAN interface: the SSID every
  # wireless candidate is matched against. It is an organisation name, like a
  # domain; the passphrase is `wan_backup_wifi_psk` in sops and never appears
  # here. Any BSSID advertising it is attempted.
  ssid = "Leighhack WAN Backup";

  # The network's own blocks. These are not WAN facts: `--lan-prefix` is what
  # tells discovery "an address in here is LAN-side", and `--nat-source` is what
  # the daemon uses if the firewall's set-based NAT rule is missing. Both are
  # generated from the same tables the firewall reads.
  lanPrefixes = lib.unique (
    (map (v: v.net) VLANS)
    ++ [
      "192.168.2.0/24"
      CONFIG.TAILNET_V4
    ]
  );

  natSources = lib.unique (
    (map (v: v.net) VLANS)
    ++ [
      "192.168.2.0/24"
      CONFIG.TAILNET_V4
    ]
  );

  args =
    [
      "--lan-interface"
      NICS.LAN_NIC
      "--l2tp-endpoint"
      NICS.L2TP_ENDPOINT
      "--wifi-ssid"
      ssid
      "--psk-file"
      "/run/secrets/wan_backup_wifi_psk"
      "--state-dir"
      "/run/wan-manager"
      "--tunnel-interface"
      NICS.UPLINK_TUNNEL
      "--tunnel-table"
      "100"
      "--hub-table"
      "200"
      "--fallback-metric"
      "100"
      "--l2tp-metric"
      "5"
      "--ping-target"
      "1.1.1.1"
      "--ping-target"
      "8.8.8.8"
      "--probe-url"
      "https://one.one.one.one/"
      "--interval"
      "5"
      "--throughput-interval"
      "60"
      "--wifi-scan-interval"
      "300"
      "--kick-ticks"
      "12"
      # Both directions of the failover decision get a duration (2026-11-09).
      # `--failover-ticks` is the grace an incumbent that is still link-up but
      # has stopped answering keeps the seat: 3 ticks = 15 s, which is the
      # difference between a lost probe and an outage. A link that loses carrier
      # is still failed over on the first tick. `--failback-dwell` is how long a
      # challenger must stay clearly better before it takes a healthy
      # incumbent's seat — failing *back* is the voluntary move, and every one of
      # them rewrites the NAT source of every open connection.
      "--failover-ticks"
      "3"
      "--failback-dwell"
      "600"
      "--failback-margin"
      "1.5"
      # When the egress does move, pppd's L2TP socket is still bound to the old
      # path's source address and the session is stranded until six missed LCP
      # echoes (60 s). Redial the session instead, once the new route has
      # settled, and never in a loop.
      "--tunnel-unit"
      config.systemd.services.xl2tpd-aaisp.name
      "--tunnel-redial"
      "10"
      "--tunnel-redial-cooldown"
      "300"
      # The declared relay path (./wan-relay.nix): a point-to-point tunnel to the
      # box that can enumerate the phone. Declared, not discovered — an ipip
      # netdev has no `device` symlink and offers no DHCP gateway, so nothing in
      # the discovery loop can see it. Probes and ranking then treat it like any
      # other path; the only difference is that it has no gateway to ping.
      "--relay-candidate"
      "wan-aibox"
    ]
    ++ lib.concatMap (p: ["--lan-prefix" p]) lanPrefixes
    ++ lib.concatMap (p: ["--nat-source" p]) natSources;
in {
  # `systemd.networkd` reads its `.network` files from `/run/systemd/network` on
  # reload. The daemon writes the per-interface files there, which needs the
  # directory to exist *before* systemd builds the service's mount namespace:
  # a `ReadWritePaths` entry for a missing path fails the whole start with
  # 226/NAMESPACE (measured 2026-11-09). tmpfiles creates it at boot and at
  # activation, before `multi-user.target`.
  systemd.tmpfiles.rules = [
    "d /run/systemd/network 0755 root root - -"
  ];

  # The daemon. `Type=notify` + `WatchdogSec` is the enforced half of "can never
  # fail to start and recovers even when wedged": a process that stops petting
  # the watchdog is killed and restarted by systemd, and `StartLimitIntervalSec=0`
  # means there is no number of restarts after which the router gives up.
  systemd.services.wan-manager = INFRA.mkNeverGiveUp {
    description = "WAN supervisor: discover, rank and continuously repair every uplink";
    wantedBy = ["multi-user.target"];
    after = ["systemd-networkd.service" "systemd-udevd.service"];
    wants = ["systemd-networkd.service"];
    # Before the tunnel dials and before anything waits for `network-online`:
    # the daemon is what gives a newly-plugged uplink a route.
    before = ["network-online.target" "xl2tpd-aaisp.service"];
    path = [
      pkgs.coreutils
      pkgs.curl
      pkgs.gnugrep
      pkgs.gnused
      pkgs.iproute2
      pkgs.iputils
      pkgs.iw
      pkgs.nftables
      pkgs.procps
      pkgs.psmisc
      pkgs.systemd
      pkgs.wpa_supplicant
    ];
    # Before the daemon's first (slow, radio-scanning) tick, keep the LAN NATed
    # on whatever is currently carrying the default route. A switch reloads
    # nftables and empties `wan_ifaces`; without this there is a window between
    # that reload and the daemon's first apply where forwarded traffic has no
    # NAT. The daemon replaces this membership on its next tick. Every command
    # is best-effort: this must never be the reason a start fails.
    preStart = ''
      mkdir -p /run/systemd/network || true
      for dev in $(ip -4 route show default 2>/dev/null | sed -n 's/.* dev \([^ ]*\).*/\1/p' | sort -u); do
        nft add element ip routerNat wan_ifaces "{ \"$dev\" }" 2>/dev/null || true
        nft add element ip routerIn wan_ifaces "{ \"$dev\" }" 2>/dev/null || true
        nft add element ip6 router6 wan_ifaces "{ \"$dev\" }" 2>/dev/null || true
      done
    '';
    serviceConfig = {
      Type = "notify";
      NotifyAccess = "main";
      # The daemon pets this between the *steps* of a tick, not once per tick:
      # one tick probes every candidate serially with `ping -c 3 -W 2` before it
      # falls back to an HTTP verdict, so a tick with two dead paths legitimately
      # takes ~45 s (measured 2026-11-09: 35-50 s on this box, and the daemon was
      # killed and restarted every ~90 s while doing exactly its job). 45 s is
      # therefore the budget for **one step** — a wedge inside a probe, a spec, a
      # `networkctl reconfigure` — and `util::run` bounds every child anyway.
      WatchdogSec = "45s";
      RestartSec = "2s";
      TimeoutStartSec = "120s";
      # It writes /run and reads the sops-rendered passphrase; it does not need
      # a login, a home or a writable store. `RuntimeDirectory` is load-bearing:
      # `ReadWritePaths` naming a missing directory makes systemd fail the
      # namespace with 226/NAMESPACE before the daemon can create it (measured
      # 2026-11-09, the first deploy). The rest of /run is already writable under
      # `ProtectSystem=strict`; the daemon creates /run/systemd/network itself.
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      NoNewPrivileges = true;
      RuntimeDirectory = "wan-manager";
      RuntimeDirectoryMode = "0755";
      # A restart must not take the page's last answer with it. systemd removes
      # `RuntimeDirectory` when the unit stops, so every watchdog restart or
      # crash wiped `/run/wan-manager/status.json` and `/api/wan` answered 503
      # for the ~40 s the first tick takes to rewrite it — which is what the
      # Uplink tab read as "WAN supervisor status unavailable" on 2026-11-09
      # while the daemon was running perfectly. `restart` keeps it across a
      # restart and still clears it on a deliberate stop, which is the honest
      # boundary: a status file left by a daemon nobody is running is not news.
      RuntimeDirectoryPreserve = "restart";
      # systemd's `ProtectSystem=strict` makes /run read-only except for the
      # directories a unit explicitly asks to write. The status file lives in
      # RuntimeDirectory; the networkd drop-ins need this one line, or `publish`
      # silently fails and the box has no DHCP file for a new uplink after a
      # reboot.
      ReadWritePaths = ["/run/systemd/network"];
      ExecStart = "${lib.getExe' wanManager "wan-manager"} ${lib.escapeShellArgs args}";
    };
  };

  # The USB side of the same job. `rndis_host` and `cdc_ether` are what Android
  # uses; `cdc_ncm`/`ipheth` are the iPhone and newer Android paths. They are
  # built into this kernel already (measured 2026-11-08), but loading them at
  # boot removes one "the driver was not bound when the cable arrived" branch —
  # and the phone currently never reaches driver binding at all, because the
  # kernel cannot read its descriptors (see `docs/wan-manager-2026-11-09.md`,
  # "The phone's greyed-out checkbox"). The kernel parameters are the documented
  # EHCI compatibility switches for the `-71`/EPROTO descriptor failure: old
  # scheme first, and no autosuspend on a port that powers a phone.
  boot.kernelModules = [
    "usbnet"
    "rndis_host"
    "cdc_ether"
    "cdc_ncm"
    "ipheth"
    # Any number of USB Wi-Fi adapters, of any brand, may be plugged in. The
    # daemon discovers every wireless netdev (`/sys/class/net/*/wireless`) and
    # runs one supplicant per interface, so there is no "the" adapter here. The
    # in-tree drivers below are the common Ralink/Realtek/MediaTek/Atheros/Zydas
    # USB parts; each is also auto-loaded by modalias on hotplug, but loading
    # them at boot removes the window where an adapter is present and has no
    # driver yet. A brand whose driver is out-of-tree needs its own package and
    # is a deliberate addition, not a name this list can guess.
    "rt2800usb"
    "rtl8xxxu"
    "rtl8192cu"
    "rtw88_8821cu"
    "rtw88_8822bu"
    "rtw89_8852bu"
    "mt7601u"
    "mt76x0u"
    "mt76x2u"
    "ath9k_htc"
    "carl9170"
    "zd1211rw"
  ];
  boot.kernelParams = [
    "usbcore.old_scheme_first=1"
    "usbcore.autosuspend=-1"
  ];
  # The firmware blobs those adapters need. A USB Wi-Fi stick that enumerates but
  # cannot bring its radio up is usually missing a blob, not unsupported.
  # `enableAllFirmware` is deliberately not used: it pulls the whole historical
  # firmware archive (including non-Linux blobs and a Broadcom source tarball)
  # onto the router. The redistributable set is the Linux and regulatory one.
  hardware.enableRedistributableFirmware = true;
  hardware.firmware = [pkgs.linux-firmware pkgs.wireless-regdb];

  # The break-glass script is installed at a stable path in the generation, so it
  # exists when the checkout, the switch or this service is what broke.
  environment.etc."wan-backup-emergency.sh" = {
    source = ./wan-backup-emergency.sh;
    mode = "0755";
  };

  # Tools one expects on a Linux server, and the USB ones are the point: `lsusb`
  # was missing on this box because `common/tools.nix` is deliberately not
  # imported here (its wait-for-network unit reads the NAS). The rest are the
  # diagnostic verbs this router's own incidents keep needing: `ethtool` for a
  # NIC's negotiated speed, `usb-devices`/`lsusb -t` for the phone, `uhubctl`
  # for a port power cycle, `socat`/`nc` for a socket, `mtr`/`traceroute` for a
  # path, `iftop`/`iotop` for what is moving.
  environment.systemPackages = [
    wanManager
    pkgs.iproute2
    pkgs.iputils
    pkgs.curl
    pkgs.wget
    pkgs.usbutils
    pkgs.pciutils
    pkgs.usb-modeswitch
    pkgs.uhubctl
    pkgs.ethtool
    pkgs.socat
    pkgs.nettools
    pkgs.inetutils
    pkgs.mtr
    pkgs.traceroute
    pkgs.nmap
    pkgs.iftop
    pkgs.iotop
    pkgs.lsof
    pkgs.htop
    pkgs.wireguard-tools
    pkgs.psmisc
    pkgs.procps
    pkgs.file
    pkgs.less
    pkgs.tree
    pkgs.rsync
    pkgs.screen
    pkgs.strace
    pkgs.iw
    pkgs.wpa_supplicant
    pkgs.android-tools
  ];

  # The old reconciler's heartbeat check is gone with the old reconciler. The
  # daemon's own liveness is now the watchdog + `wan_manager_ticks` in
  # `metrics.prom`; a stopped daemon is a red unit, not a quiet one.
}
