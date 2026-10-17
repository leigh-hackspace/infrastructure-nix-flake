# The router's interfaces: the LAN NIC, one VLAN device per VLAN in
# common/vlans.nix, and the NIC the uplink arrives on.  This is the flake's
# version of what OPNsense generated from its `<Interfaces>` XML — same device
# names, same addresses, generated from the tables instead of typed into a UI.
#
# The addresses come from common/vlans.nix and nothing else, which is the point:
# a VLAN whose dnsmasq range, nftables rule and interface address were written
# separately is the bug this file exists to prevent (./lib/check-network-tables.nix
# is the other half).
#
# The shape is machines/services1/networking.nix's, because that is the box that
# already tags its own traffic on this network: `netdevs` for the VLAN devices,
# `vlan = [...]` on the parent's network file to hang them off it, and the VLAN's
# own network file carrying its address.  Two things services1 does that this box
# must not: it must not ask for a default route on the VLANs (it *is* the default
# route — its own gateway comes from the uplink, ./uplink.nix, except while
# ./coexist.nix is on, when it comes from the router still in that job), and it
# must not let networkd take an address from the RA prefix it is itself
# advertising.
{
  config,
  lib,
  pkgs,
  ...
}: let
  COEXIST = import ./coexist.nix;

  # The VLANs as this box must read them — ./coexist.nix, which is common/vlans.nix
  # with this box's own host address substituted while the old router is still
  # live.  Nothing in this file reads common/vlans.nix directly: a file that read
  # the raw table would take .1 on one VLAN and .254 on the next.
  VLANS = COEXIST.vlans;

  NICS = import ./constants.nix;
  TABLES = import ./lib/network-tables.nix;

  # Does this VLAN carry the box's way out?  Only one does, and it is worked out
  # from the table by ./coexist.nix rather than assumed to be 225.
  isTransit = v: COEXIST.enable && v.id == COEXIST.transitVlan.id;

  # The NIC names machines/constants.nix owns (see that file's header: they are
  # hardware facts, keyed by name because a VLAN device has no MAC of its own,
  # and the `links` rules that pin each name to a MAC are in
  # hardware-configuration.nix, so these two names are what the box answers on.
  LAN_NIC = NICS.LAN_NIC;
  UPLINK_NIC = NICS.UPLINK_NIC;
