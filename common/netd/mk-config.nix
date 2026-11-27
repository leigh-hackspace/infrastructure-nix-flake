# HELPER — a function of the network's tables returning netd's configuration as a Nix value.
#
# This is the whole "Nix config, no friction" boundary: the daemon reads one JSON
# document and decides nothing about the network, and this file decides nothing
# about the daemon — it projects the tables that already exist
# (`common/vlans.nix`, `common/dns-records.nix`, `common/hosts.nix`,
# `common/int-dns.nix`) into the schema `crates/netd/src/config/` defines.
#
# The translation rules live in `machines/gw/lib/network-tables.nix`, not here: a
# VLAN device name, a tag and a lease length are spellings two renderers have to
# agree about (`machines/gw/dns.nix` still renders dnsmasq's conf from the same
# file), so this file takes `tables` as an argument rather than importing it.
#
#     (import ../../common/netd/mk-config.nix) {
#       tables = import ./lib/network-tables.nix;
#       vlans = import ../../common/vlans.nix;
#       records = import ../../common/dns-records.nix;
#       hosts = import ../../common/hosts.nix;
#       int = import ../../common/int-dns.nix;
#       zone = CONFIG.LOCAL_DOMAIN;
#       ...
#     }
#
# What it deliberately does **not** do: validate. `crates/netd` owns the rules
# (a reservation inside a pool, a tag that matches nothing, two devices on one
# MAC) and reports every finding at build time via `netd check`; a second copy
# here would be a second source of truth about what a valid config is. What this
# file does guarantee is that nothing in the tables is *dropped* on the way
# through — `common/netd/check-config.nix` compares the counts.
{
  tables,
  vlans,
  records,
  hosts,
  int,
  zone,
  # The upstreams, in the order dnsmasq's `strict-order` asked them
  # (`machines/gw/dns.nix`'s four, read off the box's rendered conf on
  # 2026-11-27).
  upstreams,
  # Suffix forwards: the tailnet's own resolver answers `ts.leighhack.org`, and
  # only that name may go there.
  conditional ? [],
  apiBind,
  apiPort ? 8092,
  stateDir ? "/var/lib/netd",
  # Addresses inside a dynamic range that must never be handed out although no
  # reservation can name them. `10.3.1.166` is the intercom: a *second* address
  # on services1's vlan225, held by the NIC that is already reserved as `.20`,
  # and inside VLAN 225's .150-.253 pool (`machines/gw/dns.nix` has the story).
  ignoredAddresses ? [],
  ignoredNames ? ["wpad"],
  # The VLAN whose access points need UniFi's DHCP vendor option 43, and the
  # label in `common/hosts.nix` whose address is the controller they are told to
  # dial. One VLAN, because a vendor blob in every other VLAN's lease is noise —
  # the APs are all reserved on 225.
  unifiController ? null,
  unifiVlan ? 225,
  # The TFTP server a PXE client fetches a netboot file from — aibox, not the
  # router (`machines/aibox/netboot.nix`); the file name a client is given is
  # rendered from `tables.netbootVariants`.
  tftpServer ? hosts.aibox,
  leaseTtl ? tables.leaseTtl,
}: let
  # The network's shared constants: the tailnet prefixes are here so the DNS
  # access list names them once, in the table that owns them.
  CONSTANTS = import ../config.nix;

  named = builtins.mapAttrs (name: r: r // {inherit name;}) records;

  # Order-preserving dedup: a VLAN's `ipv6` and its `ra` ranges are in the same
  # /64, and a prefix written twice is a fact written twice.
  uniq = builtins.foldl' (seen: x:
    if builtins.elem x seen
    then seen
    else seen ++ [x]) [];

  # --- DHCP ---------------------------------------------------------------
  # One pool per VLAN that has a dynamic range. `server_ip` is the VLAN's own
  # address, which is what makes the default router and DNS option the address
  # the client's cable actually reaches.
  pools =
    map (v: {
      interface = tables.vlanDevice v;
      tag = tables.vlanDevice v;
      from = v.dynFrom;
      to = v.dynTo;
      netmask = "255.255.255.0";
      server_ip = v.ipv4;
      ttl = leaseTtl;
    })
    (builtins.filter (v: v.dynFrom != null) vlans);

  # One reservation per record, keyed on every identifier the device answers
  # with. dnsmasq refused two lines for one address; here the schema refuses two
  # reservations for one address, and the identifiers are a list rather than a
  # comma-separated field — so the fault class cannot come back.
  reservations =
    map (r: {
      name = r.name;
      address = r.ipv4;
      macs = r.macs;
      client_ids = r.clientIds;
    })
    (builtins.attrValues named);

  # The DNS server each VLAN's clients are told, tagged by interface. netd also
  # defaults option 6 to the pool's own address; this row is the table's version
  # of the same fact (`machines/gw/dns.nix`'s generated `dhcp-option` lines) and
  # costs nothing to keep exact.
  dnsOptions =
    map (v: {
      tag = tables.vlanDevice v;
      code = 6;
      value = [v.ipv4];
    })
    vlans;

  # One dotted quad's octet as two lowercase hex digits, without `lib`: this file
  # is evaluated on its own by `just netd-render` (`nix eval -f`), which has no
  # nixpkgs, and that is the same reason
  # `machines/gw/lib/network-tables.nix` is builtins-only.
  hexDigits = ["0" "1" "2" "3" "4" "5" "6" "7" "8" "9" "a" "b" "c" "d" "e" "f"];
  hexByte = s: let
    n = builtins.fromJSON s;
  in
    builtins.elemAt hexDigits (n / 16) + builtins.elemAt hexDigits (n - (n / 16) * 16);

  # `builtins.split` returns the separators as lists; the strings are the parts.
  dotted = s: builtins.filter builtins.isString (builtins.split "\\." s);

  # Option 43 for a UniFi AP: sub-option 1, four octets of the controller —
  # `01:04:0a:03:01:14` for 10.3.1.20. The encoding belongs here rather than in
  # the daemon because it is a vendor constant of this table's controller.
  unifiOption =
    if unifiController == null
    then []
    else [
      {
        tag = tables.vlanDevice unifiVlan;
        code = 43;
        hex = "01:04:" + builtins.concatStringsSep ":" (map hexByte (dotted hosts.${unifiController}));
      }
    ];

  # The Pi Room's boot menu, from the one table `machines/gw/dns.nix` renders its
  # `dhcp-match`/`dhcp-boot` lines from. `when_tags` is dnsmasq's `tag:vlan227`
  # half, and the server is the table's `tftpServer` — a *client* fetches from
  # it, so the router never needs a TFTP server of its own.
  netboot =
    map (b: {
      tag = b.tag;
      match_option = b.option;
      match_values = b.values or [];
      match_contains = b.contains or null;
      when_tags = [(tables.vlanDevice b.vlan)];
      boot = b.boot;
      server = tftpServer;
    })
    tables.netbootVariants;

  # --- DNS ----------------------------------------------------------------
  #
  # Every name is written twice: once fully qualified, once as the bare label.
  # That is not duplication, it is what dnsmasq's hosts lines did — two names on
  # one line — and it is why `ping services1` and `ping services1.int…` both work
  # without making a bare name forwardable.
  qualify = name: ["${name}.${zone}" name];

  recordNames = map (r: {
    addresses = [r.ipv4] ++ r.ipv6;
    names = qualify r.name;
  }) (builtins.attrValues named);

  # The front door's names, at the door's addresses (three of them: IPv4, public
  # IPv6, ULA — read from the table that owns the box's address, so a name
  # missing its IPv6 half is not expressible).
  doorAddresses = tables.doorAddresses records hosts vlans int.frontDoor;
  doorNames =
    map (n: {
      addresses = doorAddresses;
      names = qualify n;
    }) (int.names
      ++ builtins.attrNames int.legacy);

  # Names that answer with their own `common/hosts.nix` address: the switches,
  # which are not DHCP clients and so have no reservation to hang a record on.
  hostNames =
    map (n: {
      addresses = [hosts.${n}];
      names = qualify n;
    })
    int.fromHosts;

  # Names dialled by a *socket* rather than a browser (the MQTT broker's 1883):
  # answered with the box that runs it, and deliberately not front-door rows.
  socketNames = map (n: {
    addresses = [hosts.${int.answeredBy.${n}.via}];
    names = qualify n;
  }) (builtins.attrNames int.answeredBy);

  dnsRecords =
    builtins.concatLists
    (map (
      e:
        map (name: {
          inherit name;
          inherit (e) addresses;
        })
        e.names
    ) (recordNames ++ doorNames ++ hostNames ++ socketNames));

  # The sources a query is answered from: this box's own networks, by address
  # rather than by interface. That is what makes a spoofed source arriving on the
  # WAN refused, which an interface filter cannot see.
  acl = uniq (
    ["127.0.0.0/8"]
    ++ map (v: v.net) vlans
    ++ [CONSTANTS.TAILNET_V4]
    ++ builtins.concatLists (map (v: map tables.v6Block64 (v.ipv6 ++ map (r: r.from) v.ra)) vlans)
    ++ [CONSTANTS.TAILNET_V6]
  );

  # --- router advertisements ----------------------------------------------
  #
  # One advertisement per VLAN that has an `ra` range, carrying that range's
  # /64s. A VLAN with `ipv6` but no `ra` is not advertised (VLAN 222 and 1337
  # have neither), which is the same thing the generated `ra-param` lines said.
  raInterfaces =
    map (v: {
      interface = tables.vlanDevice v;
      prefixes = uniq (map (r: tables.v6Block64 r.from) v.ra);
      router_lifetime = 1200;
      # dnsmasq's `ra-only` range took both lifetimes from the range's lease
      # time; the network has therefore always given clients a day. Keep it.
      valid_lifetime = leaseTtl;
      preferred_lifetime = leaseTtl;
      mtu = null;
    })
    (builtins.filter (v: v.ra != []) vlans);
in {
  version = 1;
  zone = zone;
  state_dir = stateDir;

  api = {
    bind = apiBind;
    port = apiPort;
  };

  dns = {
    listen = [
      "0.0.0.0:53"
      "[::]:53"
    ];
    inherit acl;
    records = dnsRecords;
    forwarders = upstreams;
    inherit conditional;
    cache_size = 10000;
    ttl = 1;
    forward_max = 5000;
    domain_needed = true;
    stop_rebind = true;
    rebind_ok = ["ts.leighhack.org"];
    rebind_localhost_ok = true;
    soa = {
      mname = "gw.${zone}";
      rname = "hostmaster.${zone}";
      refresh = 3600;
      retry = 600;
      expire = 604800;
      minimum = 10;
    };
  };

  dhcp = {
    interfaces = map tables.vlanDevice vlans;
    lease_ttl = leaseTtl;
    decline_quarantine = 600;
    ignored_addresses = ignoredAddresses;
    ignored_names = ignoredNames;
    inherit pools reservations netboot;
    options = dnsOptions ++ unifiOption;
  };

  ra = {
    interval = 60;
    interfaces = raInterfaces;
  };
}
