# The router's DNS and DHCP: dnsmasq, generated from the network's own tables
# (common/vlans.nix, common/dns-records.nix) rather than written out.  This is
# the flake's version of the file OPNsense generated into `/usr/local/etc/
# dnsmasq.conf` from its `<Interfaces>` XML — same generator, different source
# of truth, and the reason the XML is not in this repo.
#
# Read the two tables first: common/vlans.nix says which VLANs exist and which
# block each one's dynamic range hands out of; common/dns-records.nix says
# which device keeps which address.  Everything below turns those two facts into
# dnsmasq's own syntax, so a device's address cannot drift from what the flake
# promises without ./lib/check-network-tables.nix saying so.
#
# The three things a lease is made of, in the order dnsmasq assembles them:
#   1. an identifier — the device's MAC, or its DHCP client ID for the devices
#      that never show a MAC (common/dns-records.nix's `macs`/`clientIds`)
#   2. an address — either the fixed one in common/dns-records.nix, or the next
#      free one in the VLAN's dynamic range (common/vlans.nix's dynFrom/dynTo)
#   3. a tag — the VLAN the request arrived on, which is what ./firewall.nix
#      routes on.  dnsmasq tags a lease with the interface name by itself, and
#      `vlan<id>` is that name (see common/vlans.nix's header on FreeBSD's
#      `vlan0.<id>` vs Linux's `vlan<id>`), so the tag is generated here rather
#      than configured per VLAN.
#
# The generation itself is in ./lib/network-tables.nix, not in this file: the
# same translation is read by ./firewall.nix (nftables' rules) and asserted over
# by ./lib/check-network-tables.nix, and a VLAN tag spelled one way in dnsmasq's
# conf and another way in nftables' rules is a device that gets an address and no
# routing.  What is in here is dnsmasq's *options*, which is the part that is not
# generated and so the part an operator can read.
{
  config,
  lib,
  pkgs,
  INFRA,
  ...
}: let
  HOSTS = import ../../common/hosts.nix;
  VLANS = import ../../common/vlans.nix;
  RECORDS = import ../../common/dns-records.nix;
  TABLES = import ./lib/network-tables.nix;

  # The interfaces dnsmasq answers on: the VLANs, and nothing else.  The uplink
  # is deliberately not in the list — the old box's generated conf listed every
  # interface OPNsense knew about, including the WAN, because it was generated
  # from `<Interfaces>` wholesale, and a point-to-point uplink carries no DHCP
  # requests so the extra line never mattered.  Answering DHCP *towards* the
  # internet is not a behaviour worth inheriting.
  #
  # One `interface=` line each, not the old box's single comma-separated line:
  # dnsmasq's own `-I` syntax reads a comma as "this interface's address is …", so
  # a comma list is one interface with a garbage address rather than several
  # interfaces.  The old box generated the comma form and dnsmasq answered from it
  # because the VLANs' addresses come from the interfaces' own configuration rather
  # than from this line — the flake's form is the one that says the same thing
  # without depending on that.
  #
  # No `bind-interfaces`/`bind-dynamic` alongside this: dnsmasq's default binds the
  # wildcard and filters by interface, which is what lets it start before
  # systemd-networkd has finished creating the VLAN devices.  With either bind
  # option, a VLAN device that does not exist yet is a fatal error at startup and
  # the never-give-up policy turns it into a restart loop.
  interfaces = map TABLES.vlanDevice VLANS;
