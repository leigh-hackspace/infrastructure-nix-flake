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
  INT = import ../../../common/int-dns.nix;
  TABLES = import ./network-tables.nix;

  # The connection table's "ours", derived the way ../network-status.nix derives
  # it, so the assertions below are about the same list the router is handed.
  localPrefixes = TABLES.localPrefixes VLANS;
  localPrefixesV4 = builtins.filter (p: !has ":" p) localPrefixes;

  # The same set with a `vlan<id>=` in front of every prefix, which is the form
  # ../network-status.nix hands to `--vlan` (see the assertion about it below).
  # `builtins.split "="` on one spec gives [name [] cidr], so the two halves of a
  # spec are element 0 and element 2.
  vlanPrefixes = TABLES.vlanPrefixes VLANS;
  unlabelled = map (spec: builtins.elemAt (builtins.split "=" spec) 2) vlanPrefixes;
  labelledVlans = map (spec: builtins.elemAt (builtins.split "=" spec) 0) vlanPrefixes;
  vlanDevices = map (v: TABLES.vlanDevice v) VLANS;

  need = cond: msg:
    if cond
    then true
    else builtins.throw "check-network-tables: ${msg}";

  # Substring test without regex: the needles are full of regex metacharacters
  # (every one of them is an address).  Same helper as
  # common/check-printer-states.nix, same reason.
  has = sub: s: builtins.replaceStrings [sub] [""] s != s;

  # Prefix test, same reason as `has` and the same reason it is not
  # `lib.hasPrefix`: this file is evaluated by `nix eval -f`, which has no nixpkgs
  # lib in scope, and pulling one in would make a table check depend on an
  # evaluation of its own.
  hasPrefix = prefix: s: builtins.substring 0 (builtins.stringLength prefix) s == prefix;

  vlanIds = map (v: v.id) VLANS;

  # The /64s the network advertises, in the CIDR form `TABLES.localPrefixes`
  # produces, and the same question for one address: is it inside one of them?
  # `tryEval` because `v6Block64` throws on an address it cannot read as being in
  # a /64, and in an assertion a malformed address must name its record rather
  # than kill the evaluation.
  advertisedV6Prefixes = builtins.filter (p: has ":" p) localPrefixes;
  inAdvertisedV6 = addr: let
    block = builtins.tryEval (TABLES.v6Block64 addr);
  in
    block.success && builtins.elem block.value advertisedV6Prefixes;

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
  # between two ends of a dynamic range — the two assertions this file makes about a VLAN's own
  # address and its `dynFrom`-`dynTo` pool.
  lastOctet = addr: builtins.fromJSON (builtins.elemAt (TABLES.quadsOf addr) 3);

  # A block is inside the network's ACL when one of the ACL's prefixes is a
  # prefix of it: the ACL says "10.3.0.0/16" and the VLAN lives in "10.3.1.0/24",
  # which is the same fact nginx answers with an `allow` line.  The ACL is passed
  # as the network writes it (common/config.nix's nginx lines) and converted here
  # — one conversion, one relation, so the check and the firewall ask the same
  # question of the same table.
  #
  # Takes the *block* rather than the VLAN record because two kinds of thing ask
  # this question: a VLAN's own block, and a bare prefix out of
  # TABLES.localPrefixes.  Two callers, one relation.
  #
  # IPv4 only, and not because IPv6 does not matter: `blockPrefix` counts dotted
  # quads, so asking it about `fd7a:115c:a1e0::/48` dies in `elemAt`.  Filtering
  # here is what makes a *failing* check report the block that failed — before it,
  # this held together only because every VLAN block matched one of the ACL's IPv4
  # entries first and `any` stopped before reaching the IPv6 ones, so any block
  # that was genuinely uncovered crashed the evaluation instead of naming itself.
  # The same split ./firewall.nix makes with `ipv4AclBlocks`, for the same reason.
  blockCoveredByAcl = net: acl:
    builtins.any (b: has (blockPrefix b) (blockPrefix net)) (TABLES.ipv4AclBlocks acl);
  coveredByAcl = v: acl: blockCoveredByAcl v.net acl;

  # Records with their key as a `name` field, which is what most of the failure
  # messages below interpolate.  This used to be `builtins.attrValues RECORDS`,
  # and `name` is not a field of a record — it is the key — so four of this file's
  # messages ("a record naming a VLAN that does not exist", "…whose address is not
  # inside its VLAN's block", "…IPv4 address does not appear on exactly one
  # dhcp-host line", and the AAAA one added 2026-10-26) died with `attribute 'name'
  # missing` instead of naming the record that triggered them, which is the whole
  # reason they exist. Two of them then died a second time on `${r.vlan}`
  # coercing an integer to a string, which is why those two now say `toString`.
  # Measured 2026-10-26 by making records misbehave on purpose: the AAAA refusal
  # names `services2 -> 2001:8b0:1d14:227d::1001` and the VLAN refusal names its
  # record, where before both produced an evaluator error about this file.
  # `TABLES.named` is the same map, but it is a `let` binding inside
  # ./network-tables.nix rather than an exported attribute, so it is spelled out
  # here.
  recordValues = builtins.attrValues (builtins.mapAttrs (name: r:
    r
    // {
      inherit name;
    })
  RECORDS);

  # The rendered hosts file, checked as text rather than as a table: the shape of
  # the line is the fact dnsmasq cares about, and both assertions below are about
  # that shape.  The names are the attrset's keys, so they are put on the records
  # here the same way ./network-tables.nix's internal `named` does it.
  RECORDS_HOSTS = TABLES.recordsHosts CONFIG.LOCAL_DOMAIN RECORDS;
  # The front door's file, rendered the same way ./dns.nix renders it, so what is
  # asserted below is the text dnsmasq will read rather than the table it came from.
  INT_HOSTS = TABLES.intHosts CONFIG.LOCAL_DOMAIN RECORDS HOSTS VLANS INT;
  # Every name the table answers with the front door's addresses: the vhosts that
  # need it and the legacy names kept so this file's first deploy changed no
  # answer.  Both lists get the same treatment from dnsmasq and from the checks
  # below; only the drift check on services1 tells them apart.
  INT_DOOR_NAMES = INT.names ++ builtins.attrNames INT.legacy;
  # The addresses every front-door name must answer with.  Read through the same
  # helper ./dns.nix renders with, never written here: "the name has its IPv6 half"
  # is only a fact if it is the same fact the table that owns the box's address
  # states — common/dns-records.nix for a device door, common/vlans.nix for the
  # router (docs/router-front-door-migration.md).  `[]` is a finding, asserted below.
  DOOR_ADDRS = TABLES.doorAddresses RECORDS HOSTS VLANS INT.frontDoor;
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
  # --- the connection table's idea of "ours" --------------------------------
  # `TABLES.localPrefixes` is what the dashboard's `--vlan`/`--local-prefix` flags
  # are made of, and the whole answer to "who is downloading, and is that one of
  # ours" is decided by it.  Checked with the same containment relation as every
  # other assertion here, for the same reason the firewall uses it: a prefix the
  # network's own ACL refuses must not be labelled internal on the page either,
  # or the dashboard would be showing a stranger's traffic as one of ours.
  #
  # Only the IPv4 half is checked, and that is the *relation's* limit rather than
  # a choice: `blockPrefix` counts dotted quads, so an IPv6 block has no prefix in
  # this sense (see `blockCoveredByAcl`).  Doing the IPv6 half needs an IPv6
  # containment helper that nothing else on this box asks for yet —
  # `docs/follow-ups.md` has it as an open item.
  assert (
    need
    (builtins.all (net: blockCoveredByAcl net CONFIG.LOCAL_NETWORK) localPrefixesV4)
    ''a --local-prefix the network's own ACL does not allow: ${builtins.toJSON (builtins.filter (net: !blockCoveredByAcl net CONFIG.LOCAL_NETWORK) localPrefixesV4)}''
  );
  # Every VLAN's own block, and only blocks: a VLAN that silently drops out of the
  # derivation would label its devices' traffic as external, which the page shows
  # as "neither end is ours" — a wrong answer that looks like a network fact.
  assert (
    need
    (builtins.all (v: builtins.elem v.net localPrefixes) VLANS)
    ''a VLAN missing from the connection table's local prefixes: ${
        builtins.toJSON (map (v: v.net) (builtins.filter (v: !builtins.elem v.net localPrefixes) VLANS))
      }''
  );
  # A prefix handed to the same flag twice is a fact written twice; the derivation
  # deduplicates, and this is what says so if it stops doing it.
  assert (
    need
    (builtins.length localPrefixes == builtins.length (TABLES.uniq localPrefixes))
    ''a duplicated local prefix: ${builtins.toJSON localPrefixes}''
  );
  # --- the same prefixes, labelled with their VLAN ---------------------------
  # `TABLES.vlanPrefixes` is what `../network-status.nix` hands to `--vlan`, and it
  # is the *same set* as `localPrefixes` with a `vlan<id>=` in front.  That is the
  # whole contract: the lease table groups by the label, the connection table
  # decides direction from the prefix, and one list serving both is what keeps a
  # lease's VLAN and its flow's direction from being two different answers about
  # one address.  Carrying the label into a second, separately-derived list is how
  # they would drift, so the assertion is equality of the prefix halves.
  assert (
    need
    (builtins.sort builtins.lessThan unlabelled == builtins.sort builtins.lessThan localPrefixes)
    ''the --vlan prefixes and the --local-prefix prefixes describe different blocks: ${
        builtins.toJSON {
          onlyInVlanFlags = builtins.filter (p: !builtins.elem p localPrefixes) unlabelled;
          onlyInLocalPrefixes = builtins.filter (p: !builtins.elem p unlabelled) localPrefixes;
        }
      }''
  );
  # And each label is a VLAN device this box actually creates — a lease labelled
  # `vlan999` would be a VLAN column pointing at an interface nobody can find.
  assert (
    need
    (builtins.all (name: builtins.elem name vlanDevices) labelledVlans)
    ''a --vlan label that is not one of the router's VLAN devices: ${builtins.toJSON (builtins.filter (name: !builtins.elem name vlanDevices) labelledVlans)}''
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
    ''a record naming a VLAN that does not exist: ${builtins.toJSON (map (r: "${r.name} ${toString r.vlan}") (builtins.filter (r: r.vlan != null && !builtins.elem r.vlan vlanIds) recordValues))}''
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
    ''a record whose address is not inside its VLAN's block: ${builtins.toJSON (map (r: "${r.name} ${r.ipv4} ${toString r.vlan}") (builtins.filter (r: r.vlan != null && !inside r.ipv4 vlanById.${toString r.vlan}) recordValues))}''
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
  # Every record's IPv4 appears on exactly one dhcp-host line.  This is
  # dnsmasq's own rule, not a preference: dnsmasq exits with "duplicate
  # dhcp-host IP address" when two lines name one address, and it does not care
  # whether the lines differ by MAC, by client id or by hostname (measured
  # against dnsmasq 2.93 on the router, 2026-10-21 — see ./network-tables.nix's
  # `hostLines`).  The first version of this renderer emitted one line per
  # *identifier*, which passed every assertion here and still left the router
  # unable to start dnsmasq the moment those lines went live, so the count of
  # identifiers was never the invariant.
  #
  # A second line carrying only an IPv6 address for the same host is legal and
  # is not counted against this: the duplicate rule is about IPv4 leases.
  assert (
    need
    (
      builtins.all
      (
        r:
          builtins.length (
            builtins.filter
            (l: has "${r.ipv4}," l)
            (TABLES.lines (TABLES.hostLines RECORDS))
          )
          == 1
      )
      recordValues
    )
    ''a record's IPv4 address does not appear on exactly one dhcp-host line, which dnsmasq refuses to start on: ${builtins.toJSON (map (r: "${r.name} ${toString (builtins.length (builtins.filter (l: has "${r.ipv4}," l) (TABLES.lines (TABLES.hostLines RECORDS))))}") recordValues)}''
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
  # vhost name on the network — the whole reason the network has a
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
  # A record's IPv6 must be inside a /64 the network advertises.
  #
  # WHY: this table's `ipv6` field is a promise about the *device*, and since the
  # 2026-10-21 cutover the network assigns no IPv6 to anything (`ra-only` ranges
  # and `option6:23,[::]` in ../dns.nix, and no NPT66 — the two `*d::/64` mappings
  # the retired appliance ran are in `machines/pi-room-sys`'s and `common/`
  # history, not on this box). So an address here that is not even in a prefix the
  # router has can never be answered by anyone: the name serves an AAAA whose
  # packets go out the uplink (`ip -6 route get 2001:8b0:1d14:227d::1001` on this
  # box says `dev ppp0`), and a client that prefers IPv6 hangs instead of falling
  # back — the fault that stopped the Pi Room boards netbooting for five days
  # (`machines/pi-room-sys/README.md`). Ten PI-ROOM records and the `apps1`/
  # `yunohost` ones were emptied on 2026-10-26; this is what refuses them back.
  #
  # WHAT IT CANNOT CATCH, and the reason the field's comment in
  # `common/dns-records.nix` is load-bearing rather than this assertion: an
  # address in a *real* prefix that no host owns is indistinguishable at eval
  # time from one that is configured — `2001:8b0:1d14:225::30` passed this check
  # right up to the moment `ip -6 neigh show` called it `INCOMPLETE`. The eval-time
  # rule is containment, nothing more; ownership is a measurement on the box.
  #
  # `/64` is the only prefix length this network advertises (v6Block64 throws on
  # an address it cannot read as one, which `tryEval` turns into a refusal naming
  # the record rather than a crash of the whole evaluation).
  assert (
    need
    (
      builtins.all
      (r: builtins.all inAdvertisedV6 r.ipv6)
      recordValues
    )
    ''a record answers AAAA outside every /64 the network advertises, i.e. with an address nothing in this network can be reached at: ${
        builtins.toJSON
        (
          map
          (r: "${r.name} -> ${builtins.concatStringsSep ", " r.ipv6}")
          (builtins.filter (r: !builtins.all inAdvertisedV6 r.ipv6) recordValues)
        )
      }; the advertised /64s are ${builtins.concatStringsSep ", " advertisedV6Prefixes}''
  );
  # --- the router's own addresses against the two tables ----------------------
  #
  # Three claims about the number this box puts on every VLAN interface
  # (common/vlans.nix's `ipv4`, always .1 here): it is inside its own block, no
  # dynamic range will hand it to a client, and no reservation claims it.  Each is
  # a two-devices-on-one-address fault and each is silent — the collision shows up
  # as an intermittent ARP problem on some client hours later, never as anything
  # this box logs.  They are here rather than in common/vlans.nix because the
  # relation spans two tables and a rendered file, and this is the file that prints
  # a finding instead of failing an evaluation.  Until 2026-10-22 they were about
  # ./coexist.nix's rewritten table; that mode is gone (this box *is* the router),
  # so what is under test is the address the table itself promises.
  assert (
    need
    (builtins.all (v: inside v.ipv4 v) VLANS)
    ''a VLAN's own address outside its own block: ${builtins.toJSON (map (v: "${v.id} ${v.ipv4} ${v.net}") (builtins.filter (v: !inside v.ipv4 v) VLANS))}''
  );
  assert (
    need
    (
      builtins.all
      (
        v:
          !(
            (lastOctet v.ipv4 >= lastOctet v.dynFrom)
            && (lastOctet v.ipv4 <= lastOctet v.dynTo)
          )
      )
      (builtins.filter (v: v.dynFrom != null) VLANS)
    )
    ''a VLAN whose dynamic range contains its own router address: ${
        builtins.toJSON (map (v: "${v.id} ${v.ipv4} in ${v.dynFrom}-${v.dynTo}") (builtins.filter (
            v:
              v.dynFrom
              != null
              && (lastOctet v.ipv4 >= lastOctet v.dynFrom)
              && (lastOctet v.ipv4 <= lastOctet v.dynTo)
          )
          VLANS))
      }''
  );
  assert (
    need
    (builtins.all (v: !builtins.any (r: r.ipv4 == v.ipv4) recordValues) VLANS)
    ''a device reservation that claims a router's own address: ${
        builtins.toJSON (map (v: v.ipv4) (builtins.filter (v: builtins.any (r: r.ipv4 == v.ipv4) recordValues) VLANS))
      }''
  );
  # --- the front door's names (../../../common/int-dns.nix) -------------------
  #
  # This table used to be a mutable file on the router, edited over ssh by a tool
  # that ran when someone remembered to.  Two of its failure modes are visible
  # here and neither was checked before, which is how `authentik.int` came to be
  # answered from two places at once and how ten deployed vhosts never resolved.
  #
  # The rule that makes the file safe is that **one name has one answer**: dnsmasq
  # reads every `addn-hosts` file it is given and returns the union, so a name that
  # is also a device record answers with the device *and* the front door, and a
  # client that prefers the wrong one reaches the wrong service over a valid
  # certificate.  Every `*.int` name therefore belongs to exactly one of the three
  # lists in that table.
  # A name answered twice by one file is a name with two answers; `names` and
  # `legacy` render the same way, so a label in both is that fault, and so is a
  # label typed twice inside either list (the diff that causes it is one line).
  assert (
    need
    (builtins.all (n: builtins.length (builtins.filter (x: x == n) INT_DOOR_NAMES) == 1) INT_DOOR_NAMES)
    ''common/int-dns.nix answers a name twice (in both `names` and `legacy`, or listed twice): ${
        builtins.toJSON (builtins.filter (n: builtins.length (builtins.filter (x: x == n) INT_DOOR_NAMES) > 1) INT_DOOR_NAMES)
      }''
  );
  assert (
    need
    (
      builtins.all (n: !builtins.hasAttr n RECORDS) INT_DOOR_NAMES
      && builtins.all (n: !builtins.elem n INT.fromHosts) INT_DOOR_NAMES
    )
    ''a name in common/int-dns.nix answered by another table too: ${
        builtins.toJSON (builtins.filter (n: builtins.hasAttr n RECORDS || builtins.elem n INT.fromHosts) INT_DOOR_NAMES)
      }''
  );
  # A `fromHosts` name gets its address from common/hosts.nix, so it has to be in
  # that table; and it must not be a reservation too, which is the same
  # name-answered-twice fault wearing a different hat.
  assert (
    need
    (
      builtins.all (n: builtins.hasAttr n HOSTS) INT.fromHosts
      && builtins.all (n: !builtins.hasAttr n RECORDS) INT.fromHosts
    )
    ''a common/int-dns.nix `fromHosts` name that is not exactly one non-reservation host: ${
        builtins.toJSON (builtins.filter (n: !builtins.hasAttr n HOSTS || builtins.hasAttr n RECORDS) INT.fromHosts)
      }''
  );
  # An `exempt` name is a vhost the front door serves that must not resolve to it,
  # and the only honest reason is that a device answers the name.  So it has to be
  # a record: an exemption that answers nothing is a name the drift check on
  # services1 cannot tell from a forgotten one, which is the whole reason the list
  # exists.
  assert (
    need
    (builtins.all (n: builtins.hasAttr n RECORDS) (builtins.attrNames INT.exempt))
    ''a common/int-dns.nix `exempt` name that is not a device record in common/dns-records.nix: ${
        builtins.toJSON (builtins.filter (n: !builtins.hasAttr n RECORDS) (builtins.attrNames INT.exempt))
      }''
  );
  assert (
    need
    (builtins.all (n: !builtins.elem n (INT.names ++ INT.fromHosts ++ builtins.attrNames INT.legacy)) (builtins.attrNames INT.exempt))
    ''a name both exempt and answered by common/int-dns.nix: ${
        builtins.toJSON (builtins.filter (n: builtins.elem n (INT.names ++ INT.fromHosts ++ builtins.attrNames INT.legacy)) (builtins.attrNames INT.exempt))
      }''
  );
  # The door has to resolve to at least one address, and to the *same* number of
  # them whatever it is: `frontDoor` names a box, and the two boxes in the network
  # keep their addresses in two different tables (a device in common/dns-records.nix,
  # the router in common/vlans.nix's VLAN whose `ipv4` is `hosts.router`), which is
  # the one thing the dnsmasq rendering cannot tell apart — a door that resolves to
  # nothing renders as an empty file rather than as an error, and an empty file in an
  # `addn-hosts` list is a zone that answers nothing.
  assert (
    need
    (builtins.length DOOR_ADDRS > 1)
    ''common/int-dns.nix's frontDoor "${INT.frontDoor}" resolves to no addresses: it must name a record in common/dns-records.nix or `gw`, and `gw` needs a VLAN in common/vlans.nix whose ipv4 is ${HOSTS.router}''
  );
  # And the door answers with both address families.  This is the PARTIAL fault read
  # off the *door* rather than off a name: the assertion below counts the lines each
  # name renders, so it proves every name gets every address — it cannot notice that
  # the address list itself lost its IPv6 half, which is the version of the bug where
  # every name is consistent, complete and v4-only, and a dual-stack client hangs on
  # IPv6 before it falls back (docs/router-and-dns.md).
  assert (
    need
    (builtins.any (a: has ":" a) DOOR_ADDRS)
    ''common/int-dns.nix's frontDoor "${INT.frontDoor}" answers with no IPv6 address: ${builtins.toJSON DOOR_ADDRS}''
  );
  # **Every front-door name answers with every one of its addresses.** This is the
  # PARTIAL fault in docs/router-and-dns.md — a name with its A and not its AAAA is
  # a dual-stack client that hangs on IPv6 before it falls back — asserted against
  # the rendered file rather than the table, because the file is what dnsmasq reads
  # and one line per address is what its hosts syntax needs (a second address on a
  # line is a second *name*).  `intHosts` groups a name's lines together, so this
  # is a count of the lines ending in the name.
  assert (
    need
    (
      builtins.all
      (n: builtins.length (builtins.filter (l: has "\t${n}.${CONFIG.LOCAL_DOMAIN}" l) (TABLES.lines INT_HOSTS)) == builtins.length DOOR_ADDRS)
      INT_DOOR_NAMES
    )
    ''a front-door name not answered with every address of ${INT.frontDoor} (${
        toString (builtins.length DOOR_ADDRS)
      } expected): ${
        builtins.toJSON (builtins.filter
          (n: builtins.length (builtins.filter (l: has "\t${n}.${CONFIG.LOCAL_DOMAIN}" l) (TABLES.lines INT_HOSTS)) != builtins.length DOOR_ADDRS)
          INT_DOOR_NAMES)
      }''
  );
  # Every line of the rendered file is `<address><TAB><name>.<zone>`.  An
  # unterminated name is a file dnsmasq reads and answers nothing for — the bug
  # this file has caught twice before, on a file that is a store path and so can be
  # diffed, which is more than can be said for the one this replaced.
  assert (
    need
    (
      builtins.all
      (l:
        has ".${CONFIG.LOCAL_DOMAIN}" l
        && builtins.any (a: hasPrefix "${a}\t" l) (DOOR_ADDRS ++ builtins.attrValues HOSTS))
      (builtins.filter (l: l != "") (TABLES.lines INT_HOSTS))
    )
    ''a generated int-vhost line that is not `<address><TAB><name>.${CONFIG.LOCAL_DOMAIN}`''
  );
  # And the line count is the whole table: once per name per address, plus once per
  # own-address name.  Nothing extra, nothing missing.
  assert (
    need
    (
      builtins.length (builtins.filter (l: l != "") (TABLES.lines INT_HOSTS))
      == builtins.length INT_DOOR_NAMES * builtins.length DOOR_ADDRS + builtins.length INT.fromHosts
    )
    ''the generated int-vhosts file is not one line per (name, address): ${
        toString (builtins.length (builtins.filter (l: l != "") (TABLES.lines INT_HOSTS)))
      } lines for ${toString (builtins.length INT_DOOR_NAMES)} names x ${
        toString (builtins.length DOOR_ADDRS)
      } addresses + ${toString (builtins.length INT.fromHosts)} own-address names''
  ); "ok\n"
