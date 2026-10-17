# CO-EXISTENCE MODE — a temporary fact about this network, not a feature.
#
# This box (./README.md: the Supermicro X9SCL/X9SCM at 10.3.1.254) is plugged
# into the same VLAN-225 trunk as the router it is meant to become, while that
# router is still live at 10.3.1.1.  So this is a router that must not behave
# like the network's router, and there are four ways for it to be one by
# accident.  Each is switched off here, in one place, and each is verifiable on
# the box rather than assumed:
#
#   * it must not take .1 on any VLAN       -> `ip -4 addr` shows .254 on every
#                                             VLAN, and `getent hosts gw` answers
#                                             this box rather than the live router
#   * it must not hand out leases           -> `no-dhcp-interface=` for every VLAN
#                                             device, and the dhcp-range blocks in
#                                             /etc/dnsmasq.d/network.conf commented;
#                                             dnsmasq.leases stays empty
#   * it must not announce router adverts   -> `enable-ra = false` in ./dns.nix, and
#                                             the ra-only/ra-param lines commented
#                                             in the same file
#   * it must not advertise tailnet routes  -> no `--advertise-routes`/exit-node
#                                             flag at all, and `tailscale status
#                                             --json`'s .Self.AllowedIPs holds only
#                                             this node's own two addresses
#
# `enable = false` is the cutover, and it must not happen until ./README.md's
# cutover steps have been run against the old box: the two of them on one
# network, both answering DNS and both handing out leases, is the failure this
# file exists to prevent, and it is a failure of *clients* (two leases for one
# address, two DNS answers, an RA choosing a gateway at random) rather than of
# either router.
#
# Why a plain file rather than a module option: ./lib/check-network-tables.nix
# is a plain file too, and the assertions that keep this mode honest (the host
# address must not be inside a dynamic range, must not be a reserved address,
# the transit gateway must be inside exactly one VLAN block) have to be able to
# read it.  Everything downstream reads *this file's* `vlans` rather than
# common/vlans.nix, so the mode cannot be half-applied.
let
  VLANS = import ../../common/vlans.nix;
  TABLES = import ./lib/network-tables.nix;

  # On while the old router is.  Flip to false to cut over.
  enable = false;

  # The last octet this box takes on every VLAN, replacing the .1 that
  # common/vlans.nix says each VLAN's router owns.  254 because it is the
  # highest address of every /24 in the table and so outside every dynamic
  # range (the highest of which ends at .253) — and ./check-network-tables.nix
  # is what keeps that true when a range moves.
  hostAddr = "254";

  # The live router: this box's route out and its upstream resolver while it has
  # no uplink of its own (./uplink.nix's AAISP tunnel is not enabled here, so
  # this is the only way this box reaches anything outside its own subnets).  It
  # is `common/hosts.nix`'s router rather than a copy of it, and the VLAN it
  # falls inside is worked out from the table below rather than assumed to be
  # 225.
  transitGateway = (import ../../common/hosts.nix).router;

  # The VLANs as this box must read them.  This is the whole of the mode: the
  # ids, blocks, dynamic ranges, RA ranges and purposes are facts about the
  # network and are untouched, `ipv4` is a fact about this box and moves.
  vlans =
    if enable
    then TABLES.hostAddrs hostAddr VLANS
    else VLANS;

  # The VLAN this box reaches the live router through, i.e. the one whose block the
  # live router's own address falls in — which is the only one that gets a default
  # route.  Worked out rather than written: a transit gateway on a VLAN this box has
  # no interface on is a box with no route at all, and the failure would show up as
  # "the router cannot resolve anything" rather than as a misconfiguration.
  transitVlans = builtins.filter (v: TABLES.inBlock transitGateway v.net) vlans;
  transitVlan =
    if builtins.length transitVlans == 1
    then builtins.head transitVlans
    else
      builtins.throw ''
        machines/gw/coexist.nix: transitGateway = "${transitGateway}" is inside ${
          toString (builtins.length transitVlans)
        } of the VLAN blocks in common/vlans.nix; it has to be inside exactly one, because that is
        the interface this box sends its own traffic out of.
      '';
in {
  inherit enable hostAddr transitGateway vlans transitVlan;
}
