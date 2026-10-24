# The interface names this box's config keys on, and the two facts about the
# AAISP uplink that more than one file needs.  A name written twice is two facts
# that can be moved apart, and the failure they move apart into is a router that
# answers nothing — or, for `UPLINK_TUNNEL`, a router that routes everything
# with its real source address and gets no replies.
#
# The two NIC names are *names* rather than MACs because a VLAN device has no MAC
# of its own to match on, so everything on the LAN side is keyed by name and the
# one place a MAC appears is the `links` rule that creates the name — those rules
# are in ./hardware-configuration.nix, pinned to the two e1000e MACs measured on
# the box.  `UPLINK_TUNNEL` is the one name here the flake cannot create: pppd
# makes it at runtime, and ./README.md says to read it back off a box that has
# dialled.
{
  LAN_NIC = "router-lan"; # the NIC the VLAN devices hang off

  # The NIC the uplink arrives on — the AAISP hub's handoff, which the old box
  # called `enc0`.  It is a *physical* NIC and it takes its address from the hub
  # (./networking.nix), which is the fact that decides whether the tunnel below
  # can dial at all.
  UPLINK_NIC = "router-uplink";

  # The interface the tunnel itself creates, which is not the NIC above: pppd
  # makes this one at runtime and the flake cannot name it.  `ppp0` is the name
  # the kernel gives the first ppp unit, so it is the honest guess, and it is the
  # name ./firewall.nix's masquerade rule matches — read it off the real box with
  # `ip link` before trusting that rule (./README.md, "read the rendered file").
  UPLINK_TUNNEL = "ppp0";

  # The hub's own address on the handoff VLAN.  The old box's `netstat -rn`
  # (2026-10-17) had `default 192.168.20.1 UG em3`, and the static endpoint route
  # below has to point at the box that can reach AAISP's L2TP LNS.  It is a
  # static route rather than the DHCP default because the tunnel is the network's
  # route out and the hub's default would otherwise keep winning.
  UPLINK_HUB_GATEWAY = "192.168.20.1";

  # AAISP's L2TP endpoint and the username the old box used (`<ppps>` in
  # /conf/config.xml, read 2026-10-17).  The password is not here: it is
  # `aaisp_ppp_password` in secrets/secrets.yaml.
  L2TP_ENDPOINT = "194.4.172.12";
  L2TP_USERNAME = "lhc1@a";

  # The peer's link-local address, which is the old box's IPv6 default gateway
  # (`netstat -rn -f inet6`: `default fe80::9e89:1eff:fe3f:0%l2tp0`).  DHCPv6
  # carries prefixes and addresses but no default route, so the route has to be
  # written down; pppd's ip-up script adds it to the main table when the tunnel
  # comes up and removes it when the tunnel goes down.
  UPLINK_TUNNEL_PEER6 = "fe80::9e89:1eff:fe3f:0";

  # The public IPv4 addresses inbound services answer on.  Read off the old box's
  # alias table (`/conf/config.xml`'s `<alias>` entries, archived at
  # ~/tmp/opnsense-archive-20261017 on services1): `DMZ_NET` = 81.187.195.17/29,
  # `DMZ_SERVICES1` = 81.187.195.17, `DMZ_YUNOHOST` = 81.187.195.18.  The /29 is a
  # static block routed to the AAISP line by AAISP; it is **not** the tunnel's own
  # address, which is dynamic (`ppp0` currently holds 81.187.223.123) and is not
  # an inbound address at all.  Only ./firewall.nix's port forwards read these.
  DMZ_NET = "81.187.195.17/29";
  PUBLIC_SERVICES1 = "81.187.195.17";
  PUBLIC_YUNOHOST = "81.187.195.18";

  # The addresses inbound public **IPv6** used to arrive on, and the ones this
  # box now blocks forwarded HTTP(S) to when they still belong to an origin.
  # They have to be written down because IPv6 has no NAT to infer them from. The
  # public front door moved to this box on 2026-10-24, so the DO AAAA for
  # `nginx.leighhack.org` is now `2001:8b0:1d14:225::1` (this box); both entries
  # below are services1's addresses and are the origin-reachability side of
  # `firewall.nix` rather than an inbound allowlist.
  #
  #   2001:8b0:1d14:225::20    the AAAA every `*.leighhack.org` vhost used to
  #                            answer with (`nginx.leighhack.org` was the CNAME
  #                            they all share) and services1's own address on
  #                            VLAN 225.
  #   2001:8b0:1d14:225:d::1020 the old box's `SERVICES1_IPV6` alias, still on
  #                            services1 (marked LEGACY TO BE DELETED there). It
  #                            used to be headscale's DERP IPv6; DERP now
  #                            publishes this box's `2001:8b0:1d14:225::1`.
  #
  # The yunohost appliance is *not* here: `yunohost.leighhack.org` has no AAAA, so
  # it has never been reachable over IPv6, and neither is anything else on the LAN
  # — the old box allowed exactly these ports on exactly this host and nothing
  # else (see ./firewall.nix's `publicV6ForwardRules`).
  PUBLIC_SERVICES1_V6 = [
    "2001:8b0:1d14:225::20"
    "2001:8b0:1d14:225:d::1020"
  ];

  # The yunohost appliance's LAN address, from the old box's `YUNOHOST` alias.
  # Not in common/hosts.nix because nothing in the flake proxies to it; it exists
  # here only so ./firewall.nix's forwards name one fact once.
  YUNOHOST = "10.3.1.15";
}
