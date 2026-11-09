# MODULE — the router's *declared* WAN path to a box that holds the uplink.
#
# **What this is.** A point-to-point IP-in-IP tunnel to `aibox`, declared to
# `wan-manager` as `--relay-candidate wan-aibox`, so the daemon probes, ranks and
# selects it exactly as it does a cable, a radio or the AAISP tunnel. It is not a
# second router and it carries nothing of its own: `machines/aibox/wan-relay.nix`
# is the other half, and the far end is whatever uplink *that* box has plugged in.
#
# **Why it exists at all** (measured 2026-11-09, `docs/wan-manager-2026-11-09.md`
# has the receipts). This box has no xHCI at all — Intel 6 Series EHCI only — and
# an Android phone plugged into it never gets past `device descriptor read/64,
# error -71`: it chirps, associates at high speed and is then never enabled, on
# both of this box's sockets, through every VBUS cycle and both documented
# `usbcore.*` switches. The same phone, same cable, enumerates as a SuperSpeed
# `cdc_ncm` device on `aibox` and hands out a lease. USB/IP was tried and is out:
# binding the device to `usbip-host` unbinds Android's NCM function and the phone
# re-enumerates with the tether gone (twice), so a *network* path is the only one.
#
# **What it is not.** Not a default route. The tunnel sits there at all times and
# answers nothing when `aibox` has no tether plugged in; the daemon measures that
# and refuses to select it (`eligible()` requires internet through *this* device).
# Nothing here decides the egress — `crates/wan-manager` does, and this file only
# makes the interface exist.
#
# **The address pair is deliberate and meaningless.** `10.255.255.0/30` is the
# inner link, chosen from 240/4 so it can never collide with anything the LAN, a
# carrier or a hotspot hands out; the two addresses exist to give each end
# something to source probes from, and neither is routed anywhere else.
{lib, ...}: let
  HOSTS = import ../../common/hosts.nix;

  # Read by `machines/gw/wan-backup.nix`'s `--relay-candidate` and by
  # `machines/aibox/wan-relay.nix`'s netdev; one spelling, two boxes.
  NAME = "wan-aibox";
in {
  systemd.network.netdevs."20-wan-relay" = {
    netdevConfig = {
      Name = NAME;
      Kind = "ipip";
      # 1500 on the LAN minus the 20-byte outer header, with headroom: the phone
      # tethers at 1500 and an inner packet that has to fragment across a link
      # nobody is watching is a stall, not a slow page.
      MTUBytes = "1400";
    };
    # No `Local=`: the outer source is whatever the route to `aibox` uses, which
    # is this box's vlan225 address. Pinning it here would be a second copy of a
    # fact `common/vlans.nix` already owns.
    tunnelConfig.Remote = HOSTS.aibox;
  };

  systemd.network.networks."20-wan-relay" = {
    matchConfig.Name = NAME;
    address = ["10.255.255.1/30"];
    networkConfig = {
      # A relay is not a DHCP client and has no link-local anything. Everything
      # else — the default route in both tables, the NAT set membership — is the
      # daemon's to install on the tick it selects this path.
      DHCP = "no";
      IPv6AcceptRA = false;
      LinkLocalAddressing = "no";
    };
    # The *probe* route, and the reason it is not the main table: a path that is
    # not selected must still be measurable, and binding a probe to the device is
    # not enough when the device has no default of its own. So a probe is bound to
    # the tunnel's own address and the rule below sends exactly that source into
    # this table — measured working on 2026-11-09. When the daemon does select the
    # relay it writes `default dev wan-aibox` into the main and hub tables itself;
    # this route is never removed and never selected by anything else.
    routes = [
      {
        Destination = "0.0.0.0/0";
        Table = 950;
        Scope = "link";
      }
    ];
    # The rule that makes the route above reachable and nothing else: only the
    # tunnel's own source address is sent into table 950. LAN traffic keeps the
    # main and hub tables, so a relay that is being *measured* is never a relay
    # that is being *used*.
    routingPolicyRules = [
      {
        From = "10.255.255.1/32";
        Table = 950;
        Priority = 950;
        Family = "ipv4";
      }
    ];
  };

  # Nothing here asserts the daemon was told. The wiring lives where the unit is
  # defined — `machines/gw/wan-backup.nix` passes `--relay-candidate` — because a
  # name is checked by *using* it: a candidate that the daemon was not told about
  # is indistinguishable from one whose far end has no uplink, and the Uplink tab
  # shows both as `relay` with "no internet through this path".
}