in {
  networking = {
    # No DHCP client on the VLANs or the LAN: the router's addresses are in
    # common/vlans.nix and its tunnel is dialled by ./uplink.nix.  The uplink NIC's
    # hub-assigned address comes from systemd-networkd's own client
    # (`DHCP = "ipv4"` above), not from dhcpcd, and this is the switch that keeps
    # dhcpcd out of the way so the two never manage one interface.
    #
    # Spelled as the deprecated `useDHCP` rather than `dhcpcd.enable` because that
    # is what machines/services1/networking.nix does and what the networkd module
    # checks: leaving it at its default is a *warning* on every evaluation of this
    # machine, and `nix flake check` is expected to stay warning-free (AGENTS.md).
    useDHCP = false;

    # The firewall is nftables (./firewall.nix), which is the old box's own choice
    # and the reason `networking.firewall.enable` stays off: systemd-networkd's own
    # rules and nixpkgs' firewall would both be editing the same table for different
    # reasons.  services1 spells the same thing the same way.
    firewall.enable = false;
  };

  # The router's own name resolution: it answers for the network, and it also has
  # to *reach* it (dns-sync sshes here, and the box's own boot wants to resolve
  # the names it serves).  Same shape as machines/services1/networking.nix's
  # extraHosts, one line shorter: the router is the thing the names point at, and
  # its own address must not come from the dnsmasq that is being started.
  #
  # The address is this box's own, on the VLAN it reaches the network through:
  # `gw` is this box's name (./default.nix sets the hostname), and while
  # ./coexist.nix is on, this box is not at the address common/hosts.nix gives the
  # name it shares with the live router.  Pointing it at 10.3.1.1 would make
  # `getent hosts gw` answer with a different machine than `hostname` — which is
  # exactly the kind of two-facts-about-one-name confusion the rest of this
  # directory refuses.
  networking.extraHosts = ''
    ${COEXIST.transitVlan.ipv4} gw gw.int.leighhack.org
  '';

  systemd.network = {
    enable = true;

    # The network's DNS is the router's own dnsmasq; a wait-online hook here
    # would be waiting for the thing that answers for everything else.  Same
    # reason as on services1 (common/systemd.nix's comment).
    wait-online.enable = false;

    # VLAN devices, one per VLAN in common/vlans.nix.  `vlanConfig.Id` is the id
    # on the wire — the number a client's traffic arrives tagged with — and the
    # device name is what dnsmasq turns into a DHCP tag (common/vlans.nix's
    # header: FreeBSD spelled it `vlan0.<id>`, Linux spells it `vlan<id>`), so
    # both halves of that name are generated here rather than written twice.
    #
    # No `matchConfig` here: a .netdev file has no [Match] section — the device
    # is created by whatever .network file names it in its `VLAN=` key, which is
    # the LAN NIC's file below.  Putting a Match in a netdev is a warning from
    # networkd at best.
    netdevs = builtins.listToAttrs (
      map
      (
        v: {
          name = "50-vlan-${toString v.id}";
          value = {
            netdevConfig = {
              Kind = "vlan";
              Name = TABLES.vlanDevice v;
            };
            vlanConfig.Id = v.id;
          };
        }
      )
      VLANS
    );

    networks =
      {
        # The LAN NIC itself: it carries the VLANs and owns no address of its own.
        # `vlan` is the list the netdevs above create; services1 spells it the same
        # way, and the two spellings of the same list is what ./lib/network-tables.nix
        # exists to keep together.
        "10-lan" = {
          matchConfig.Name = LAN_NIC;
          networkConfig.DHCP = false;
          vlan = map TABLES.vlanDevice VLANS;
          # No `linkConfig.RequiredForOnline` here even though it is the sentence
          # this box wants to say ("the LAN cable being unplugged is a fault, but a
          # *routed* LAN NIC is not what the box being up means"): that key belongs
          # to systemd.link(5), not to a .network file's [Link] section, and nixpkgs'
          # networkd module refuses it as a type error rather than dropping it.  The
          # fleet's own answer to "when is the network up" is already here: the
          # wait-online unit is off, so nothing on this box waits for anybody.
        };

        # The uplink NIC: the AAISP hub's handoff, and the interface the tunnel is
        # dialled through (the old box called it `enc0`).  It takes its address from
        # the hub by DHCP — that is what makes it different from every other
        # interface on this box, and it is the fact the tunnel depends on: pppd has
        # to have a route to the L2TP endpoint before it can dial it, and that route
        # is the hub's.  If the uplink turns out to be a direct handoff instead
        # (PPPoE or a routed public block straight into this NIC), this is the line
        # that changes and ./README.md says how to tell which it is.
        "10-uplink" = {
          matchConfig.Name = UPLINK_NIC;
          networkConfig = {
            DHCP = "ipv4";
            IPv6AcceptRA = false;
          };
          # The hub is not the network's DNS and must not become it: dnsmasq on this
          # box answers for the network, and a `/etc/resolv.conf` pointed at the hub
          # would make the router's own name resolution depend on the thing it is
          # supposed to be shielding.  A separate section, not a key of [Network]:
          # networkd's own schema, and the NixOS module's.
          dhcpV4Config.UseDNS = false;
          # (No `RequiredForOnline` here either — see ./networking.nix's own note
          # about the LAN NIC: the key is a .link-file setting, and the tunnel's
          # interface is not something networkd creates in any case.)
        };
      }
      // builtins.listToAttrs (
        map
        (
          v: {
            name = "50-${TABLES.vlanDevice v}";
            value = {
              matchConfig.Name = TABLES.vlanDevice v;
              networkConfig = {
                DHCP = false;
                # The address this VLAN answers with (common/vlans.nix's `ipv4`),
                # written with the block it belongs to — both halves come from the
                # same record, so the address cannot be in a block the VLAN does
                # not have (./lib/check-network-tables.nix refuses that too).  The
                # mask, not the block address: `10.3.1.1/10.3.1.0` is what writing
                # the wrong half of `net` produces, and networkd calls it an
                # invalid prefix at boot rather than at eval.
                Address = ["${v.ipv4}/${TABLES.maskOf v.net}"];
                # No `DNS=` here, and no default route on the other VLANs.  This
                # box *is* the network's resolver: its own resolution comes from
                # /etc/resolv.conf, which ./dns.nix writes to point at dnsmasq on
                # loopback, and systemd-resolved — which would otherwise own
                # 127.0.0.53:53 and refuse to start dnsmasq at all — is switched off
                # there too.  A `DNS=` line in a .network file is read by resolved,
                # so with resolved gone it would be decoration, and decoration about
                # the one port this box most needs free is the worst kind.
                #
                # Only the VLANs that have IPv6 of their own accept router
                # advertisement; the old box answered IPv4-only on the others
                # (common/vlans.nix says which, per VLAN).
                IPv6AcceptRA = v.ipv6 != [];
              };
              # This box's own default route, while it has no uplink: the live
              # router, on the one VLAN that can reach it.  A [Route] section rather
              # than [Network] Gateway=, which systemd deprecated in favour of it.
              #
              # Not on every VLAN, and not when co-existence is off: the router's
              # own route out is the AAISP tunnel ./uplink.nix dials, and a default
              # route via the old router would survive into the cutover and point the
              # new router's traffic at the old one — a loop with two DNS servers in
              # it.  `ip route` on the box is the check that this is the only one.
              routes = lib.optionals (isTransit v) [{Gateway = COEXIST.transitGateway;}];
              # The RA prefix is for routing only: the addresses on a VLAN are the
              # fixed reservations in common/dns-records.nix and the leases dnsmasq
              # hands out, and letting networkd invent a third kind of address from
              # the prefix this same box advertises is what would move a device off
              # the address the rest of the flake promises.  Same setting as
              # machines/services1/networking.nix, for the same reason.
              ipv6AcceptRAConfig = {
                UseAutonomousPrefix = false;
                UseOnLinkPrefix = true;
                UseGateway = true;
                UseDNS = false;
              };
            };
          }
        )
        VLANS
      );
  };
}
