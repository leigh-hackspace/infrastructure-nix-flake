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
# route — its own way out is ./uplink.nix's AAISP tunnel, and a networkd `Gateway=`
# on a downstream interface would be a router routing to itself), and it
# must not let networkd take an address from the RA prefix it is itself
# advertising.
{
  config,
  lib,
  pkgs,
  ...
}: let
  # The network's VLAN table, read directly: this box owns every VLAN's gateway
  # address, at the number the table gives it.  (Until 2026-10-22 this was
  # `./coexist.nix`'s rewritten copy, which held .254 on every VLAN while the
  # OPNsense appliance was still answering on .1; that box is retired and the mode
  # with it, so nothing downstream substitutes anything.)
  VLANS = import ../../common/vlans.nix;

  HOSTS = import ../../common/hosts.nix;
  NICS = import ./constants.nix;
  TABLES = import ./lib/network-tables.nix;

  # The two policy-routing marks ./firewall.nix's prerouting chain sets.  They
  # are the flake's version of the old box's `route-to` pf rules, and they are
  # *marks* rather than source-address routes because the same VLAN has a
  # different upstream depending on what it is talking to: the old box sent
  # VLAN 225 out of the AAISP tunnel but sent the rest of the network, and
  # everything that was talking to the hub's own address, out of the hub's
  # default.  A rule keyed on a packet mark can say that; a route keyed on the
  # source VLAN cannot.
  MARK_AAISP = 1;
  MARK_HUB = 2;

  # Table 100 is the AAISP tunnel's table and 200 the hub's.  The hub's default is
  # also in the main table, because the box's own traffic and any socket that does
  # not carry a mark must still find a default; the tunnel's is not, because a
  # default in the main table would be chosen by unmarked traffic and would hide a
  # dead tunnel behind a working hub.
  TABLE_AAISP = 100;
  TABLE_HUB = 200;

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
  # to *reach* it (the box's own boot wants to resolve the names it serves, and
  # services1 sshes here for `just switch-gw`'s confirm).  Same shape as
  # machines/services1/networking.nix's extraHosts, one line shorter: the router is
  # the thing the names point at, and its own address must not come from the
  # dnsmasq that is being started.
  #
  # The address is `common/hosts.nix`'s `router` — which is this box's own VLAN 225
  # address, and the assertion in ./lib/check-network-tables.nix that every
  # `common/hosts.nix` address must be a reservation or a VLAN address is what makes
  # that sentence true rather than hopeful (checked 2026-10-22 by breaking each side:
  # a `router` that is no VLAN's `ipv4` fails the evaluation).  One fact, one place:
  # this line cannot then disagree with `hosts.nix`, with `common/vlans.nix`, or with
  # `ip addr`.
  networking.extraHosts = ''
    ${HOSTS.router} gw gw.int.leighhack.org
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
          # The hub's DHCP route does not become this box's default route by
          # itself: the old box's main table had the hub's default, and its
          # `route-to` rules chose the tunnel or the hub per VLAN.  The same split
          # is written below as two tables and two marks.  `UseRoutes = false`
          # leaves only the connected route, and the endpoint /32 and defaults
          # below are written explicitly — a DHCP default that appeared and
          # disappeared with the hub's lease would race the tunnel's own default
          # and make the failure depend on lease timing.
          dhcpV4Config = {
            UseDNS = false;
            UseRoutes = false;
          };
          # The L2TP endpoint is reached through the hub, so its /32 has to be
          # in the main table or the dial has no route to its own LNS.  The next
          # two routes are the hub's default in its own table (selected by
          # MARK_HUB) and the box's own default in the main table.
          routes = [
            {
              Destination = "${NICS.L2TP_ENDPOINT}/32";
              Gateway = NICS.UPLINK_HUB_GATEWAY;
              GatewayOnLink = true;
              Metric = 10;
            }
            {
              Destination = "0.0.0.0/0";
              Gateway = NICS.UPLINK_HUB_GATEWAY;
              GatewayOnLink = true;
              Table = TABLE_HUB;
              Metric = 200;
            }
            # The box's own default: the hub, exactly as the old box's main table
            # had `default 192.168.20.1 UG em3`.  Its traffic (dnsmasq's upstreams,
            # tailscaled, the AAISP dial itself) is not the forwarded traffic the
            # marks above are about, and it must not be sent into a tunnel whose
            # endpoint it is still trying to reach.
            {
              Destination = "0.0.0.0/0";
              Gateway = NICS.UPLINK_HUB_GATEWAY;
              GatewayOnLink = true;
              Metric = 300;
            }
          ];
          # The old box's `pass out route-to (em3 192.168.20.1)` half: traffic
          # marked 2 by ./firewall.nix's prerouting chain looks in table 200, and
          # that table's default is the hub.  `Family = "ipv4"` because the hub
          # is an IPv4-only handoff and there is no IPv6 default here at all.
          routingPolicyRules = [
            {
              FirewallMark = MARK_HUB;
              Table = TABLE_HUB;
              Priority = 200;
              Family = "ipv4";
            }
          ];
          # (No `RequiredForOnline` here either — see ./networking.nix's own note
          # about the LAN NIC: the key is a .link-file setting, and the tunnel's
          # interface is not something networkd creates in any case.)
        };

        # The tunnel itself, once pppd has made it.  pppd owns the interface's
        # addresses (IPCP and IPv6CP) and ./uplink.nix's ip-up script owns the
        # IPv6 default route, but DHCPv6-PD is networkd's: AAISP delegates a
        # prefix, and networkd has to request it and hand the per-VLAN /64s out
        # (see the VLAN files below).  `DHCP = "ipv6"` asks for the delegated
        # prefix; `PrefixDelegationHint` is the old box's `/48` and is what makes
        # AAISP's server delegate the whole block rather than a single /64.
        #
        # The unit is present whether or not ppp0 exists yet: networkd writes a
        # .network file for a device that is not there and does nothing with it,
        # which is the least surprising way to have "the tunnel has not dialled"
        # be a link-state fact rather than a config that appears and disappears.
        "10-tunnel" = {
          matchConfig.Name = NICS.UPLINK_TUNNEL;
          networkConfig = {
            DHCP = "ipv6";
            IPv6AcceptRA = false;
          };
          dhcpV6Config = {
            UseDNS = false;
            PrefixDelegationHint = "::/48";
          };
          # The old box's `route-to (l2tp0 81.187.81.187)` half: VLAN 225 looks
          # in table 100, whose default ./uplink.nix's ip-up script writes.
          routingPolicyRules = [
            {
              FirewallMark = MARK_AAISP;
              Table = TABLE_AAISP;
              Priority = 100;
              Family = "ipv4";
            }
          ];
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
                Address =
                  ["${v.ipv4}/${TABLES.maskOf v.net}"]
                  # The VLAN's own IPv6 addresses (common/vlans.nix's `ipv6`), at
                  # the /64 the RA ranges are in.  They were missing from the
                  # first version of this file, which is why the router had no
                  # address in the prefix it was advertising: dnsmasq can hand
                  # out an RA for a prefix the interface does not have, and the
                  # The result is clients that configure an address and cannot reach
                  # their gateway.  The delegated VLANs' addresses *used* to be absent
                  # on the theory that networkd would assign them from the prefix it
                  # gets on ppp0; PD has never been seen to work on this box, so
                  # common/vlans.nix now carries the public /64 of every VLAN that
                  # advertises one, and the delegation below is the bonus rather than
                  # the source of truth.
                  ++ map (a: "${a}/64") v.ipv6;
                # No `DNS=` here, and no default route on the other VLANs.  This
                # box *is* the network's resolver: its own resolution comes from
                # /etc/resolv.conf, which ./dns.nix writes to point at dnsmasq on
                # loopback, and systemd-resolved — which would otherwise own
                # 127.0.0.53:53 and refuse to start dnsmasq at all — is switched off
                # there too.  A `DNS=` line in a .network file is read by resolved,
                # so with resolved gone it would be decoration, and decoration about
                # the one port this box most needs free is the worst kind.
                #
                # No VLAN accepts router advertisements, including the ones with
                # IPv6 of their own.  A router that accepts an RA on a downstream
                # interface is a router that can install a default route via
                # itself: dnsmasq advertises on these VLANs (./dns.nix), and the
                # old box's `enable-ra` plus a `UseGateway=true` was a loop
                # waiting for a second router to appear on a VLAN.  The IPv6
                # addresses below are written, not learned.
                IPv6AcceptRA = false;
                # The public /64s, which used to come only from AAISP's delegation.
                # common/vlans.nix now carries them statically on 226 and 227 as well,
                # because the `ra` lists advertise them and a prefix the router has no
                # address in is a prefix its clients cannot use.  The static value and
                # the delegated one are the same /64 by construction — the VLAN's id as
                # `SubnetId` is hexadecimal (`226` = 0x226), which is exactly the old
                # box's decimal `sla-id 550` in its archived `dhcp6c` config — so both
                # halves agree if PD ever starts answering.  VLAN 225 is static for the
                # same reason and asks for no delegation: assigning the delegated prefix
                # there too would add a second public address to the same router for no
                # reason.
                DHCPPrefixDelegation = v.id == 226 || v.id == 227;
              };
              # One static route on the wired VLAN, and no default route anywhere
              # in networkd: this box's way out is ./uplink.nix (the AAISP tunnel
              # in table 100, the hub's default in table 200 and in the main table,
              # all written by pppd's ip-up), and a `Gateway=` on a downstream VLAN
              # would be a router installing a route to itself.  A [Route] section
              # rather than [Network] Gateway=, which systemd deprecated.
              #
              # The static route is the old box's `192.168.49.0/24 via 10.3.1.20`
              # (its <staticroutes> section, read 2026-10-17): a tailnet subnet
              # route served by services1, which is on the LAN and must be reached
              # through it rather than through the tunnel — the tunnel's own
              # default would otherwise swallow it.
              routes = lib.optionals (v.id == 225) [
                {
                  Destination = "192.168.49.0/24";
                  Gateway = HOSTS.services1;
                }
              ];
              # The delegated prefix's per-VLAN half: networkd takes the /64 it
              # asked for on ppp0, appends `SubnetId` (the old box's track6 prefix
              # id), gives the VLAN an address from it, and routes the /64 here.
              # `Assign = true` is what makes the router reachable at an address in
              # the prefix it is advertising; without it the VLAN would route the
              # prefix but have no address in it.
              dhcpPrefixDelegationConfig = lib.mkIf (v.id == 226 || v.id == 227) {
                SubnetId = toString v.id;
                Assign = true;
              };
            };
          }
        )
        VLANS
      );
  };
}
