# The router's packet decisions: what may be forwarded, and what may reach the
# router itself.  This is the flake's version of the rules the old box ran —
# OPNsense generated nftables from its `<Interfaces>` XML and ran
# `/usr/local/sbin/nft -f /var/etc/nftables.conf` (its own `ps` said so), so the
# tool is inherited and only the source of truth changes: common/vlans.nix and
# common/config.nix here instead of the XML there.
#
# WHAT THE OLD BOX'S FIREWALL WAS FOR, in one paragraph, because it is not
# obvious and it is the reason this file exists at all: the network is one uplink
# (AAISP's tunnel) shared by every device on it, and the uplink's address space is
# not the network's.  So the router has to decide, per packet, whether the traffic
# is allowed to leave the building at all, and the answer it uses is the VLAN the
# packet arrived on plus the network's own allow-list (common/config.nix's
# LOCAL_NETWORK — the same table the nginx servers read, which is the point: one
# list of who is allowed in, two renderers).  The per-VLAN part is routing, not a
# security boundary; the ACL is the boundary.
#
# THREE THINGS THIS FILE CANNOT PROVE WITHOUT THE HARDWARE, and they are the
# parts to read before believing the rest:
#
#   1. **The VLAN match form.**  nftables matches a tagged packet with `vlan id`,
#      which applies to the packet as it arrives on the parent NIC with its tag
#      still on it.  Whether the flake's VLAN devices (./networking.nix) see the
#      packet on the VLAN device or on the parent is a question about Linux's
#      in-kernel VLAN path: with a `vlan<id>` *device*, the tag has usually
#      already been stripped by the time the packet reaches the VLAN device, so
#      `vlan id` matches on the parent NIC's hooks and `iifname "vlan225"` matches
#      on the VLAN's.  Both forms are written below (the anti-spoof rule carries
#      both), because the honest answer is "one of these is redundant and the real
#      box says which".  Delete the redundant one once it is known — and read the
#      rendered ruleset, not this comment, to decide.
#   2. **The old box's exact rules.**  They have never been read: the box was
#      found to *run* nft from `/var/etc/nftables.conf`, which is not the same as
#      knowing what that file said.  ./README.md gives the read-only commands
#      (`cat /var/etc/nftables.conf`, `nft list ruleset`); anything below that
#      contradicts them is an accident, not a decision, and "the rules parse" is
#      not "the rules are the network's rules".
#   3. **The uplink's interface name.**  NAT below matches the tunnel's own
#      interface (./constants.nix's `UPLINK_TUNNEL`), which is a name pppd gives
#      the device at runtime and nothing in the flake can check.
#
# What nixpkgs *does* check: `networking.nftables.checkRuleset` defaults to true,
# so a ruleset that does not parse is a build failure rather than a router that
# stops routing.  That is a syntax guarantee, not a semantics one — every rule
# below can parse and still be the wrong rule.
{
  config,
  lib,
  pkgs,
  ...
}: let
  COEXIST = import ./coexist.nix;
  # The VLANs as this box reads them (./coexist.nix).  Only the addresses move in
  # co-existence mode and this file rules on blocks, not addresses — but it reads
  # the same list dnsmasq and the VLAN devices are generated from, because two
  # lists that happen to agree today are the drift this directory keeps removing.
  VLANS = COEXIST.vlans;
  CONFIG = import ../../common/config.nix;
  NICS = import ./constants.nix;
  TABLES = import ./lib/network-tables.nix;

  # The network's allow-list, as the two nftables set literals the two address
  # spaces need.  Generated from common/config.nix, which writes it as nginx
  # `allow` lines because the network's nginx servers are its other reader — the
  # split is in ./lib/network-tables.nix and ./lib/check-network-tables.nix proves
  # the split is exhaustive, so nothing here has to know which entries are which.
  aclV4 = TABLES.setOf (TABLES.ipv4AclBlocks CONFIG.LOCAL_NETWORK);
  aclV6 = TABLES.setOf (TABLES.ipv6AclBlocks CONFIG.LOCAL_NETWORK);

  # One VLAN's anti-spoof rule: a packet that arrived on this VLAN must come from
  # the block this VLAN's leases are handed out of.  That is the rule the old box's
  # per-interface "block private" behaviour actually needed — a client cannot claim
  # to be someone else's address by lying in its own packet — and it is per VLAN
  # because the VLAN is what says which block is legitimate.
  #
  # VLAN 222 (phones) has no range and no reservations (common/vlans.nix says so),
  # so its rule drops everything tagged 222 that is not in 10.3.22.0/24, i.e.
  # everything: that is the VLAN's documented purpose ("a device arriving on it
  # gets no lease"), not an oversight, and it is why the rule is generated rather
  # than argued about per VLAN.
  #
  # The rule is `drop`, not `reject`: a spoofed packet gets nothing back, and a
  # client whose *own* address is wrong finds out from its own timeouts.
  vlanAntiSpoof =
    builtins.concatStringsSep "\n"
    (
      map
      (
        v: ''
          # VLAN ${toString v.id} (${v.ipv4})
          vlan id ${toString v.id} ip saddr != ${TABLES.setOf [v.net]} drop
          iifname "${TABLES.vlanDevice v}" ip saddr != ${TABLES.setOf [v.net]} drop
        ''
      )
      VLANS
    );

  # The services this box exists to answer, from the LAN side: DHCP (a client with
  # no address yet, so matched by interface and port rather than by address) and
  # DNS.  The old box answered both on every LAN interface and nothing else, and
  # its admin UI (443) is deliberately not here — ./README.md says why.
  lanServiceRules = ''
    # DHCP: the client's own source address is 0.0.0.0 until it has a lease, so
    # this is the one rule in the file that cannot be keyed on the ACL.
    iifname "vlan*" udp dport { 67, 68 } accept
    # DNS, both transports: an EDNS answer bigger than 512 bytes arrives over TCP,
    # and the network's own resolvers do use EDNS.
    iifname "vlan*" udp dport 53 accept
    iifname "vlan*" tcp dport 53 accept
    # The tailnet reaches the same names, on the interface the ACL already allows
    # (common/config.nix names 100.64.0.0/16).  DNS only: a DHCP request arriving
    # on a VPN tunnel is not a lease this box should be honouring, and answering
    # one would be handing out LAN addresses to a client that is not on the LAN.
    iifname "tailscale0" udp dport 53 accept
    iifname "tailscale0" tcp dport 53 accept
  '';

  # The uplink's own input rules: the box has to be able to *take an address* from
  # the AAISP hub before anything else on this box works, and a DHCP reply is not
  # `established` from the point of view of a client that sent its request to the
  # broadcast address.  Without this rule the router has no uplink address, no
  # tunnel, no route out, and no way in to find out why — which is the definition
  # of a bad 04:00.
  uplinkServiceRules = ''
    iifname "${NICS.UPLINK_NIC}" udp sport 67 udp dport 68 accept
  '';