in {
  # No TFTP here, on purpose, and it is worth a paragraph because the generated
  # `dhcp-boot` lines below name `/etc/tftp/undionly.kpxe` and look like they do.
  # The TFTP server is **aibox**: `machines/aibox/netboot.nix` runs
  # `in.tftpd -l -a 10.3.1.32:69 … /etc/tftp`, and those three files
  # (`ipxe.efi`, `undionly.kpxe`, `autoexec.ipxe`) are its `/etc/tftp`.  The
  # bootfile name in a `dhcp-boot` line is what the *client* asks the *named
  # server* for, so the path belongs to aibox and the old box's conf named aibox as
  # the server for exactly that reason — the flake inherits both halves together
  # (./lib/network-tables.nix's `netbootLines`, with the server from
  # common/hosts.nix's `aibox`).
  #
  # Copying aibox's three `environment.etc` entries here would be the mistake this
  # comment exists to prevent: two boxes with an `/etc/tftp` and one of them not
  # serving it.  If the router ever takes over TFTP, the change is those three files
  # *plus* dnsmasq's `enable-tftp`/`tftp-root` (or an `in.tftpd` unit) — and the
  # server address in the generated lines moves with them, which is why it is read
  # from the table rather than written in either place.
  environment.etc = {
    # The generated records file, and the file dns-sync maintains for the
    # `*.int.leighhack.org` vhost names (see ./README.md: dns-sync's half of the
    # old box's contract, and the one path that has to move with the router).
    "dnsmasq-records.hosts".text = TABLES.recordsHosts RECORDS + "\n";

    # The generated conf file: the VLAN ranges, the reservations, the netboot
    # variants.  One file, three generated blocks, in the order dnsmasq reads
    # them.
    "dnsmasq.d/network.conf".source = pkgs.writeText "network.conf" ''
      # GENERATED by machines/gw/dns.nix — edit common/vlans.nix or
      # common/dns-records.nix instead, never this file.  The old box generated
      # the same shape of file into /usr/local/etc/dnsmasq.conf from its <Interfaces>
      # XML, and the rule "never edit the generated file" is inherited with it.

      # VLAN dynamic ranges and router-advertisement ranges
      ${TABLES.rangeLines VLANS}

      # Fixed reservations: which device keeps which address
      ${TABLES.hostLines RECORDS}

      # Netboot variants (the PI-ROOM devices' boot menus)
      ${TABLES.netbootLines HOSTS.aibox}
    '';
  };

  # dnsmasq's own state: the leases it has handed out, and the directory both
  # it and the file dns-sync maintains live in.  The empty `names.hosts` is
  # dnsmasq's own failure mode: a missing addn-hosts file makes dnsmasq exit,
  # and with the never-give-up policy that is an endless restart loop rather
  # than a visible error, so the file is created empty on a fresh box and dns-sync
  # fills it.
  systemd.tmpfiles.rules = [
    "d /var/lib/dnsmasq 0755 root root -"
    "f /var/lib/dnsmasq/dnsmasq.leases 0640 root root -"
    "f /var/lib/dnsmasq/names.hosts 0644 root root -"
  ];

  services.dnsmasq = {
    enable = true;

    # nixpkgs' own resolvconf stub would point the box at a DNS server that is
    # not this one; the router's resolution comes from the VLAN network files in
    # ./networking.nix (DNS = the VLAN's own address) and from
    # networking.extraHosts.  Same reason as on the other machines' configs.
    resolveLocalQueries = false;

    settings = {
      # --- the interfaces it answers on --------------------------------------
      interface = interfaces;

      # --- names ------------------------------------------------------------
      # The zone the network's own names live in, and the wildcard that makes
      # every `*.int.leighhack.org` answer here rather than going out to the
      # public DNS (which is where the `*.leighhack.org` names live, in
      # DigitalOcean — see docs/router-and-dns.md and just dns-sync-*).
      domain = "int.leighhack.org";
      local = "/int.leighhack.org/";

      # The upstreams, and why they are written here rather than read from
      # /etc/resolv.conf: this box *is* the network's resolver, its own resolv.conf
      # points at itself (the VLANs' `DNS=` in ./networking.nix), and a dnsmasq that
      # forwards to the address in its own resolv.conf is a forwarding loop — the
      # query arrives back at the process that sent it.  `no-resolv` is what closes
      # the loop, and it is also the reason the box's own name resolution can be the
      # VLANs' address without surprising anyone.
      #
      # The two plain addresses are the old box's own upstreams.  The
      # `/ts.leighhack.org/<addr>` form is not an upstream at all but a conditional
      # forward — one suffix to the tailnet's own resolver — and it is the reason the
      # `rebind-domain-ok` exception below exists.  Its address is a placeholder: it
      # is a tailnet address, and ./README.md says how to read the real one off the
      # old box rather than trusting the number here.
      #
      # A list, because nixpkgs' dnsmasq module makes `server` a real option (a list
      # of strings) rather than a freeform key, and a string here is a type error at
      # eval.
      no-resolv = true;
      server = [
        "1.1.1.1"
        "8.8.8.8"
        "/ts.leighhack.org/100.100.100.100"
      ];
      rebind-domain-ok = "/ts.leighhack.org/";

      addn-hosts = [
        "/etc/dnsmasq-records.hosts"
        "/var/lib/dnsmasq/names.hosts"
      ];

      # --- DHCP behaviour ----------------------------------------------------
      # A lease dnsmasq hands out is the network's answer, not a suggestion: a
      # client that asks for a different address gets the network's.  This is
      # the option that makes common/dns-records.nix a *guarantee* rather than a
      # default, and the old box ran it for the same reason.
      dhcp-authoritative = true;
      # Answer a query for a device name with `<name>.<zone>` as well as the
      # bare name, so the records in common/dns-records.nix answer the same
      # query the vhost names do.
      dhcp-fqdn = true;
      # Do not hand out a lease for `wpad`: every Windows device on the network
      # asks for it, and answering makes a phantom device appear in the router's
      # leases (and in ./lib/check-network-tables.nix's address-unique check).
      dhcp-name-match = "set:wpad-ignore,wpad";
      dhcp-ignore-names = "tag:wpad-ignore";

      # --- IPv6 --------------------------------------------------------------
      # The network's IPv6 is advertised, not leased: the addresses that matter
      # are the fixed ones in common/dns-records.nix and the RA ranges in
      # common/vlans.nix, and `option6:23,[::]` is dnsmasq's own way of saying
      # "there is no DHCPv6 server here" so a client that asks does not sit and
      # wait for one.  The per-VLAN `ra-only` ranges are in the generated conf.
      #
      # And the DNS every client is offered: the VLAN's own router address, one
      # tagged line per VLAN, generated from common/vlans.nix — a client on the
      # wireless VLAN being sent at the wired LAN's gateway would get a lease it
      # cannot use, and hand-writing five lines that say "the interface's own
      # address" is how that happens.
      dhcp-option = (TABLES.dnsOptionLines VLANS) ++ ["option6:23,[::]"];
      enable-ra = true;

      # --- DNS serving -------------------------------------------------------
      # The old box's own values, and they are about the network's size rather
      # than dnsmasq's defaults: a hundred devices, a Prometheus scrape that
      # resolves names on every sample, and a short TTL for the network's own
      # answers so a renamed device is picked up without a reboot.
      dns-forward-max = 5000;
      cache-size = 10000;
      local-ttl = 1;
      strict-order = true;
      no-ident = true;
      # Never forward a name that is not fully qualified: a device's typo should
      # not be sent to the network's upstream resolvers (and the search domains
      # this box happens to have must not decide what a client's query means).
      domain-needed = true;
      rebind-localhost-ok = true;
      stop-dns-rebind = true;

      # --- the generated tables ----------------------------------------------
      # Written as one conf file rather than a few hundred options in the
      # settings attrset because that is the file an operator reads when a
      # device gets the wrong address: `cat /etc/dnsmasq.conf` has to show the
      # reservation, not a pointer into a Nix file.  A list, because nixpkgs'
      # module sets `conf-file` as a list itself (its own default is the
      # resolvconf stub's file) and two lists merge rather than replace.
      conf-file = ["/etc/dnsmasq.d/network.conf"];
    };
  };

  # The router answers for the whole network, so it must not give up: dnsmasq
  # is the thing every other machine's DNS and every lease depends on, and the
  # network's own "never give up" policy (common/systemd.nix) is what keeps it
  # up after a power cut.  The old box's `wait-for-nas` equivalent is the
  # uplink: dnsmasq can answer from the records whether or not the uplink is up,
  # so nothing here waits for it.  `alwaysKeepRunning` is the module's own half
  # of that (Restart=always); the other half — the start rate limit, which lives
  # on the unit rather than in serviceConfig — is what INFRA's override adds,
  # and the two halves living apart is the trap common/systemd.nix documents.
  services.dnsmasq.alwaysKeepRunning = true;

  # The unit half.  One assignment rather than an assignment plus
  # `systemd.services.dnsmasq.after = …`: the two forms are the same attribute in
  # one attrset, which Nix rejects before the module system ever sees it.
  #
  # `after`/`wants` on networkd because the VLAN devices are what dnsmasq answers
  # on: it binds the wildcard and filters by name, so it does not need them to
  # exist, but a DHCP socket for an interface that is not there yet is a lease that
  # cannot be answered, and the restart policy below is what would notice.
  systemd.services.dnsmasq =
    INFRA.mkNeverGiveUpOverride
    // {
      after = ["systemd-networkd.service"];
      wants = ["systemd-networkd.service"];
    };
}
