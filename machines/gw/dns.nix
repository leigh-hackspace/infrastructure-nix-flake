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
  CONFIG = import ../../common/config.nix;
  HOSTS = import ../../common/hosts.nix;
  VLANS = import ../../common/vlans.nix;
  RECORDS = import ../../common/dns-records.nix;
  INT = import ../../common/int-dns.nix;
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
  # The VLAN devices plus the tailnet interface.  `tailscale0` is not a LAN: it
  # is there because the old box answered on it (`interface=...,tailscale0` in
  # its generated /usr/local/etc/dnsmasq.conf, read 2026-10-17) and because the
  # tailnet is how the network's names are reached when the LAN is what is
  # broken.  The unit starts whether or not tailscaled has made the device —
  # dnsmasq binds the wildcard and filters by name, so a missing device is not a
  # startup failure.
  interfaces = map TABLES.vlanDevice VLANS ++ ["tailscale0"];

  # The zone this box is authoritative for, in the one place. It appears three
  # times below (dnsmasq's `domain`, its `local`, and the suffix on every generated
  # hosts entry) and those three have to agree exactly: `local=` without the suffix
  # on the records is the failure mode — the box refuses to forward the name *and*
  # has no entry for it, so `*.int.leighhack.org` answers nothing rather than
  # answering wrong. Measured on the box, 2026-10-17, and
  # ./lib/check-network-tables.nix now refuses the hosts file that would cause it.
  zone = CONFIG.LOCAL_DOMAIN;
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
    # The device records: every reservation's address against its name, both bare
    # and qualified.
    "dnsmasq-records.hosts".text = TABLES.recordsHosts zone RECORDS + "\n";

    # The front door's records: every `*.int.leighhack.org` vhost name against the
    # address the flake serves it from (../../common/int-dns.nix).  This file is
    # why there is no sync tool: it is a store path, so the deploy that adds a
    # vhost adds the record, and `restartTriggers` below is the half that makes
    # dnsmasq notice.
    #
    # It used to be `/var/lib/dnsmasq/names.hosts` — 183 lines edited over ssh by
    # `crates/dns-sync`, owned by no deployment.  Audited on 2026-10-22 against the
    # deployed nginx (which serves 34 `*.int` names, all 34 recorded, so nothing was
    # broken that day) and the finding was not a missing answer, it was that nothing
    # could *produce* the file: 46 of its lines were device records duplicating
    # `common/dns-records.nix` under names restored from the old box, 3 named
    # devices that are in no table at all (switch1/2/3), and the rest were the
    # vhost names — present because someone had remembered to run `just
    # dns-sync-sync`, which is a command, not a guarantee.  A machine rebuilt from
    # this flake answered a different `*.int` zone than the one running, and no
    # amount of `just switch` on the router would have told anyone.
    "dnsmasq-int-vhosts.hosts".text = TABLES.intHosts zone RECORDS HOSTS INT + "\n";

    # The generated conf file: the VLAN ranges, the reservations, the netboot
    # variants.  One file, three generated blocks, in the order dnsmasq reads them.
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

  # dnsmasq's own state: the leases it has handed out, and the directory both it
  # and the retired dns-sync's file live in.  Both `addn-hosts` files this
  # generation reads are store paths, so the failure the `f` rules used to guard —
  # a missing addn-hosts file makes dnsmasq exit, and the never-give-up policy
  # turns that into an endless restart loop rather than a visible error — cannot
  # happen from a record file any more than from a missing /etc/hosts.
  #
  # `names.hosts` is kept, and deliberately untouched, for exactly one deploy.  It
  # is no longer in `addn-hosts` above, so nothing reads it; it still exists,
  # content and all, because `system.autoRollback` can put the *previous*
  # generation back within five minutes and that generation's conf does name it.
  # Delete it in the same window and a rollback is a dnsmasq that exits, on the
  # box that answers DNS for the network.  Removing the rule and the file together
  # is the follow-up in docs/follow-ups.md, and it belongs in a deploy after the
  # one that made the file dead.
  systemd.tmpfiles.rules = [
    "d /var/lib/dnsmasq 0755 root root -"
    "f /var/lib/dnsmasq/dnsmasq.leases 0640 root root -"
    "f /var/lib/dnsmasq/names.hosts 0644 root root -"
  ];

  # --- who owns port 53 on this box: dnsmasq, and nothing else ----------------
  #
  # Measured on the box's first boot (2026-10-17), and the reason this block exists:
  # `networking.useNetworkd` enables systemd-resolved, resolved binds its stub to
  # 127.0.0.53:53, dnsmasq binds the wildcard and filters by interface (see
  # `interfaces` above), and the two cannot coexist — dnsmasq died at startup with
  #
  #   dnsmasq: failed to create listening socket for port 53: Address already in use
  #
  # exited 2, and the never-give-up policy retried it forever.  The box looked
  # healthy: `systemctl is-system-running` said `running`, because a router that
  # serves no DNS has no failed unit.
  #
  # The alternative was to make dnsmasq bind narrowly (`bind-interfaces`), which is
  # the wrong way round twice over: a VLAN device networkd has not created yet is a
  # fatal error for dnsmasq with that option (the comment above explains why it is
  # not set), so it trades a hard failure for a flaky one; and a stub resolver in
  # front of the network's authoritative DNS is a second cache with its own
  # behaviour, on the one box whose answer has to be the network's.
  #
  # So resolved is off, openresolv's resolvconf is off (its module asserts when
  # /etc/resolv.conf is managed by anything else), and the resolver file is written
  # here to point the box at its own dnsmasq.  `local-service` above is what makes
  # loopback an acceptable source, and `no-resolv` is what stops this being a loop:
  # dnsmasq's forwarders are its own `server=` lines and it never reads this file.
  services.resolved.enable = false;
  networking.resolvconf.enable = false;
  # No `search` line, deliberately: this box answers for `int.leighhack.org` and a
  # search domain of its own zone on the box that *is* that zone's authority turns
  # one typo into a second query that answers.  Everything the box resolves by name
  # is an FQDN, and its own name comes from networking.extraHosts.
  environment.etc."resolv.conf".text = ''
    # GENERATED by machines/gw/dns.nix.  This box is the network's resolver, so it
    # resolves through itself; see that file's header for why resolved is off.
    nameserver 127.0.0.1
    options edns0 trust-ad
  '';

  services.dnsmasq = {
    enable = true;

    # The same decision as the block above, from dnsmasq's side: it must not try to
    # make itself the system resolver (there is no resolved/resolvconf left to make
    # it the system resolver through), and the box's own resolution comes from
    # environment.etc."resolv.conf" just above and from networking.extraHosts.
    resolveLocalQueries = false;

    settings = {
      # --- the interfaces it answers on --------------------------------------
      interface = interfaces;

      # --- names ------------------------------------------------------------
      # The zone the network's own names live in, and the wildcard that makes
      # every `*.int.leighhack.org` answer here rather than going out to the
      # public DNS (which is where the `*.leighhack.org` names live, in
      # DigitalOcean — see docs/router-and-dns.md).
      domain = zone;
      local = "/${zone}/";

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
      # `rebind-domain-ok` exception below exists.  The address is the old box's own,
      # read from its generated dnsmasq.conf on 2026-10-17; it is a tailnet address,
      # so re-read it from the box rather than trusting it forever.
      #
      # A list, because nixpkgs' dnsmasq module makes `server` a real option (a list
      # of strings) rather than a freeform key, and a string here is a type error at
      # eval.
      # The four plain addresses are the old box's own upstreams, in the order its
      # `/conf/config.xml` listed them (`<dnsserver>`: 9.9.9.9, 8.8.8.8, 1.1.1.1,
      # 8.8.4.4 — four rather than two because this is the only resolver the whole
      # network has and dnsmasq marks a server failing after two timeouts and keeps
      # asking the others).  What is *not* inherited is the old box's
      # `dnsallowoverride`, which let the uplink's DHCP hand back resolvers: this
      # box's uplink is the mill's handoff, and a resolver chosen by someone
      # else's DHCP server is a second authority over what the network resolves.
      no-resolv = true;
      server = [
        "9.9.9.9"
        "8.8.8.8"
        "1.1.1.1"
        "8.8.4.4"
        "/ts.leighhack.org/100.100.100.100"
      ];
      rebind-domain-ok = "/ts.leighhack.org/";

      addn-hosts = [
        "/etc/dnsmasq-records.hosts"
        "/etc/dnsmasq-int-vhosts.hosts"
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
      dhcp-option = TABLES.dnsOptionLines VLANS ++ ["option6:23,[::]"];

      # Router advertisements on every VLAN that has `ra` ranges in
      # common/vlans.nix (the generated `ra-only`/`ra-param` lines are in the conf
      # above).  A router advertisement *is* a default gateway announcement, so
      # this box having it switched off while a second router lived here was the
      # difference between a shadow and a network where every client picked a
      # gateway at random; there is no second router any more.
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
  #
  # `restartTriggers` because dnsmasq reads every one of these files **at
  # startup**, and nixpkgs' module restarts the unit only when `/etc/hosts` changes
  # or when `settings` changes (its ExecStart is `-C <store path>`, so an option
  # change moves the store path and systemd does the rest — but a *file under /etc*
  # that an option merely names is invisible to it).  Without this, a switch that
  # moved a device's address or added a vhost wrote the new file, exited 0, and
  # went on answering from the old one — hit for real on 2026-10-17, where the
  # records file was newer than the process and the fix did not take effect until
  # dnsmasq was restarted by hand.  A store path is the right trigger for a
  # generated file: content changes, path changes, unit restarts.
  #
  # That is now all three of them, which is the shape this box wants: there is no
  # mutable DNS left on it.  The file dns-sync used to edit, and `systemctl reload`
  # after each of its whole-file rewrites, is `/etc/dnsmasq-int-vhosts.hosts` — a
  # store path — so the record and the restart arrive in the same activation and
  # nothing on the network has to remember to run a tool.
  systemd.services.dnsmasq =
    INFRA.mkNeverGiveUpOverride
    // {
      after = ["systemd-networkd.service"];
      wants = ["systemd-networkd.service"];
      restartTriggers = [
        config.environment.etc."dnsmasq-records.hosts".source
        config.environment.etc."dnsmasq-int-vhosts.hosts".source
        config.environment.etc."dnsmasq.d/network.conf".source
      ];
    };
}
