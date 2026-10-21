# The VLANs this network's LAN traffic is tagged with, and the address block
# each one carries.  Network-wide on purpose, like ./hosts.nix: the VLAN id is
# what a *client* sees — its DHCP lease is handed out on it, the router routes
# its traffic by it, and a box that tags its own traffic (see
# machines/services1/networking.nix) asks for one by id — so the table lives
# here rather than in the router's machine directory, and the router, the
# reservations in ./dns-records.nix and the firewall's interface sets all read
# the same one.
#
# This is the flake's replacement for OPNsense's `<Interfaces>` section: the
# VLANs the old router created in its XML, with the same ids and the same
# address blocks, so a client's lease does not change when the router does.
#
# ONE NAMING TRAP, and it is the reason this file keys on the id rather than the
# device name: FreeBSD (the old router) spells the resulting device `vlan0.<id>`
# and dnsmasq's `tag:` ranges used that spelling, because dnsmasq takes a VLAN
# tag from the *device name*; systemd-netd spells the same VLAN `vlan<id>`.
# Anything that compares VLAN names across the two has to say which spelling it
# means.  machines/gw/dns.nix uses the Linux spelling throughout.
#
# Fields
#   id       the VLAN id on the wire (1..4095: the Linux limit; the old box
#            allowed 1..1023, which every id here satisfies)
#   ipv4     the address the router answers *on this VLAN* (…​.1): what a
#            client's gateway and DNS records point at while it is on it
#   net      the /24 the VLAN lives in — the block its dynamic DHCP range hands
#            out of, and the block common/config.nix's LOCAL_NETWORK ACL has to
#            allow (the check module asserts it, so the two halves cannot drift)
#   dynFrom  first address of the VLAN's dynamic DHCP range inside `net`
#   dynTo    last address of that range; null = the VLAN has no dynamic range,
#            so nothing arriving on it gets a lease and only the fixed
#            reservations in ./dns-records.nix answer
#   ipv6     the /64 addresses the router answers on the VLAN (its own: …​::1)
#   ra       prefixes the router advertises by router-advertisement, for other
#            devices on the VLAN to take addresses within (dnsmasq `ra-only`),
#            as {from,to} pairs — the range dnsmasq advertises, not a bare
#            prefix, because a prefix written twice is a range guessed twice
#   purpose  what the VLAN is for, in the words OPNsense's own `description`
#            used
[
  {
    id = 225;
    ipv4 = "10.3.1.1";
    net = "10.3.1.0/24";
    dynFrom = "10.3.1.150";
    dynTo = "10.3.1.253";
    # The router's own IPv6, by the network's own convention (`<vlan>::<n>`
    # with n = the last part of the block address, so the router at 10.3.1.1 is
    # …::1 — services1 is …::20).  The old box answered IPv6 on this VLAN from
    # AAISP's delegated prefix; these two are what the flake answers, and the
    # ranges it *advertises* are the `ra` ones below.
    ipv6 = [
      "2001:8b0:1d14:225::1"
      "fd99:dead:beef:225d::1"
    ];
    ra = [
      {
        from = "2001:8b0:1d14:225::1000";
        to = "2001:8b0:1d14:225::2000";
      }
      {
        from = "fd99:dead:beef:225d::1000";
        to = "fd99:dead:beef:225d::2000";
      }
    ];
    purpose = ''
      Wired LAN: the boxes, the ProCurve switches, the NAS.  Every flake
      machine lives in this block, so this is the VLAN the network's fixed
      addresses are reserved on (see ./dns-records.nix and ./hosts.nix).
    '';
  }
  {
    id = 226;
    ipv4 = "10.3.2.1";
    net = "10.3.2.0/24";
    dynFrom = "10.3.2.100";
    dynTo = "10.3.2.253";
    # The router's own address in the /64 this VLAN advertises.
    #
    # This was empty ("the old box answered IPv4-only on this VLAN") while the
    # plan was for networkd to take it from AAISP's DHCPv6-PD delegation.  It is
    # written down now because PD has never been observed to work on this box
    # (docs/gw-cutover-attempt-2026-10-21.md), and an empty `ipv6` next to a
    # non-empty `ra` is the worst of the two failure modes: dnsmasq advertises
    # `2001:8b0:1d14:226::/64` and the router has no address in it, so every
    # client on this VLAN configures a public address it cannot reach a gateway
    # in — the comment in machines/gw/networking.nix spells that out, and on a
    # phone it looks like the internet is down.
    #
    # Static is no new assumption: VLAN 225 below has carried a static address
    # out of the same /48 since the box existed, which only works because AAISP
    # routes 2001:8b0:1d14::/48 to the line.  It also agrees with what PD would
    # hand out — systemd's `SubnetId` is hexadecimal, so `226` is 0x226, which
    # is the old box's `sla-id 550` (decimal) in the archived
    # `ppp-ipv6.txt` — so this stays correct if PD starts working.
    ipv6 = ["2001:8b0:1d14:226::1"];
    # The old box advertised this VLAN's prefix with a SLAAC constructor
    # (`dhcp-range=…,constructor:vlan0.226,slaac,64`) rather than an `ra-only`
    # range, i.e. it let clients derive their address from the VLAN id instead
    # of handing them a prefix.  Same effect for the clients that were happy
    # with it, one less thing for a Linux VLAN device name to get wrong, so the
    # flake advertises it the same way as every other VLAN.  Read this line as
    # "the old box was different here", not as "this VLAN wants SLAAC".
    ra = [
      {
        from = "2001:8b0:1d14:226::1000";
        to = "2001:8b0:1d14:226::2000";
      }
    ];
    purpose = "Wireless LAN: the cameras, the 3D printers' lights, the workshop's Wi-Fi-only devices";
  }
  {
    id = 227;
    ipv4 = "10.3.14.1";
    net = "10.3.14.0/24";
    dynFrom = "10.3.14.150";
    dynTo = "10.3.14.253";
    # The old box's only IPv6 address on this VLAN was the ULA VIP
    # `fd99:dead:beef:227d::1` (its <virtualip>, read 2026-10-17).  The public
    # /64 used to be left to DHCPv6-PD as "not a fixed fact"; it is written here
    # for the same reason as VLAN 226's above — the `ra` list below advertises
    # `2001:8b0:1d14:227::/64`, and advertising a prefix the router has no
    # address in gives its clients an unreachable gateway.  Both are kept: the
    # ULA is what a client answers on when it only has the ULA prefix, and the
    # public /64 is what it reaches the internet with.
    ipv6 = [
      "2001:8b0:1d14:227::1"
      "fd99:dead:beef:227d::1"
    ];
    ra = [
      {
        from = "2001:8b0:1d14:227::1000";
        to = "2001:8b0:1d14:227::2000";
      }
      {
        from = "fd99:dead:beef:227d::1000";
        to = "fd99:dead:beef:227d::2000";
      }
    ];
    purpose = ''
      The workshop's own wired block (10.3.14.x), fed over the network's
      second run: the PI-ROOM devices, the printers, the doors.  aibox lives
      here too (common/hosts.nix), which is why the netboot `dhcp-boot` lines
      in machines/gw/dns.nix are tagged with this VLAN.
    '';
  }
  {
    id = 222;
    ipv4 = "10.3.22.1";
    net = "10.3.22.0/24";
    dynFrom = null;
    dynTo = null;
    ipv6 = [];
    ra = [];
    purpose = ''
      Phones: a VLAN so a phone's traffic can be told from the LAN's, and
      nothing more.  It has no dynamic range on the old box and still has none
      here — a device arriving on it gets no lease, which is what makes the
      VLAN exist.  Keep it: dropping it would make every phone fall into
      another VLAN's block and be routed as LAN traffic.
    '';
  }
  {
    id = 1337;
    ipv4 = "192.168.2.1";
    net = "192.168.2.0/24";
    dynFrom = "192.168.2.150";
    dynTo = "192.168.2.250";
    ipv6 = []; # the CS network's own addressing: IPv4 only, as on its router
    ra = [];
    purpose = ''
      CS_VLAN: the other network's traffic, arriving on this box's LAN with
      its own VLAN tag (the same id the other router's config calls
      `CS_VLAN`, and the block common/config.nix's LOCAL_NETWORK ACL labels
      "CS_VLAN Internal").  The router routes it but does not own any of its
      addresses.
    '';
  }
]
