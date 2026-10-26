# DATA — plain attrset read by `import` from anywhere in the tree; it is not a module and defines no options.
# Every device the router hands a *fixed* address to, keyed by the name it
# answers.  This is the DHCP reservation table: the old router generated
# `dhcp-host=<mac>,set:<tag>,<ipv4>,[<ipv6>],<name>` lines from its `<Interfaces>`
# XML, machines/gw/dns.nix generates the same lines from this table, and the
# addresses the flake's own machines live on are the ones in here (see
# ./hosts.nix, its hand-maintained mirror image: every entry here that is a flake
# machine must appear there and `machines/gw/lib/check-network-tables.nix` refuses the
# disagreement — nothing generates hosts.nix, so the two tables are kept in step by that
# check rather than by a build step).
#
# Why the MAC and not a name: the router cannot know a device's name, only its
# hardware address — the `ether` on the wire, or the DHCP client ID for the
# devices that never show a MAC.  A device that is *not* in here still gets an
# address, out of its VLAN's dynamic range (see ./vlans.nix); a device in here
# gets the same address forever, which is the whole point of the table.  The
# old box's `<Hostwatch>` section existed to notice when one of these MACs
# stopped matching — see machines/gw/README.md for what replaces it.
#
# Fields
#   vlan      the VLAN id (see ./vlans.nix) the lease is tagged with; null =
#             no VLAN, i.e. the old router answered with just an address and
#             nothing on it is routed by VLAN
#   ipv4      the address the device keeps
#   ipv6      the /64 addresses the *device itself* holds, and so the only
#             addresses its name may answer with (dnsmasq wants them in
#             [brackets]; machines/gw/dns.nix adds those).
#
#             **This field is a promise about the device, not about the
#             network, and since 2026-10-21 the network keeps none of it for
#             you.** The retired appliance honoured these as per-host static
#             DHCPv6 bindings — `dhcp-host=…,10.3.1.32,[2001:8b0:1d14:225::32],
#             [fd99:dead:beef:225d::32],aibox`, read out of its generated
#             dnsmasq.conf — with no `range6` behind them, and it additionally
#             NPT66'd `fd99:dead:beef:225d::/64` → `2001:8b0:1d14:225d::/64` and
#             `fd99:dead:beef:227d::/64` → `2001:8b0:1d14:227d::/64`, which is
#             why ten of these records name a `*d::/64` at all. `machines/gw`
#             renders the same `dhcp-host=` lines and implements *neither*
#             mechanism: its IPv6 ranges are `ra-only`, `option6:23,[::]` tells
#             clients there is no DHCPv6 server, and `nft list table ip6 nat`
#             on it holds nothing but tailscale's masquerade. So an entry here
#             is live only if the device configures the address itself —
#             `machines/services1/networking.nix` and
#             `machines/aibox/networking.nix` are the two that do — and every
#             other entry is a name answering with an address nobody owns, which
#             is a *hang* for a client that prefers IPv6 and has no fallback (a
#             PXE client: `machines/pi-room-sys/README.md`). `./lib/check-network-
#             tables.nix` refuses an address outside any /64 the network
#             advertises, which is the half of this it can prove at eval time;
#             "inside the prefix but owned by nobody" it cannot, and the test
#             for that is `ip -6 neigh show` on the router: `INCOMPLETE` means
#             the address is unowned. 2026-10-26: 12 records were emptied this
#             way (the ten PI-ROOM boards, `apps1`, `yunohost`), each saying so
#             at its own entry.
#   macs      hardware addresses, as they appear on the wire
#   clientIds DHCP client IDs, without the `id:` prefix
#
# The names are the devices' own, not DNS names: dnsmasq answers them as a bare
# query, and `dhcp-fqdn` makes the lease answer as `<name>.<zone>` as well.
# Keep them lowercase-with-hyphens: they end up in a hosts file and, for the
# PI-ROOM devices, in the devices' own boot menus.
{
  # --- VLAN 225: the wired LAN, 10.3.1.x ------------------------------------
  # The flake's own boxes first: their addresses are what common/hosts.nix
  # promises, so a change here moves the nginx proxies, the Prometheus scrape
  # targets and the gatus checks all at once.
  services1 = {
    vlan = 225;
    ipv4 = "10.3.1.20";
    ipv6 = [
      "2001:8b0:1d14:225::20"
      "fd99:dead:beef:225d::20"
    ];
    macs = [
      "c8:d3:ff:a5:b2:25" # the NIC that carries VLAN 225
      "c8:d3:ff:a5:be:7c" # renamed to `lan` in machines/services1/networking.nix
    ];
    clientIds = ["00:02:00:00:ab:11:e5:55:4a:3e:88:2f:66:5e"];
  };
  services2 = {
    vlan = 225;
    ipv4 = "10.3.1.21";
    ipv6 = [];
    macs = ["70:85:c2:53:f1:d4"];
    clientIds = [];
  };
  aibox = {
    vlan = 225;
    ipv4 = "10.3.1.32";
    ipv6 = [
      "2001:8b0:1d14:225::32"
      "fd99:dead:beef:225d::32"
    ];
    macs = ["84:47:09:40:d2:25"];
    clientIds = ["00:02:00:00:ab:11:96:35:cb:b3:58:98:28:b0"];
  };
  # The name the device answers to is `nas2`, the old box's own: common/hosts.nix
  # calls the same address `nas`, and the two names are the share's and the
  # device's.  Keep the old box's spelling here — it is what a device that
  # queries for itself gets back.
  nas2 = {
    vlan = 225;
    ipv4 = "10.3.1.6";
    ipv6 = [];
    macs = ["3c:a8:2a:a0:23:45"];
    clientIds = [];
  };
  authentik = {
    vlan = 225;
    ipv4 = "10.3.1.36";
    ipv6 = [];
    macs = ["aa:29:e1:d0:14:35"];
    clientIds = [];
  };
  # No IPv6: `2001:8b0:1d14:225::30`/`fd99:dead:beef:225d::30` were the old
  # appliance's static DHCPv6 binding for this VM, and apps1 does not configure
  # them — answering on IPv4 (10.3.1.30) while the router's neighbour table
  # reports both addresses `INCOMPLETE` (measured 2026-10-26). Re-add them only
  # together with configuring them on the VM.
  apps1 = {
    vlan = 225;
    ipv4 = "10.3.1.30";
    ipv6 = [];
    macs = ["92:99:29:be:af:95"];
    clientIds = ["00:02:00:00:ab:11:28:de:e0:0f:f2:ac:dc:1e"];
  };
  # The old router's own record is NOT in this table, and that is not an
  # omission: a reservation needs an identifier to match on, and the router has
  # no lease of its own — its address is VLAN 225's `ipv4` in common/vlans.nix
  # and the name common/hosts.nix calls `router`.  A record with no `macs` and
  # no `clientIds` matches nothing, which is what ./lib/check-network-tables.nix
  # refuses rather than letting it through as a silent dead line.
  nas1 = {
    vlan = 225;
    ipv4 = "10.3.1.5";
    ipv6 = [];
    macs = ["00:08:9b:ec:1b:d1"];
    clientIds = [];
  };
  linksys = {
    vlan = 225;
    ipv4 = "10.3.1.8";
    ipv6 = [];
    macs = ["58:ef:68:a5:e6:96"];
    clientIds = [];
  };
  zigbee1 = {
    vlan = 225;
    ipv4 = "10.3.1.9";
    ipv6 = [];
    macs = ["78:e3:6d:e4:05:fb"];
    clientIds = [];
  };
  # No IPv6, same reason as `apps1` and with the same measurement
  # (`2001:8b0:1d14:225::15`/`fd99:dead:beef:225d::15` `INCOMPLETE` on the
  # router while the appliance answers on 10.3.1.15) — and it is consistent
  # this way: `machines/gw/constants.nix` records that `yunohost.leighhack.org`
  # has never had a AAAA either, so the appliance was only ever reachable over
  # IPv6 to this one internal name, and nothing reached it there.
  yunohost = {
    vlan = 225;
    ipv4 = "10.3.1.15";
    ipv6 = [];
    macs = ["44:a8:42:3f:a1:ea"];
    clientIds = [
      "ff:42:3f:a1:ea:00:01:00:01:31:32:2a:a8:44:a8:42:3f:a1:ea"
    ];
  };
  Henry-iDrac = {
    vlan = 225;
    ipv4 = "10.3.1.16";
    ipv6 = [];
    macs = ["44:a8:42:3f:a1:ed"];
    clientIds = ["01:44:a8:42:3f:a1:ed"];
  };
  # The Wi-Fi access points that sit in the wired block because they were
  # installed on the wired run, not the wireless one.  Their names say which
  # part of the space each one serves; do not rename them to match VLAN 226.
  AP1-MainSpace = {
    vlan = 225;
    ipv4 = "10.3.1.41";
    ipv6 = [];
    macs = ["44:d9:e7:90:69:11"];
    clientIds = [];
  };
  AP2-PiRoom = {
    vlan = 225;
    ipv4 = "10.3.1.42";
    ipv6 = [];
    macs = ["78:8a:20:f0:db:24"];
    clientIds = [];
  };
  AP3-Fabrication = {
    vlan = 225;
    ipv4 = "10.3.1.43";
    ipv6 = [];
    macs = ["fc:ec:da:a6:4b:53"];
    clientIds = [];
  };
  AP4-Social = {
    vlan = 225;
    ipv4 = "10.3.1.44";
    ipv6 = [];
    macs = ["fc:ec:da:a6:26:43"];
    clientIds = [];
  };
  AP1-Fast = {
    vlan = 225;
    ipv4 = "10.3.1.45";
    ipv6 = [];
    macs = ["94:a6:7e:00:31:61"];
    clientIds = [];
  };

  # --- VLAN 226: the wireless block, 10.3.2.x -------------------------------
  SLZB-06M = {
    vlan = 226;
    ipv4 = "10.3.2.2";
    ipv6 = [];
    macs = ["78:e3:6d:e4:05:f8"];
    clientIds = [];
  };
  cam1 = {
    vlan = 226;
    ipv4 = "10.3.2.6";
    ipv6 = [];
    macs = ["10:a4:be:03:0d:36"];
    clientIds = [];
  };
  cam5 = {
    vlan = 226;
    ipv4 = "10.3.2.10";
    ipv6 = [];
    macs = ["3c:52:a1:48:fa:b2"];
    clientIds = [];
  };
  cam7 = {
    vlan = 226;
    ipv4 = "10.3.2.12";
    ipv6 = [];
    macs = ["3c:52:a1:49:24:0a"];
    clientIds = [];
  };
  sumup-solo = {
    vlan = 226;
    ipv4 = "10.3.2.15";
    ipv6 = [];
    macs = ["74:7a:90:23:8b:7b"];
    clientIds = [];
  };
  cam9 = {
    vlan = 226;
    ipv4 = "10.3.2.17";
    ipv6 = [];
    macs = ["3c:52:a1:49:1b:f7"];
    clientIds = [];
  };
  big-button = {
    vlan = 226;
    ipv4 = "10.3.2.21";
    ipv6 = [];
    macs = ["7c:2c:67:d2:e0:6c"];
    clientIds = [];
  };
  brother = {
    vlan = 226;
    ipv4 = "10.3.2.30";
    ipv6 = [];
    macs = ["00:41:0e:0d:00:3b"];
    clientIds = [];
  };
  voron = {
    vlan = 226;
    ipv4 = "10.3.2.50";
    ipv6 = [];
    macs = ["d8:3a:dd:12:cf:87"];
    clientIds = [];
  };
  elegoo = {
    vlan = 226;
    ipv4 = "10.3.2.55";
    ipv6 = [];
    macs = ["3c:22:7f:82:22:83"];
    clientIds = ["01:3c:22:7f:82:22:83"];
  };
  snapmaker = {
    vlan = 226;
    ipv4 = "10.3.2.56";
    ipv6 = [];
    macs = ["28:2d:06:1d:bc:96"];
    clientIds = [];
  };
  cnc-1 = {
    vlan = 226;
    ipv4 = "10.3.2.61";
    ipv6 = [];
    macs = ["e4:5f:01:c3:a0:c3"];
    clientIds = [];
  };
  led180 = {
    vlan = 226;
    ipv4 = "10.3.2.70";
    ipv6 = [];
    macs = ["50:78:7d:40:12:98"];
    clientIds = [];
  };
  led-pi-room = {
    vlan = 226;
    ipv4 = "10.3.2.71";
    ipv6 = [];
    macs = ["5c:cf:7f:c6:85:ed"];
    clientIds = [];
  };

  # --- VLAN 227: the workshop's own block, 10.3.14.x ------------------------
  workshop = {
    vlan = 227;
    ipv4 = "10.3.14.11";
    ipv6 = [];
    macs = ["74:3f:c2:55:ed:ea"];
    clientIds = [];
  };
  main_space = {
    vlan = 227;
    ipv4 = "10.3.14.12";
    ipv6 = [];
    macs = ["74:3f:c2:6d:4a:00"];
    clientIds = [];
  };
  woodwork = {
    vlan = 227;
    ipv4 = "10.3.14.13";
    ipv6 = [];
    macs = ["74:3f:c2:69:7d:5f"];
    clientIds = [];
  };
  cad-1 = {
    vlan = 227;
    ipv4 = "10.3.14.51";
    ipv6 = [];
    macs = ["30:9c:23:f4:2f:f0"];
    clientIds = [];
  };
  # The Pi-room's ten devices, in the order their own boot menus list them:
  # the address number is the room number, so 10.3.14.101 is PI-ROOM-01.
  #
  # None of them has an IPv6 address, and they never did in the sense this table
  # means. The old box bound each board to `2001:8b0:1d14:227d::10NN`, which is
  # not a prefix any interface of this network has ever had: `227d::/64` existed
  # only as the *destination* of an NPT66 rule (`fd99:dead:beef:227d::0/64` →
  # `2001:8b0:1d14:227d::0/64` on `opt3`, "Note the \"d\" prefix. Only map
  # addresses within the \"d\" range.", read out of the old config.xml alongside
  # the binding). With no NPT66 on gw the address exists nowhere and is not even
  # routable in-house — `ip -6 route get 2001:8b0:1d14:227d::1001` on the router
  # sends it out `dev ppp0`, i.e. a client dialling `PI-ROOM-01.int` over IPv6
  # hands its packets to the internet uplink. The boards' real IPv6 is SLAAC in
  # `2001:8b0:1d14:227::/64` and `fd99:dead:beef:227d::/64`, and nothing
  # consults it by name; their IPv4 is what `just switch-netboot` and every
  # monitor uses. `./lib/check-network-tables.nix` refuses these ten coming
  # back, because they are outside every advertised /64.
  "PI-ROOM-01" = {
    vlan = 227;
    ipv4 = "10.3.14.101";
    ipv6 = [];
    macs = ["0c:9d:92:c6:61:d0"];
    clientIds = ["00:01:00:01:2f:b6:81:fe:0c:9d:92:c6:61:d0"];
  };
  "PI-ROOM-02" = {
    vlan = 227;
    ipv4 = "10.3.14.102";
    ipv6 = [];
    macs = [
      "2c:fd:a1:6f:bd:6b"
      "2c:fd:a1:6f:ce:9c"
    ];
    clientIds = ["00:01:00:01:2f:b6:8f:be:2c:fd:a1:6f:ce:9c"];
  };
  "PI-ROOM-03" = {
    vlan = 227;
    ipv4 = "10.3.14.103";
    ipv6 = [];
    macs = ["18:31:bf:2a:8b:5a"];
    clientIds = ["00:01:00:01:2f:b6:9b:08:18:31:bf:2a:8b:5a"];
  };
  "PI-ROOM-04" = {
    vlan = 227;
    ipv4 = "10.3.14.104";
    ipv6 = [];
    macs = ["2c:fd:a1:6f:d0:62"];
    clientIds = ["00:01:00:01:2f:df:63:11:2c:fd:a1:6f:d0:62"];
  };
  "PI-ROOM-05" = {
    vlan = 227;
    ipv4 = "10.3.14.105";
    ipv6 = [];
    macs = ["2c:fd:a1:6f:cf:9a"];
    clientIds = ["00:01:00:01:2f:e4:96:e3:2c:fd:a1:6f:cf:9a"];
  };
  "PI-ROOM-06" = {
    vlan = 227;
    ipv4 = "10.3.14.106";
    ipv6 = [];
    macs = ["2c:fd:a1:6f:d0:10"];
    clientIds = ["00:01:00:01:2f:e4:94:a8:2c:fd:a1:6f:d0:10"];
  };
  "PI-ROOM-07" = {
    vlan = 227;
    ipv4 = "10.3.14.107";
    ipv6 = [];
    macs = ["2c:fd:a1:6f:c9:48"];
    clientIds = ["00:01:00:01:2f:e4:9a:6d:2c:fd:a1:6f:c9:48"];
  };
  "PI-ROOM-08" = {
    vlan = 227;
    ipv4 = "10.3.14.108";
    ipv6 = [];
    macs = ["2c:fd:a1:6f:cf:7a"];
    clientIds = ["00:01:00:01:2f:df:75:b8:2c:fd:a1:6f:cf:7a"];
  };
  "PI-ROOM-09" = {
    vlan = 227;
    ipv4 = "10.3.14.109";
    ipv6 = [];
    macs = ["70:8b:cd:ba:2a:44"];
    clientIds = ["00:01:00:01:2f:e4:9e:db:70:8b:cd:ba:2a:44"];
  };
  "PI-ROOM-10" = {
    vlan = 227;
    ipv4 = "10.3.14.110";
    ipv6 = [];
    macs = ["2c:fd:a1:6f:cb:90"];
    clientIds = ["00:01:00:01:30:aa:5f:cb:2c:fd:a1:6f:cb:90"];
  };
  # The Pi-room's own wired box, in the same block as the ten rooms.
  aibox-wired = {
    vlan = 227;
    ipv4 = "10.3.14.32";
    ipv6 = [];
    macs = [
      "84:47:09:40:d2:27"
      "84:47:09:40:d3:27"
    ];
    clientIds = [];
  };
  # The two printers.  Their fixed IPv4 is deliberate, not a quirk of this
  # table: the two Pis were cloned from one SD card, so their DNS names used to
  # advertise each other's addresses (docs/printer-servers.md).  Keep the
  # addresses, and do not "fix" the names.
  "3d-lime" = {
    vlan = 227;
    ipv4 = "10.3.14.61";
    ipv6 = [];
    macs = ["b8:27:eb:a8:fb:ff"];
    clientIds = [];
  };
  "3d-blue" = {
    vlan = 227;
    ipv4 = "10.3.14.62";
    ipv6 = [];
    macs = ["b8:27:eb:e7:7c:30"];
    clientIds = [];
  };

  # --- No VLAN: an address, and nothing else --------------------------------
  # The old router had no VLAN tag on this one, so nothing on it is routed by
  # VLAN and its lease is untagged.  Left in because removing it would move the
  # device into a VLAN's dynamic range and change its address.
  toshiba = {
    vlan = null;
    ipv4 = "10.3.1.50";
    ipv6 = [];
    macs = ["00:80:91:76:6c:0a"];
    clientIds = [];
  };
}
