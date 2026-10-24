# The router's packet decisions: what may be forwarded, and what may reach the
# router itself.  This is the flake's version of the rules the old box ran.  The
# old box ran **pf** (OPNsense's generated ruleset; `pfctl -sr`/`-sn` read on
# 2026-10-17), while this box runs nftables — the tool changes, and the source of
# truth changes with it: common/vlans.nix and common/config.nix here instead of
# the XML there.  The old pf rules are quoted where they matter below because the
# semantics are what is being copied, not the syntax.
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
#   2. **The old box's exact rules.**  They were read on 2026-10-17 with
#      `pfctl -sr`, `pfctl -sn` and `pfctl -sa` (the old box runs pf, not nft,
#      despite the comment above): the per-VLAN `route-to` split is what the
#      marks below copy, and the old box's NAT was the two `nat on` rules in
#      `pfctl -sn`.  Anything below that contradicts the archive is an accident,
#      not a decision, and "the rules parse" is not "the rules are the network's
#      rules".
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
  # The network's VLAN table — the same list ./networking.nix creates the devices
  # from and ./dns.nix answers DHCP on, so a VLAN id, block or address means one
  # thing in nftables and in dnsmasq (two lists that happen to agree today are the
  # drift this directory keeps removing).
  VLANS = import ../../common/vlans.nix;
  CONFIG = import ../../common/config.nix;
  HOSTS = import ../../common/hosts.nix;
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
    # (common/config.nix's TAILNET_V4).  DNS only: a DHCP request arriving
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
    # The tunnel's DHCPv6 client, which asks AAISP for the /48 and the public
    # address.  The reply comes from the peer's link-local address and is not
    # `established` from the client's point of view, exactly like the DHCPv4
    # reply above.
    iifname "${NICS.UPLINK_TUNNEL}" udp sport 547 udp dport 546 accept
  '';

  # --- the front door: HTTP(S) *to the router itself* -----------------------
  #
  # ./http.nix — phase 1 of docs/router-front-door-migration.md, and the two facts
  # about these four lines are worth more than the lines:
  #
  # * **Matched by interface, not by `ip saddr ${aclV4}`** — the opposite of the ssh
  #   rule below, deliberately.  ssh is keyed on *who* you are; the front door is
  #   keyed on *where it answers*, because the one property that keeps a `*.int` vhost
  #   private on this network is that every address a `*.int` name resolves to is a
  #   LAN-side one.  Read the other way round: common/config.nix's ACL contains
  #   **public** addresses (Chris's Zen /32, Kian's /32, two /48s), so an ACL-shaped
  #   rule here would be a listener on this box's own public IPv6
  #   (`2001:8b0:1d14:225::1`) and on the tunnel's IPv4 — both routable from anywhere,
  #   reachable by design from the handful of addresses the ACL trusts, and answering
  #   internal vhosts.  Today no public address answers a `*.int` name at all, and
  #   these rules are not the thing that changes that.  With the interface match the
  #   door answers exactly where DNS answers — the `vlan*` and `tailscale0` interfaces
  #   ./dns.nix serves, and nothing else — which is the invariant worth writing down:
  #   *if you can resolve the name from here, you can reach the front door from here*.
  #   nginx's per-vhost `LOCAL_NETWORK` is still what decides which name a given
  #   client may have; this is only which direction the listener answers at all.
  # * **No DNAT row is added.** The internet's 80/443 still translate to services1
  #   (`portForwards` above) and nothing on the public addresses answers HTTP here.
  #   This is the network's own traffic arriving, not a new door from outside — and
  #   `docs/router-front-door-migration.md`'s "The decision" is why it stays that way.
  frontDoorRules = ''
    iifname "vlan*" tcp dport { 80, 443 } accept
    iifname "tailscale0" tcp dport { 80, 443 } accept
  '';

  # The destinations that are *not* the internet for the purpose of the routing
  # marks below: the old box's `<Local>` alias (10.3.0.0/16 and 192.168.2.0/24),
  # the tailnet (common/config.nix's `TAILNET_V4`, and the old box's `TAILSCALE`
  # alias was the same /10 — this list said /16 for a while, which is a node
  # outside 100.64.0.0/16 being marked for the uplink and dialled out of the
  # building instead of into the tailnet), the tailnet subnet route at Chris's
  # house, and the AAISP hub's own handoff network.  Traffic to one of these must
  # keep the route it already has (connected, LAN, tailscale0, or the hub's own
  # link) rather than being forced into a table whose default is further away.
  #
  # `DMZ_NET` is in the list because of NAT reflection: a LAN client reaching a
  # public name dials 81.187.195.17, and this chain runs at priority mangle, i.e.
  # **before** the `dstnat` chain translates it back onto the LAN.  Without this
  # entry the packet is marked for the hub and the policy route sends it out the
  # uplink instead of back out the VLAN to 10.3.1.20 — the mark has to say "this
  # destination is ours" before the DNAT has a chance to.
  markLocalV4 = TABLES.setOf [
    "10.3.0.0/16"
    "192.168.2.0/24"
    "192.168.49.0/24"
    CONFIG.TAILNET_V4
    "192.168.20.0/24"
    NICS.DMZ_NET
  ];

  # One VLAN's mark, which is the flake's version of the old box's
  # `pass in quick on vlan0.<id> route-to (...)`.  VLAN 225 (the trusted wired
  # LAN) was the one the old box sent out of the AAISP tunnel on IPv4; every
  # other VLAN went out of the hub's own uplink.  VLAN 225's IPv6 went out of the
  # tunnel like every other VLAN's, which needs no mark here because the tunnel's
  # ip-up script owns the IPv6 default.
  #
  # The mark is set in prerouting, before the route decision, and the matching
  # `ip rule` is in ./networking.nix.  `meta mark set` on a packet in a table
  # that is only used for this is safe: nothing else on the box reads marks, and
  # conntrack keeps the decision for the connection's later packets through the
  # established rule.
  vlanMarkRules =
    builtins.concatStringsSep "\n"
    (
      map
      (
        v: ''
          iifname "${TABLES.vlanDevice v}" ip daddr != ${markLocalV4} meta mark set ${
            if v.id == 225
            then "1"
            else "2"
          }
        ''
      )
      VLANS
    );

  # --- inbound NAT: what the internet may reach ----------------------------
  #
  # The old box's `pfctl -sn` archive has 128 `rdr` lines for these, because
  # OPNsense emitted one line per interface (l2tp0, every VLAN, lo0, ovpns1) for
  # NAT reflection.  Folded to one row per (public address, port), it is this
  # table, and nothing else from outside reaches the LAN: the forward chain's
  # policy is `drop`, so a row here is the only way to create a `ct status dnat`
  # packet, and a port not in the table does not exist from the internet.
  #
  # **The policy this table implements is: 80, 443 and tailscale.**  The retired
  # box's archive had more (`DMZ_SERVICES1 :80,:443,:8522,:3478,:50443,:51820` and
  # `DMZ_YUNOHOST :80,:443`), and the cutover copied it faithfully; the 2026-10-21
  # audit after the box went live removed the rows that are neither web nor
  # tailscale:
  #
  #   * 8522 — GitLab's SSH port.  Not a web service, and nothing was listening
  #     on services1:8522 by the time of the audit (`ss -lnt`).  Use the tailnet
  #     for administrative SSH.
  #   * 50443 — the old box's "matrix/livekit, etc." port.  Nothing listens on
  #     services1:50443, so it was an open port to a closed door.
  #   * 51820 — WireGuard to the NAS.  A VPN, not tailscale, so it is out of the
  #     allowed set; tailscale is the network's remote-access path.
  #
  # What remains is the web pair on both public addresses, plus 3478/udp: the
  # public port of headscale's DERP STUN socket (`stun_listen_addr =
  # 0.0.0.0:3479` in machines/services1/services/headscale.nix; UniFi claims 3478,
  # which is why it is remapped).  STUN is UDP — the old table wrote it as TCP,
  # which opened a TCP port to a UDP service and left tailnet clients' STUN
  # unassisted.  `protocol` is per-row for that reason.
  portForwards = [
    {
      public = NICS.PUBLIC_SERVICES1;
      port = 80;
      to = HOSTS.services1;
      toPort = 80;
    }
    {
      public = NICS.PUBLIC_SERVICES1;
      port = 443;
      to = HOSTS.services1;
      toPort = 443;
    }
    {
      public = NICS.PUBLIC_SERVICES1;
      port = 3478;
      protocol = "udp";
      to = HOSTS.services1;
      toPort = 3479;
    }
    {
      public = NICS.PUBLIC_YUNOHOST;
      port = 80;
      to = NICS.YUNOHOST;
      toPort = 80;
    }
    {
      public = NICS.PUBLIC_YUNOHOST;
      port = 443;
      to = NICS.YUNOHOST;
      toPort = 443;
    }
  ];

  # One rule per row.  The port is only written when it changes: `dnat to
  # <address>` keeps the matched port, which is dnsmasq's/OPNsense's own `->
  # <host>` form and not the same thing as `dnat to <address>:0`.  `protocol`
  # defaults to tcp so the web rows do not repeat it.
  dnatRules =
    builtins.concatStringsSep "\n"
    (
      map
      (f: ''
        ip daddr ${f.public} ${f.protocol or "tcp"} dport ${toString f.port} dnat to ${f.to}${
          lib.optionalString (f.toPort != f.port) ":${toString f.toPort}"
        }
      '')
      portForwards
    );

  # --- inbound NAT's IPv6 counterpart --------------------------------------
  #
  # The `dstnat` chain has no v6 twin, because none is needed: an internet client
  # dialling a public name is *already* addressing the box that answers it, since
  # every `*.leighhack.org` vhost publishes a AAAA (`2001:8b0:1d14:225::20`) rather
  # than a NAT'd v4 pair.  What that means is the opposite of convenient — the v6
  # forward chain's `policy drop` answers nothing at all unless a rule names the
  # destination by address, so "public IPv6 inbound" is a rule here rather than a
  # side effect of the NAT, and forgetting it is silent (the site works over IPv4,
  # from the LAN, and from anyone whose address happens to be in the shared ACL).
  #
  # This started as the old box's three "Public IPv6 Services" rules:
  # `pass in quick on l2tp0 reply-to (l2tp0 fe80::9e89:1eff:fe3f:0) inet6 proto tcp
  # from any to <SERVICES1_IPV6> port = {http, https, 8522}` (read out of the
  # archived `pf.txt`, `~/tmp/opnsense-archive-20261017` on services1).  The
  # 2026-10-21 audit cut 8522 (GitLab SSH, not web, and nothing listening) to
  # match the v4 policy: **80 and 443 only**.  3478/50443/51820 were never
  # allowed in over v6 and stay that way — headscale's DERP has an IPv6 *address*
  # but a STUN socket reachable over v4 only (docs/follow-ups.md).  The yunohost
  # appliance has no row because `yunohost.leighhack.org` has no AAAA.
  #
  # Matched on the tunnel rather than on "not a VLAN" for the same reason the old
  # rules named `l2tp0`: the hub's handoff has no public IPv6 to arrive on, and an
  # interface-less rule would let the mill's segment pose as the internet.  The
  # reply path needs no rule of its own — services1's answer is `established`, and
  # VLAN 225's routing mark sends it back out of the tunnel, which is the job the
  # pf `reply-to` did.
  #
  # `counter` is the one on this file, and it is here because this rule's only failure
  # mode is silence and it cannot be proven from inside the network: a LAN client
  # matches `ip6 saddr 2001:8b0:1d14::/48` two rules later, so every local test passes
  # whether or not the internet can arrive. Read it after a probe from a v6 address
  # that is not in the ACL — `nft list chain ip6 router6 forward`.
  publicV6ForwardRules = ''
    iifname "${NICS.UPLINK_TUNNEL}" ip6 daddr ${TABLES.setOf NICS.PUBLIC_SERVICES1_V6} tcp dport { 80, 443 } counter accept
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
          # The policy-routing half: mark before the routing decision, then let
          # ./networking.nix's two fwmark rules pick table 100 (AAISP) or table
          # 200 (the hub).  This is the old box's `route-to` split, and it is in
          # the same table as the forward chain so the two halves cannot be
          # installed apart.
          chain prerouting {
            type filter hook prerouting priority mangle; policy accept;
            ${vlanMarkRules}
            # The tailnet is a LAN-side network: a packet arriving on tailscale0
            # for the internet is forwarded traffic from a remote node, and the
            # old box sent it out of the hub (`route-to (em3 ...)` on
            # tailscale0).  A packet for the LAN never reaches this mark because
            # its destination is in markLocalV4.
            iifname "tailscale0" ip daddr != ${markLocalV4} meta mark set 2
          }

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

            # A packet this box translated on the way in (the `dstnat` chain in
            # the nat table below) is one the internet asked for, and its source
            # is never in the ACL.  The old box's `rdr pass` lines admitted those
            # directly and its non-`pass` ones (80/443, 51820) were matched by a
            # filter rule keyed on the internal address; one rule says both here.
            # It sits after the anti-spoof rules on purpose: a translated packet
            # must still not be able to claim another LAN address.
            ct status dnat accept

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
            # started (its own DNS upstream, ntp, and the ssh that `just switch-gw`
            # and every debugging session from services1 arrive over).
            iifname "lo" accept
            ct state established,related accept
            ct state invalid drop

            ${lanServiceRules}
            ${uplinkServiceRules}
            ${frontDoorRules}

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

            # The network dashboard (./network-status.nix) listens on
            # 10.3.1.1:8091.  Both of its *remote* clients are on services1 — the
            # nginx that terminates network-info.int.leighhack.org and Prometheus —
            # so this is one address, not the ACL: the JSON API is
            # unauthenticated and has no other consumer.  Binding the LAN
            # address in the unit and this rule are two halves of the same
            # decision.  (./http.nix now proxies the same port from this box too,
            # which needs no rule: a locally-generated request to a local address
            # arrives on `lo`.  The remote client is the one this line is for, and
            # it stays services1 until the answer moves — see
            # docs/router-front-door-migration.md.)
            ip saddr ${HOSTS.services1} tcp dport 8091 accept
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
          # Inbound: the internet reaching a service.  This is the half that makes
          # a public vhost exist at all — without it the forward chain's policy
          # drops every packet from outside, which is what the network looked like
          # after the cutover (every 81.187.195.17/:80/:443 timed out while the
          # same vhost answered over IPv6 and on the LAN).
          chain dstnat {
            type nat hook prerouting priority dstnat; policy accept;
            ${dnatRules}
          }

          chain srcnat {
            type nat hook postrouting priority srcnat; policy accept;
            # The trusted VLAN out of the AAISP tunnel (the old box's
            # `nat on l2tp0 ... -> (l2tp0:0)` half).
            oifname "${NICS.UPLINK_TUNNEL}" ip saddr ${TABLES.blockSet VLANS} masquerade
            # Every other VLAN out of the hub, which the old box did not need a
            # nat rule for because its `nat on em3` rule covered the interface
            # rather than the source; the same rule here is written with the
            # source block so a packet arriving from the hub cannot be
            # masqueraded back out of it.
            oifname "${NICS.UPLINK_NIC}" ip saddr ${TABLES.blockSet VLANS} masquerade
            # The tailnet, when this box is its exit node: the old box sent
            # tailscale0's traffic out of the hub (`route-to (em3 ...)` on
            # tailscale0), and the hub only routes the address it gave this box.
            # The tailnet's own range is common/config.nix's TAILNET_V4.
            oifname "${NICS.UPLINK_NIC}" ip saddr ${CONFIG.TAILNET_V4} masquerade
            # NAT reflection, which is the old box's `rdr on vlan0.X` copies of
            # every forward: a LAN client resolving a *public* name gets the public
            # address from the network's resolver (the *.leighhack.org names live
            # in DigitalOcean, not in dnsmasq), so its packet arrives on a VLAN and
            # is translated back onto the LAN.  Without this the internal host's
            # reply goes straight to the client from an address the client never
            # dialled, and the connection hangs — indistinguishable from a dead
            # service.
            #
            # Keyed on the source block *and* `ct status dnat`, so ordinary
            # LAN-to-LAN traffic keeps its real source address and services1 still
            # logs the client rather than the router.
            ip saddr ${TABLES.blockSet VLANS} ct status dnat oifname "vlan*" masquerade
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
            ${publicV6ForwardRules}
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
            ${frontDoorRules}
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