in {
  # The router forwards: that is the whole point of it.  Written as sysctls rather
  # than as networkd's `IPForward=` per interface, because the answer is the same
  # for every interface on this box and a router whose forwarding depends on a
  # per-interface flag someone forgot to set is a router that works until the VLAN
  # table grows.
  boot.kernel.sysctl = {
    "net.ipv4.ip_forward" = 1;
    "net.ipv6.conf.all.forwarding" = 1;
  };

  networking.nftables = {
    enable = true;

    tables = {
      # --- IPv4: forwarding --------------------------------------------------
      routerForward = {
        family = "ip";
        name = "router";
        content = ''
          chain forward {
            type filter hook forward priority 0; policy drop;

            # A packet that belongs to a connection the router is already
            # carrying needs no rule of its own, and a packet in no connection at
            # all is not the start of one the network asked for.
            ct state established,related accept
            ct state invalid drop

            # Path MTU discovery has to be allowed to fail loudly: a packet that is
            # too big for the tunnel has to come back as "unreachable, needs
            # fragmentation" or the connection stalls until it times out, which looks
            # like a network that works for DNS and not for TLS.  This is the rule
            # that makes an MTU that is wrong (see ./uplink.nix's TODO) a slow network
            # rather than a broken one.
            #
            # The whole `destination-unreachable` family, not just the need-frag code:
            # filtering ICMP by code is how a firewall ends up eating the one message
            # it needed.  And note the v4 names are not the v6 ones — `packet-too-big`
            # is ICMPv6's name for the same idea and nft refuses to parse it here
            # (which the build check caught, the first time this line was written).
            ip protocol icmp icmp type { destination-unreachable, time-exceeded, parameter-problem } accept

            # Anti-spoof first: the ACL below would happily accept a packet from
            # 10.3.1.20 arriving tagged with someone else's VLAN, and "the ACL
            # allows that address" is exactly the lie this rule exists to catch.
            ${vlanAntiSpoof}

            # The network's own allow-list, and the only thing that may leave.
            # This is the boundary; everything above it is routing inside it.
            ip saddr ${aclV4} accept

            # Everything else is traffic from an address the network does not
            # allow, and the policy says drop.  No rule, on purpose: a rule here
            # would be a second opinion about the policy, and the two would drift.
          }
        '';
      };

      # --- IPv4: the router's own traffic -----------------------------------
      routerInput = {
        family = "ip";
        name = "routerIn";
        content = ''
          chain input {
            type filter hook input priority 0; policy drop;

            # The box talking to itself, and the replies to connections it
            # started (its own DNS upstream, ntp, the ssh dns-sync runs in from
            # services1).
            iifname "lo" accept
            ct state established,related accept
            ct state invalid drop

            ${lanServiceRules}
            ${uplinkServiceRules}

            # The tailnet's data plane: tailscale's own UDP port, open to the
            # internet because the tailnet *is* the internet as far as this box is
            # concerned.  The tunnel's control plane needs no rule — pppd dials out,
            # and the replies arrive as `established`; the first version of this file
            # opened IKE/IPsec here on the assumption that the tunnel was dialed in,
            # which was a listener on the network's public address for a protocol
            # nothing here speaks.  OpenVPN's port lives with OpenVPN
            # (./openvpn.nix), so that no rule is open for a service that is off.
            udp dport 41641 accept

            # The network's own diagnostics: pinging the router is how a power cut
            # is diagnosed from a laptop with no DNS, and the old box answered it.
            icmp type echo-request accept
            # The router's own name resolution and its upstream queries are
            # outgoing; the answers arrive as `established` above.

            # ssh, from the allow-list only.  The old box's admin UI (443) is
            # *not* opened: it was OPNsense's, and there is nothing here for a
            # browser to speak to (./README.md).
            ip saddr ${aclV4} tcp dport 22 accept
          }
        '';
      };

      # --- IPv4: the one uplink, shared -------------------------------------
      # Every device's traffic leaves with the tunnel's address, because the
      # uplink's address space is not the network's and the return path would
      # otherwise not exist.  This is the rule that makes a shared uplink work at
      # all, and it is the one rule here that the old box's XML expressed as a
      # checkbox ("Outbound NAT: automatic").
      #
      # Matched on the tunnel's interface rather than "everything not going to the
      # LAN", so that LAN-to-LAN traffic (a box on VLAN 225 reaching a box on
      # VLAN 226) keeps its real source address — the switches' and the NAS's logs
      # name the client, and after a masquerade they would name the router.
      routerNat = {
        family = "ip";
        name = "routerNat";
        content = ''
          chain srcnat {
            type nat hook postrouting priority srcnat; policy accept;
            oifname "${NICS.UPLINK_TUNNEL}" ip saddr ${TABLES.blockSet VLANS} masquerade
          }
        '';
      };

      # --- IPv6 --------------------------------------------------------------
      # The network's IPv6 is public and routed, so there is no NAT66 here and
      # there must not be one.  What is here is the same shape as the IPv4 pair:
      # forward only what the ACL allows, and let the router answer DNS/DHCP and
      # keep its own tunnels.
      #
      # ICMPv6 is not optional: neighbour discovery and router advertisement *are*
      # the data plane's plumbing, and a ruleset that drops ICMPv6 breaks IPv6 in a
      # way that looks like a dead prefix rather than a firewall.  dnsmasq's RAs
      # leave this box (output, not filtered by these chains); the inbound half is
      # what these types allow.
      routerForward6 = {
        family = "ip6";
        name = "router6";
        content = ''
          chain forward {
            type filter hook forward priority 0; policy drop;
            ct state established,related accept
            ct state invalid drop
            icmpv6 type { destination-unreachable, packet-too-big, time-exceeded, parameter-problem } accept
            ip6 saddr ${aclV6} accept
          }

          chain input {
            type filter hook input priority 0; policy drop;
            iifname "lo" accept
            ct state established,related accept
            # One spelling per type, and nft's own: the ND types are `nd-*` and
            # spelled with an American "neighbor" — `neighbour-solicitation` is not
            # a name nft knows, which is exactly the kind of thing this file's build
            # check catches (it did: the first version of this line named the ND
            # types the way systemd's man page spells them, and the ruleset
            # derivation failed to build with "Could not parse ICMPv6 type").
            icmpv6 type { destination-unreachable, packet-too-big, time-exceeded, parameter-problem, echo-request, nd-router-advert, nd-router-solicit, nd-neighbor-advert, nd-neighbor-solicit, redirect, mld-listener-query, mld-listener-report, mld-listener-done } accept
            iifname "vlan*" udp dport 53 accept
            iifname "vlan*" tcp dport 53 accept
            iifname "tailscale0" udp dport 53 accept
            iifname "tailscale0" tcp dport 53 accept
            ${uplinkServiceRules}
            # Same reasoning as the IPv4 side: tailscale's data plane, and nothing
            # that is not needed.  (No ICMPv6 *outbound* rule is needed — RAs leave
            # via the output path, which these chains do not filter.)
            udp dport 41641 accept
            icmpv6 type echo-request accept
            ip6 saddr ${aclV6} tcp dport 22 accept
          }
        '';
      };
    };
  };

  # The unit, and why it gets *no* treatment here — the one place in this machine
  # where the network's "never give up" policy is deliberately not applied, and the
  # reason is worth the paragraph.
  #
  # nixpkgs' nftables unit is `Type=oneshot, RemainAfterExit=true`: it loads the
  # ruleset and exits.  systemd does not restart a oneshot unit however `Restart=`
  # is set, so INFRA.mkNeverGiveUpOverride would be two lines of dead config and a
  # misleading comment — the failure it protects against cannot happen this way.
  # What protects against the real failures instead:
  #
  #   * a ruleset that does not parse is a *build* failure, and a much stronger one
  #     than `nft -c`: the module's `checkRuleset` default validates the ruleset
  #     against a kernel in userspace (lkl) at build time;
  #   * a ruleset that changes is reloaded, not restarted — the module sets
  #     `reloadIfChanged`, and `ExecReload` re-applies the same generated file;
  #   * a ruleset that fails to load at boot (a missing kernel module, a table the
  #     kernel refuses) leaves the unit failed, which is what the network's own
  #     diagnostics read, and the next `nixos-rebuild switch` re-applies it.
  #
  # And no ordering against systemd-networkd, which is the other thing a reader
  # might expect: the unit is `before network-pre.target` and networkd starts after
  # it, so `after = [ "systemd-networkd.service" ]` here would be a start-order
  # cycle for no benefit — nftables matches an interface name as a string and needs
  # no device to exist to load a rule about one.
}
