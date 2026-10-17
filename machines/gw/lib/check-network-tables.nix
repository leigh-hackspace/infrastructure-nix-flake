# check-network-tables.nix
#
# Assertions over the network's two tables — ./common/vlans.nix (the VLANs and
# the block each one's dynamic range hands out of) and ./common/dns-records.nix
# (which device keeps which address) — and over the router's reading of them:
# ./lib/network-tables.nix's generated lines and rules.  Run it with:
#
#   nix eval --impure --raw -f machines/gw/lib/check-network-tables.nix
#
# It prints "ok" or throws with the offending values.  `just check` runs it, and
# ./assert-network-tables.nix runs the *same* assertions as a module so a deploy
# of the router hits them too — one set of assertions, two entry points, which is
# why this file returns a string rather than a set of booleans.
#
# WHY THIS CHECK EXISTS AT ALL, in one sentence: a reservation that names a VLAN
# the router does not create, or two devices claiming one address, is invisible
# until a client gets the wrong address — and unlike every other module in this
# machine's list, that failure cannot be found by deploying and reading the
# rendered file, because the rendered file is *generated from* the same tables
# and so says the same wrong thing.  The check is over the tables, not over the
# rendering.
#
# The assertions are over evaluated values: no derivation, no network, and they
# exercise the exact functions ./dns.nix and ./firewall.nix use rather than a
# copy of them.
let
  VLANS = import ../../../common/vlans.nix;
  RECORDS = import ../../../common/dns-records.nix;
  HOSTS = import ../../../common/hosts.nix;
  CONFIG = import ../../../common/config.nix;
  COEXIST = import ../coexist.nix;
  TABLES = import ./network-tables.nix;

  need = cond: msg:
    if cond
    then true
    else builtins.throw "check-network-tables: ${msg}";

  # Substring test without regex: the needles are full of regex metacharacters
  # (every one of them is an address).  Same helper as
  # common/check-printer-states.nix, same reason.
  has = sub: s: builtins.replaceStrings [sub] [""] s != s;

  vlanIds = map (v: v.id) VLANS;
  vlanById = builtins.listToAttrs (map (v: {
      name = toString v.id;
      value = v;
    })
    VLANS);

  # The block *prefix* of an address-notation block: VLAN 225's `net` is
  # "10.3.1.0/24", so its prefix is "10.3.1." and an address is in the block when
  # it starts with that prefix.  Generated rather than written, which is the point
  # of the check: the block and the address inside it are one fact in the tables
  # and the check is what says the two halves of that fact agree.  The helper is
  # the one in ./network-tables.nix, because "is this address in that block" and
  # "is that VLAN's block inside the ACL" are the same relation, used twice.
  blockPrefix = TABLES.blockPrefix;
  maskOf = s: builtins.elemAt (builtins.split "/" s) 2;

  # An address is inside a VLAN's block when it starts with the block's prefix
  # and the block's netmask is a /24 — which is all the tables use, and the
  # reason the assertion says so rather than pretending to do bitwise math.
  # Not a range comparison: these are dotted-quad strings, and comparing them as
  # strings is what dnsmasq does with them too.
  inside = addr: v: TABLES.inBlock addr v.net && maskOf v.net == "24";

  # The last octet of a /24 address, as a number.  Only meaningful because every
  # block this is used on is a /24 (the `inside` above refuses anything else), and
  # only used for the one thing a prefix test cannot answer: whether an address is
  # between two ends of a dynamic range.  ./../coexist.nix is the only caller.
  lastOctet = addr: builtins.fromJSON (builtins.elemAt (TABLES.quadsOf addr) 3);

  # A VLAN's block is inside an ACL block when the ACL's prefix is a prefix of
  # the VLAN's: the ACL says "10.3.0.0/16" and the VLAN lives in "10.3.1.0/24",
  # which is the same fact nginx answers with an `allow` line.  The ACL is passed
  # as the network writes it (common/config.nix's nginx lines) and converted here
  # — one conversion, one relation, so the check and the firewall ask the same
  # question of the same table.
  coveredByAcl = v: acl: builtins.any (b: has (blockPrefix b) (blockPrefix v.net)) (TABLES.aclBlocks acl);

  recordValues = builtins.attrValues RECORDS;

  # The rendered hosts file, checked as text rather than as a table: the shape of
  # the line is the fact dnsmasq cares about, and both assertions below are about
  # that shape.  The names are the attrset's keys, so they are put on the records
  # here the same way ./network-tables.nix's internal `named` does it.
  RECORDS_HOSTS = TABLES.recordsHosts CONFIG.LOCAL_DOMAIN RECORDS;
  namedRecords =
    builtins.attrValues
    (builtins.mapAttrs (name: r: r // {inherit name;}) RECORDS);
  macs = builtins.concatLists (map (r: r.macs) recordValues);
in
  # --- the VLAN table --------------------------------------------------------
  # VLAN ids are unique: two VLANs with one id are one VLAN wearing two blocks,
  # and dnsmasq's tag (which is the *name*, `vlan<id>`) is the thing that would
  # silently pick one of them.
  assert (
    need
    (builtins.length vlanIds
      == builtins.length (builtins.attrValues (builtins.listToAttrs (map (id: {
          name = toString id;
          value = null;
        })
        vlanIds))))
    "duplicate VLAN id: ${builtins.toJSON vlanIds}"
  );
  # VLAN ids are in the range the kernel accepts (1..4095) and the range the
  # *client* can carry on the wire (1..4095, the same limit — VLAN ids are
  # 12 bits).  The old box's own limit was narrower (1..1023) and VLAN 1337,
  # the CS network's tag, is outside it: the old box never wrote that id, it
  # only ever *saw* one arriving from the other network's router, which is a
  # FreeBSD VLAN device the old box created by hand rather than a VLAN it
  # configured.  So the assertion is the Linux limit, and the sentence here is
  # the reason the flake's limit is not the old box's — a VLAN the router has to
  # *create* with an id above 1023 is a question for the real box, and ./README.md
  # says so.
  assert (
    need
    (builtins.all (id: id > 0 && id <= 4095) vlanIds)
    "a VLAN id outside 1..4095 (the Linux limit): ${builtins.toJSON vlanIds}"
  );
  # Every VLAN's block address is the VLAN's own `ipv4`: the address a client
  # sees while it is on the VLAN, and the thing ./networking.nix writes as the
  # VLAN's gateway and DNS.  A VLAN whose ipv4 is not in its own block is a VLAN
  # whose clients get an address in someone else's block.
  assert (
    need
    (builtins.all (v: inside v.ipv4 v) VLANS)
    ''a VLAN whose ipv4 is not inside its own block: ${builtins.toJSON (map (v: "${v.ipv4} ${v.net}") VLANS)}''
  );
  # Every VLAN's dynamic range is inside the VLAN's own block, and in order.
  # A range that crosses blocks is a VLAN handing out addresses in another
  # VLAN's block, which is the drift ./firewall.nix would then route wrongly —
  # the two halves would disagree about one device, silently.
  assert (
    need
    (
      builtins.all
      (
        v:
          if v.dynFrom == null
          then v.dynTo == null
          else (inside v.dynFrom v) && (inside v.dynTo v)
      )
      VLANS
    )
    ''a VLAN whose dynamic range is not inside its own block: ${builtins.toJSON (map (v: "${v.ipv4} ${v.net} ${v.dynFrom} ${v.dynTo}") VLANS)}''
  );
  # Every VLAN's block is inside the network's own ACL (common/config.nix's
  # LOCAL_NETWORK), which is the list of blocks a *client* may reach.  The two
  # tables are generated from different facts on purpose — the ACL is the
  # network's, the VLAN blocks are the router's — and this assertion is what
  # keeps "the router routes it" and "the network allows it" from being two
  # answers about one block.  A VLAN outside the ACL is a VLAN whose traffic the
  # network's own nginx servers refuse, which is a device that cannot be reached
  # rather than a device that is unreachable.
  assert (
    need
    (builtins.all (v: coveredByAcl v CONFIG.LOCAL_NETWORK) VLANS)
    ''a VLAN whose block is not in the network's ACL: ${builtins.toJSON (map (v: v.net) VLANS)}''
  );
  # --- the records table -----------------------------------------------------
  # Every record's VLAN exists in the VLAN table.  A record naming a VLAN that
  # is not in the table is a reservation dnsmasq will tag with a VLAN device
  # that does not exist, i.e. a device that gets an address and no routing —
  # which is exactly the failure the old box's `<Hostwatch>` section existed to
  # notice, and this check is what notices it before the box does.
  assert (
    need
    (builtins.all (r: r.vlan == null || builtins.elem r.vlan vlanIds) recordValues)
    ''a record naming a VLAN that does not exist: ${builtins.toJSON (map (r: "${r.name} ${r.vlan}") (builtins.filter (r: r.vlan != null && !builtins.elem r.vlan vlanIds) recordValues))}''
  );
  # Every record's address is inside its VLAN's block.  A record outside its own
  # VLAN's block is a device whose address ./firewall.nix will drop traffic to,
  # because the rule it generates allows the VLAN's block and nothing else.
  assert (
    need
    (
      builtins.all
      (
        r:
          if r.vlan == null
          then true
          else inside r.ipv4 vlanById.${toString r.vlan}
      )
      recordValues
    )
    ''a record whose address is not inside its VLAN's block: ${builtins.toJSON (map (r: "${r.name} ${r.ipv4} ${r.vlan}") (builtins.filter (r: r.vlan != null && !inside r.ipv4 vlanById.${toString r.vlan}) recordValues))}''
  );
  # Every record has an identifier: a reservation needs something to match on,
  # and a record with none produces no dhcp-host line at all.  That is not a
  # harmless omission — it is a device that silently falls into its VLAN's
  # dynamic range and moves address, which is what the table exists to prevent.
  assert (
    need
    (builtins.all (r: r.macs != [] || r.clientIds != []) recordValues)
    ''a record with no macs and no clientIds: ${builtins.toJSON (builtins.filter (n: RECORDS.${n}.macs == [] && RECORDS.${n}.clientIds == []) (builtins.attrNames RECORDS))}''
  );
  # No address is claimed twice.  Two records with one address are two devices
  # that cannot both be where the flake says they are, and every other machine's
  # nginx proxy, Prometheus target and gatus check points at one of them.
  assert (
    need
    (builtins.length recordValues
      == builtins.length (builtins.attrValues (builtins.listToAttrs (map (r: {
          name = r.ipv4;
          value = null;
        })
        recordValues))))
    ''duplicate ipv4 across records: ${builtins.toJSON (map (r: r.ipv4) recordValues)}''
  );
  # No MAC is claimed twice: one MAC on two addresses is one device that the
  # router answers with two addresses, and dnsmasq's answer is whichever line it
  # reads last, which is the table's order rather than anyone's decision.
  assert (
    need
    (builtins.length macs
      == builtins.length (builtins.attrValues (builtins.listToAttrs (map (m: {
          name = m;
          value = null;
        })
        macs))))
    ''duplicate MAC across records: ${builtins.toJSON macs}''
  );
  # --- the router's reading of the tables ------------------------------------
  # Every address the flake's own machines live on is an address the router
  # answers: common/hosts.nix is the table every other machine's config reads,
  # and this is what says the router's reservations cover it.  The router's own
  # address is the exception, and it is answered by VLAN 225's `ipv4` rather
  # than by a reservation (the router has no lease of its own — see
  # common/dns-records.nix's header).
  assert (
    need
    (
      builtins.all
      (
        a:
          builtins.any (r: r.ipv4 == a) recordValues
          || builtins.any (v: v.ipv4 == a) VLANS
      )
      [HOSTS.services1 HOSTS.services2 HOSTS.aibox HOSTS.nas HOSTS.router]
    )
    ''an address common/hosts.nix promises that no reservation answers: ${builtins.toJSON [HOSTS.services1 HOSTS.services2 HOSTS.aibox HOSTS.nas HOSTS.router]}''
  );
  # The generated lines, exercised through the same functions ./dns.nix and
  # ./firewall.nix use rather than a copy of them: the VLAN tag in a dhcp-range
  # line and the VLAN device in an `interface=` line have to be the same name
  # ./networking.nix creates, and the one place that name is spelled differently
  # is the trap common/vlans.nix's header is about (FreeBSD's `vlan0.<id>` vs
  # Linux's `vlan<id>`).  A VLAN that has a dynamic range but no dhcp-range
  # line is a VLAN whose clients get no lease, which is a network outage rather
  # than a missing line anyone reading the conf would notice.
  assert (
    need
    (
      builtins.all
      (
        v:
          v.dynFrom
          == null
          || has "dhcp-range=tag:${TABLES.vlanDevice v}" (TABLES.rangeLines VLANS)
      )
      VLANS
    )
    ''a VLAN with a dynamic range and no dhcp-range line: ${builtins.toJSON (map (v: v.id) (builtins.filter (v: v.dynFrom != null && !has "dhcp-range=tag:${TABLES.vlanDevice v}" (TABLES.rangeLines VLANS)) VLANS))}''
  );
  # Every record produces at least one dhcp-host line, i.e. the generation and
  # the table agree about how many reservations there are.  The count is the
  # thing that catches a record whose identifiers are all empty strings, which
  # the assertion above does not (it only asks that the lists are non-empty).
  assert (
    need
    (
      builtins.length
      (builtins.filter
        (l: has "dhcp-host=" l)
        (TABLES.lines (TABLES.hostLines RECORDS)))
      == builtins.length
      (
        builtins.concatLists
        (map (r: r.macs ++ (map (c: "id:${c}") r.clientIds)) recordValues)
      )
    )
    ''the generated dhcp-host lines and the table's identifiers disagree: ${builtins.toJSON (map (r: "${r.name} ${builtins.length (r.macs ++ r.clientIds)}") recordValues)}''
  );
  # The netboot variants' VLANs exist, and their tags are the tags the dhcp-boot
  # half names: the two halves of a netboot variant name each other by tag, and
  # this is what says the generation did not lose one.
  assert (
    need
    (
      builtins.all
      (b: builtins.elem b.vlan vlanIds)
      TABLES.netbootVariants
    )
    ''a netboot variant naming a VLAN that does not exist: ${builtins.toJSON (map (b: b.vlan) (builtins.filter (b: !builtins.elem b.vlan vlanIds) TABLES.netbootVariants))}''
  );
  # The ACL renders as bare blocks that are still address notation: the stripping
  # of nginx's `allow ` prefix and `;` is the one place the network's table is
  # translated, and a translation that leaves a semicolon in a rule is a rule
  # nft will not parse — which nixpkgs' nftables module would catch at build
  # time, but not at `nix flake check`, which is where this assertion runs.
  assert (
    need
    (
      builtins.all
      (b: !has ";" b && !has "allow" b && !has " " b && has "/" b)
      (TABLES.aclBlocks CONFIG.LOCAL_NETWORK)
    )
    ''an ACL entry that does not render as a bare block: ${builtins.toJSON (TABLES.aclBlocks CONFIG.LOCAL_NETWORK)}''
  );
  # Every ACL entry is IPv4 or IPv6 notation, decided by the one character only
  # IPv6 notation has.  Written as a test of the filter the firewall uses rather
  # than of the entries' shape, because the filter and the entries are the two
  # halves of one claim: `ipv4AclBlocks` says which entries are IPv4, and this says
  # it is exactly the ones with no colon.  An entry that is neither notation (a
  # hostname, a comment that survived the stripping) would otherwise vanish from
  # the firewall's set silently, and "the ACL allows it" would stop being true.
  assert (
    need
    (
      builtins.all
      (b: (has ":" b) == (!builtins.elem b (TABLES.ipv4AclBlocks CONFIG.LOCAL_NETWORK)))
      (TABLES.aclBlocks CONFIG.LOCAL_NETWORK)
    )
    ''an ACL entry the firewall's IPv4/IPv6 split cannot place: ${builtins.toJSON (TABLES.aclBlocks CONFIG.LOCAL_NETWORK)}''
  );
  # The two halves partition the ACL: every entry is in exactly one of them.  The
  # assertion above checks each entry is placed; this one checks nothing is counted
  # twice or lost, which is the failure a filter pair drifts into (a filter that
  # matches both halves would let a v6 block into the v4 set and nft would refuse
  # the ruleset, but a filter that matches *neither* would silently drop an entry
  # from the firewall and the ACL would stop meaning what it says).
  assert (
    need
    (builtins.length (TABLES.ipv4AclBlocks CONFIG.LOCAL_NETWORK)
      + builtins.length (TABLES.ipv6AclBlocks CONFIG.LOCAL_NETWORK)
      == builtins.length (TABLES.aclBlocks CONFIG.LOCAL_NETWORK))
    ''the ACL's IPv4/IPv6 halves do not add up to the ACL: ${builtins.toJSON {
        v4 = TABLES.ipv4AclBlocks CONFIG.LOCAL_NETWORK;
        v6 = TABLES.ipv6AclBlocks CONFIG.LOCAL_NETWORK;
      }}''
  );
  # The ACL's IPv4 half is what the firewall's set is built from, and it is
  # non-empty: an ACL that renders as an empty set is a firewall that drops
  # every packet, which is a network outage rather than a rule anyone reading
  # the rendered file would notice.
  assert (
    need
    (builtins.length (TABLES.ipv4AclBlocks CONFIG.LOCAL_NETWORK) > 0)
    "the ACL renders no IPv4 blocks at all"
  );
  # --- the generated hosts file (./dns.nix's `dnsmasq-records.hosts`) ---------
  #
  # Every record has to be answerable by its FQDN, not only by its short name.
  # This is the check that would have caught the shadow router answering
  # `services1` and answering nothing at all for `services1.int.leighhack.org`
  # (measured against the live router on 2026-10-17): dnsmasq's `local=` makes this
  # box authoritative for the zone and does *not* complete a hosts entry, so a
  # file of bare names is a silent empty answer for every vhost, monitoring
  # endpoint and dns-sync name on the network — the whole reason the network has a
  # router DNS at all. Asserted against the rendered text rather than the table
  # because the shape of the line is the fact: dnsmasq reads only the first address
  # on a line and treats the rest as names, so an IPv4 and its IPv6 sharing one
  # line is the same class of bug and this check sees it too.
  assert (
    need
    (
      builtins.all
      (r: has "${r.ipv4} ${r.name}.${CONFIG.LOCAL_DOMAIN} ${r.name}" RECORDS_HOSTS)
      namedRecords
    )
    ''a record whose FQDN is not in the generated hosts file: ${
        builtins.toJSON (
          map
          (r: r.name)
          (
            builtins.filter
            (r: !has "${r.ipv4} ${r.name}.${CONFIG.LOCAL_DOMAIN} ${r.name}" RECORDS_HOSTS)
            namedRecords
          )
        )
      }''
  );
  # And one line per address: dnsmasq reads only the first address on a hosts line
  # and treats everything after it as a name, so a record's IPv4 and IPv6 sharing a
  # line registers the v6 address as an alias *name* — a wrong answer, not an error.
  # The line count is the observable: one per IPv4, plus one per IPv6.
  assert (
    need
    (
      builtins.length (builtins.filter (l: l != "") (TABLES.lines RECORDS_HOSTS))
      == (
        builtins.foldl'
        (n: r: n + 1 + builtins.length r.ipv6)
        0
        recordValues
      )
    )
    ''the generated hosts file does not have one line per address: ${
        toString (builtins.length (builtins.filter (l: l != "") (TABLES.lines RECORDS_HOSTS)))
      } lines for ${
        toString (builtins.foldl' (n: r: n + 1 + builtins.length r.ipv6) 0 recordValues)
      } addresses''
  );
  # --- co-existence (./../coexist.nix) ---------------------------------------
  #
  # These four are about the one number that file rewrites: the host part this box
  # takes on every VLAN while the live router still answers on .1.  They are here
  # rather than in that file because this is the file that prints a finding rather
  # than throwing an eval error, and because the thing being checked is a
  # relationship between the co-exist address and the *two tables* — which is what
  # this file exists to be asked about.
  #
  # They hold in both modes, so nothing here is gated on `COEXIST.enable`: with
  # co-existence off the address under test is .1, and "the router's own address
  # is not leasable and not reserved" is worth asserting then too.
  # The host part is a host part: an octet, not a fifth one.  `hostAddr` in
  # ./network-tables.nix refuses a non-numeric host; this refuses 255 (a /24's
  # broadcast address, which no interface may hold) and anything above it.
  assert (
    need
    (lastOctet (TABLES.hostAddr COEXIST.hostAddr "0.0.0.0") < 255)
    ''coexist.nix's hostAddr "${COEXIST.hostAddr}" is not a usable host part (1-254)''
  );
  # Every VLAN's own address — the one ./networking.nix puts on the interface and
  # dnsmasq offers as the DNS server — is inside that VLAN's block.  Trivially true
  # of common/vlans.nix (asserted above); this is the same claim about the
  # rewritten table, which is the one that actually gets rendered.
  assert (
    need
    (builtins.all (v: inside v.ipv4 v) COEXIST.vlans)
    ''a co-exist VLAN address outside its own block: ${builtins.toJSON (map (v: "${v.ipv4} ${v.net}") (builtins.filter (v: !inside v.ipv4 v) COEXIST.vlans))}''
  );
  # No VLAN's dynamic range may contain this box's own address.  While the old
  # router is live it is the one handing out leases, and a lease for the address
  # the shadow router is sitting on is two devices on one address — the failure is
  # silent, arrives on the old router's own schedule (a client asking for a name,
  # hours later), and looks like a broken NIC rather than like this file.
  assert (
    need
    (
      builtins.all
      (
        v:
          v.dynFrom
          == null
          || !(
            (lastOctet v.ipv4 >= lastOctet v.dynFrom)
            && (lastOctet v.ipv4 <= lastOctet v.dynTo)
          )
      )
      COEXIST.vlans
    )
    ''a VLAN whose dynamic range contains this box's own address: ${
        builtins.toJSON (
          map
          (v: "${v.ipv4} in ${v.dynFrom}-${v.dynTo}")
          (
            builtins.filter
            (
              v:
                v.dynFrom
                != null
                && (
                  (lastOctet v.ipv4 >= lastOctet v.dynFrom)
                  && (lastOctet v.ipv4 <= lastOctet v.dynTo)
                )
            )
            COEXIST.vlans
          )
        )
      }''
  );
  # And no reservation in common/dns-records.nix claims it either — same failure,
  # arriving through the reservation table instead of through a lease pool.
  assert (
    need
    (
      builtins.all
      (v: !builtins.any (r: r.ipv4 == v.ipv4) recordValues)
      COEXIST.vlans
    )
    ''a co-exist VLAN address that a device has reserved: ${
        builtins.toJSON (map (v: v.ipv4) (builtins.filter (v: builtins.any (r: r.ipv4 == v.ipv4) recordValues) COEXIST.vlans))
      }''
  );
  # The transit gateway — the live router this box routes itself out through while
  # it has no uplink of its own — is inside exactly one VLAN block and is not one
  # of this box's own addresses.  "Inside none" is a box with no route and no
  # error; "inside two" is a default route nobody can say the interface of; "is
  # this box" is a routing loop, and it is also the sign that co-existence has been
  # left on with the host part set to .1 — i.e. two routers claiming one address.
  # Gated on the mode, unlike the three above: with co-existence off there is no
  # transit gateway at all (the box's route out is ./uplink.nix's tunnel), and the
  # address this file would be objecting to is the box's own legitimate one.
  # ./../coexist.nix throws on the same condition with less detail, so the two
  # agree by construction: this is the version an operator can read.
  assert (
    need
    (
      !COEXIST.enable
      || (
        builtins.length (builtins.filter (v: inside COEXIST.transitGateway v) VLANS)
        == 1
        && !builtins.any (v: v.ipv4 == COEXIST.transitGateway) COEXIST.vlans
      )
    )
    ''coexist.nix is on, so its transitGateway must be a router still in the job: inside exactly one VLAN block, and not one of this box's own addresses. It is "${COEXIST.transitGateway}" against ${
        builtins.toJSON (map (v: "${v.id} ${v.net} ${v.ipv4}") COEXIST.vlans)
      }''
  ); "ok\n"
