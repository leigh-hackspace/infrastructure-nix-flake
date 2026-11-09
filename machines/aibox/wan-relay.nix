# MODULE — aibox as the far end of the router's declared relay path.
#
# **What this is.** `machines/gw/wan-relay.nix` is one end of an IP-in-IP tunnel;
# this is the other. When the phone is plugged into *this* box (its xHCI
# enumerates it, gw's EHCI never can — `docs/wan-manager-2026-11-09.md`), the
# phone's USB tether becomes a WAN path for the whole network: the router NATs the
# LAN onto the tunnel, this box forwards those packets out the tether and
# masquerades them, and the phone NATs them again onto whatever it has.
#
# **Three facts, and each is one line of config:**
#
#   1. the tether gets a DHCP lease whose *default route lands in table 900*, not
#      in the main table — this box's own egress must never follow a phone that
#      was plugged in for the router's benefit;
#   2. a packet arriving on the tunnel looks up table 900 (`iif wan-gw`), so the
#      only traffic that ever leaves through the tether is traffic the router
#      handed over on purpose;
#   3. that traffic is masqueraded to the tether's address. The phone rejects a
#      source outside its own /24 — measured: `From 10.111.16.77 icmp_seq=1
#      Destination Net Unreachable` for an un-NATed inner source, 0 % loss and
#      2.6 ms once this rule existed.
#
# **No forwarding change is required and none is made here.** `./networking.nix`
# already turns IPv4 forwarding on (for QEMU guests) and this box's firewall is
# off, so the relay adds *reachability* where it already had it, not a new
# exposure: nothing new is accepted, only forwarded traffic that arrives on
# `wan-gw` is NATed, and the only packets that arrive there are the router's.
#
# **What must not move.** Do not give `wan-gw` a default in the main table and do
# not let the tether's lease land there: this box is the network's build machine
# and its own egress is `vlan225`. `ip route show default` on this box must keep
# saying `via 10.3.1.1 dev vlan225` when a phone is plugged in.
{lib, ...}: let
  HOSTS = import ../../common/hosts.nix;

  # The tunnel's name, shared with the router's half; see `machines/gw/wan-relay.nix`.
  RELAY = "wan-gw";
in {
  systemd.network.netdevs."20-wan-relay" = {
    netdevConfig = {
      Name = RELAY;
      Kind = "ipip";
      MTUBytes = "1400";
    };
    tunnelConfig.Remote = HOSTS.router;
  };

  systemd.network.networks."20-wan-relay" = {
    matchConfig.Name = RELAY;
    address = ["10.255.255.2/30"];
    networkConfig = {
      DHCP = "no";
      IPv6AcceptRA = false;
      LinkLocalAddressing = "no";
    };
    routingPolicyRules = [
      {
        IncomingInterface = RELAY;
        Table = 900;
        Priority = 900;
        Family = "ipv4";
      }
    ];
  };

  # The phone, matched by the driver that claims it rather than by a name: the
  # netdev name depends on which socket the cable is in (`enp118s0f3u1` today),
  # and the driver does not. `cdc_ncm` is what this phone presents — measured on
  # this box, `22d9:908c` at 5000M with three interfaces, `enp118s0f3u1` renamed
  # from `usb0`.
  systemd.network.networks."30-wan-tether" = {
    matchConfig.Driver = "cdc_ncm";
    networkConfig = {
      DHCP = "ipv4";
      # The tether's own DNS is the phone's; letting it into resolved would let a
      # phone decide how this box resolves names.
      IPv6AcceptRA = false;
      LinkLocalAddressing = "no";
    };
    dhcpV4Config = {
      UseDNS = false;
      # Everything the lease installs — address, gateway, route — is for the
      # router's traffic, and the address is the one this interface holds.
      RouteTable = 900;
      RouteMetric = 100;
    };
  };

  # The one rule that makes table 900 reachable, and only for packets that came
  # over the tunnel: everything the router NATs onto this path carries the
  # tunnel's own address as its source, so the rule is keyed on the incoming
  # interface and this box's own traffic never looks in table 900. It is declared
  # on the tunnel's own `.network` file, below, so networkd installs it with the
  # interface and cannot leave a rule pointing at a device that is not there.

  networking.nftables.tables."wan-relay" = {
    family = "ip";
    content = ''
      chain postrouting {
        type nat hook postrouting priority srcnat; policy accept;
        # Keyed on the *input* interface, so no tether name has to be written
        # here and no other traffic on this box can be masqueraded by accident.
        iifname "${RELAY}" masquerade
      }
    '';
  };
}
