# The network's two tables (common/vlans.nix, common/dns-records.nix) turned into
# the syntax the router's own tools read.  One file, three consumers:
# ../dns.nix (dnsmasq's conf), ../firewall.nix (nftables' rules) and
# ./check-network-tables.nix (assertions over the output).  The reason it is a
# file rather than a `let` in each consumer is the reason the three consumers
# exist at all: a VLAN tag spelled one way in dnsmasq's conf and another way in
# nftables' rules is a device that gets an address and no routing, and the only
# thing that can notice is the two halves being generated from one place.
#
# Read common/vlans.nix and common/dns-records.nix first: this file does not
# decide anything about the network, it translates.  Every decision in here is a
# decision about dnsmasq's or nftables' syntax, and the comment on each one says
# which tool it is about.
let
  # The records' names, as a field on the record: dnsmasq's dhcp-host line ends
  # with the name, the hosts file ends with the name, and the name is the table's
  # key in both cases — so the key is turned into a field once, here, rather than
  # twice in two functions that would then have to agree about how.
  named = builtins.mapAttrs (name: r: r // {inherit name;});

  # Substring test, defined once for the whole file: the ACL's IPv4 filter needs
  # it and so does the check module, and the two must agree about what "contains"
  # means or the filter and the assertions are about different sets.
  has = sub: s: builtins.replaceStrings [sub] [""] s != s;
in rec {
  # The VLAN device name, spelled the Linux way.  common/vlans.nix's header is
  # about the FreeBSD spelling the old box used (`vlan0.<id>`) and about the
  # fact that dnsmasq takes a VLAN tag from the *device name*, which is why the
  # name is generated here rather than written per VLAN in three files.
  # Takes a VLAN record or a bare VLAN id, because the callers have both: a
  # dhcp-host line knows only the id its record carries, a dhcp-range line is
  # generated from the record itself.  One function either way, because two
  # spellings of the same device name is exactly the drift this file exists to
  # prevent (and machines/gw/networking.nix used to have its own copy).
  vlanDevice = v: "vlan${toString (
    if builtins.isInt v
    then v
    else v.id
  )}";

  # dnsmasq's two tag prefixes, and the difference is the whole point of having
  # both: `tag:` *matches* a tag (used where the entry only applies to requests
  # already carrying it — dhcp-range, dhcp-boot) and `set:` *adds* one (used
  # where the entry is what gives the lease its tag — dhcp-host, dhcp-match).
  # The old box's generated conf used exactly these two forms, which is why the
  # flake's does too.
  matchTag = v: "tag:${vlanDevice v}";
  addTag = v: "set:${vlanDevice v}";

  # The lease lifetime, in seconds: a day.  A device that is rebooted keeps its
  # address for as long as the router's record of it does, and a device that is
  # physically moved is picked up the next day rather than at the router's next
  # boot.  The old box's own value.
  leaseTtl = 86400;

  # One dhcp-host line per *record*, carrying every identifier the device
  # answers with as comma-separated selectors — one line, not one line per
  # identifier.  A device with two MACs (a laptop with two NICs, a Pi-room board
  # with a replaced one) or with a MAC and a client id is still one reservation,
  # and dnsmasq 2.93 refuses to read two `dhcp-host` lines that name the same
  # address at all:
  #
  #   dnsmasq: duplicate dhcp-host IP address 10.3.1.16 at line 30 of
  #   /etc/dnsmasq.d/network.conf ... FAILED to start up
  #
  # That is not a warning.  dnsmasq exits, the whole network loses DNS and DHCP,
  # and the unit's never-give-up policy turns it into an endless restart loop —
  # measured on the box on 2026-10-21, after the cutover made these lines live
  # for the first time (./coexist.nix comments them out).  Multiple selectors on
  # one line *are* legal (`dhcp-host=<mac>,<mac>,id:<client-id>,<addr>,<name>`),
  # which is why the shape below is the fix rather than dropping a selector.
  # Also measured: a second line for the same host carrying only an *IPv6*
  # address is fine, because the duplicate rule is about IPv4 leases.
  #
  # A record with no identifier at all produces no line, which is why
  # ./check-network-tables.nix refuses those rather than letting a dead record
  # look like a working reservation.
  #
  # The `set:vlan<id>` field is the lease's tag, and it is *optional*: a record
  # whose `vlan` is null (common/dns-records.nix: a device the old router answered
  # with an address and nothing else, on no VLAN of ours) gets the address with no
  # tag, which is what makes it land in no VLAN's firewall set.  Writing
  # `set:vlan` for it would produce the device name `vlan`, which dnsmasq would
  # happily tag a lease with and nftables would never match.
  hostLines = records:
    builtins.concatStringsSep "\n"
    (
      map
      (
        r: let
          ident = r.macs ++ (map (c: "id:${c}") r.clientIds);
          addr = [r.ipv4] ++ (map (i: "[${i}]") r.ipv6);
          tag =
            if r.vlan == null
            then ""
            else ",${addTag r.vlan}";
        in "dhcp-host=${builtins.concatStringsSep "," ident}${tag},${builtins.concatStringsSep "," addr},${r.name}"
      )
      (builtins.attrValues (named records))
    );

  # The device names as a hosts file, so a bare `services1` and a
  # `services1.int.leighhack.org` query answer from the same record and cannot
  # disagree with the reservation.  Generated, which is the point: the names are
  # the table's keys.
  #
  # One line per address, with both names on it — `<addr> <name>.<zone> <name>` —
  # which is the shape the old box's generated `/var/etc/dnsmasq-hosts` has, and two
  # parts of it are not interchangeable:
  #
  #   * The FQDN has to be spelled out. `domain=` and `local=` make this box
  #     authoritative for the zone; they do not *complete* a hosts entry. A file of
  #     bare names answers `services1` and answers `services1.int.leighhack.org`
  #     with nothing at all — measured on the box against the old router on
  #     2026-10-17, which is how this was found: the old box answered the FQDN, this
  #     one answered the short name only, and every vhost, monitoring endpoint and
  #     `just dns-sync` consumer on the network asks for the FQDN.
  #   * Only the first address on a line is an address; everything after it is a
  #     name. So a record's IPv4 and its IPv6 cannot share a line — that would
  #     register the v6 address as an alias *name* for the host, which is a silently
  #     wrong answer rather than an error. The old box's file has one line per
  #     address for the same reason.
  recordsHosts = zone: records:
    builtins.concatStringsSep "\n"
    (
      builtins.concatLists
      (
        map
        (r: map (addr: "${addr} ${r.name}.${zone} ${r.name}") ([r.ipv4] ++ r.ipv6))
        (builtins.attrValues (named records))
      )
    );

  # The VLANs' dynamic ranges and their router-advertisement ranges, one block
  # per VLAN that has either.  A VLAN with neither (the phones') gets no line at
  # all, which is what makes a device arriving on it get no lease.
  rangeLines = vlans:
    builtins.concatStringsSep "\n"
    (
      map
      (
        v: builtins.concatStringsSep "\n" (builtins.filter (x: x != "") (rangeLinesFor v))
      )
      (builtins.filter (v: v.dynFrom != null || v.ra != []) vlans)
    );

  # One VLAN's lines: its dynamic range (if it has one), its RA ranges (if it
  # has any), and the `ra-param` line that makes dnsmasq re-advertise every
  # 60s for up to 1200s — which is what a device that has just been moved needs
  # the router to do without waiting for the lease's day to expire.  Flat list
  # comprehension rather than nested parenthesised groups: the whole point of
  # this file being a file is that it gets read, and a block of nested groups
  # is not readable.
  rangeLinesFor = v:
    [
      (
        if v.dynFrom != null
        then "dhcp-range=${matchTag v},${v.dynFrom},${v.dynTo},255.255.255.0,${toString leaseTtl}"
        else ""
      )
    ]
    ++ map
    (r: "dhcp-range=${matchTag v},${r.from},${r.to},ra-only,64,${toString leaseTtl}")
    v.ra
    ++ [
      (
        if v.ra != []
        then "ra-param=${vlanDevice v},60,1200"
        else ""
      )
    ];

  # The netboot variants the PI-ROOM devices' boot menus offer: a tag, the DHCP
  # option the client's request has to carry for the variant to be chosen
  # (option 93 is the client's "how do I want to be chainloaded" value: 0 is the
  # legacy BIOS path, 7/8/9 the EFI paths; option 77 is iPXEScript), and the
  # file/URL dnsmasq serves.  Kept as one table because the `dhcp-match` half and
  # the `dhcp-boot` half name each other by tag — writing them as two lists is
  # how they would drift.
  #
  # The last two fields of dhcp-boot are the TFTP server the *client* fetches
  # from — aibox's own tftpd (machines/aibox/netboot.nix runs it at
  # 10.3.1.32:69).  dnsmasq reads the file below from the router's own
  # filesystem to build the chainload binary, so the router needs the file and
  # the client needs the server, and both are stated here.
  netbootLines = tftpServer:
    builtins.concatStringsSep "\n"
    (
      map
      (
        b:
          "dhcp-match=set:${b.tag},${b.match}\n"
          + "dhcp-boot=${matchTag b.vlan},tag:${b.tag},${b.boot},${tftpServer},${tftpServer}"
      )
      netbootVariants
    );

  netbootVariants = [
    {
      vlan = 227;
      tag = "netboot-legacy";
      match = "93,0";
      boot = "/etc/tftp/undionly.kpxe";
    }
    {
      vlan = 227;
      tag = "netboot-efi";
      match = "93,7";
      boot = "/etc/tftp/ipxe.efi";
    }
    {
      vlan = 227;
      tag = "netboot-efi";
      match = "93,8";
      boot = "/etc/tftp/ipxe.efi";
    }
    {
      vlan = 227;
      tag = "netboot-efi";
      match = "93,9";
      boot = "/etc/tftp/ipxe.efi";
    }
    {
      vlan = 227;
      tag = "netboot-autoexec";
      match = "77,iPXE";
      boot = "http://aibox.int.leighhack.org/boot/netboot.ipxe";
    }
  ];

  # What the DHCP offer says about DNS: the VLAN's own router address, one line
  # per VLAN, tagged so the offer a client on vlan226 receives names 10.3.2.1 and
  # not the wired LAN's gateway.  Without the tag every VLAN's clients are sent at
  # a gateway on someone else's segment and the lease is unusable — which is the
  # shape the reference router's config has (its own `dhcp-option` lines are one
  # per interface: `tag:…,option:dns-server,<that interface's address>`).
  #
  # A list rather than a conf-file block, because dnsmasq's `settings` is a
  # freeform attrs and `dhcp-option` there is a real list option: one element per
  # `dhcp-option=` line, which is also what the rendered unit and
  # `/etc/dnsmasq.conf` show an operator.
  dnsOptionLines = vlans: map (v: "${matchTag v},option:dns-server,${v.ipv4}") vlans;

  # The blocks, as nftables set literals (nftables' `{ a, b }` set syntax, which
  # is why the helper that dnsmasq does not need lives here too).
  blockSet = vs: setOf (map (v: v.net) vs);

  # The same literal from bare block strings rather than VLAN records — the ACL
  # half of ./firewall.nix has strings, not records, and the two must render the
  # same way or "the ACL allows it" and "the rule allows it" are different claims.
  setOf = vs: "{ " + builtins.concatStringsSep ", " vs + " }";

  # An address-notation block's four dotted quads and its mask, as strings.
  # The regex is written "\\." because a Nix string escape eats one backslash:
  # "\." in a Nix string is just ".", which as a regex matches *any* character
  # and splits the address into single characters — which is what makes the
  # prefix come out as "..." rather than "10.3.1.".
  quadsOf = s: builtins.filter builtins.isString (builtins.split "\\." (builtins.head (builtins.split "/" s)));
  maskOf = s: builtins.elemAt (builtins.split "/" s) 2;

  # The block *prefix* of an address-notation block: "10.3.1.0/24" -> "10.3.1.",
  # "10.3.0.0/16" -> "10.3.".  One helper, used for both "is this address in that
  # block" and "is that block inside this one", which is what makes the router's
  # VLAN blocks and the network's ACL comparable by the same relation the tools
  # use — and why it is string comparison rather than bitwise math: dnsmasq and
  # nftables both take the notation as written, so the notation is the fact.
  # The prefix is cut at the mask's own boundary (mask/8 dotted quads), which is
  # what makes a /16 ACL block contain a /24 VLAN block — the naive "drop the
  # last quad" version gets "10.3.0." for 10.3.0.0/16 and then says VLAN 225's
  # 10.3.1.0/24 is not in it, i.e. the firewall drops the wired LAN.  IPv6
  # notation has no dotted quads, which is why every caller filters to the ACL's
  # IPv4 half first (ipv4AclBlocks) rather than this helper deciding to skip.
  blockPrefix = s: let
    q = quadsOf s;
    n = builtins.div (builtins.fromJSON (maskOf s)) 8;
  in
    builtins.concatStringsSep "." (builtins.genList (i: builtins.elemAt q i) n) + ".";

  # The network's own ACL as bare blocks: common/config.nix's LOCAL_NETWORK is
  # written as nginx `allow` lines ("allow 10.3.1.0/24;") because it is read by
  # the network's nginx servers; the prefix and the `;` are stripped here rather
  # than the block being written a second time, which is the whole point: one
  # table, two renderers.  Only the `allow` lines are taken — `deny all;` is
  # nginx's own trailing rule and has no meaning in an nft set.
  aclBlocks = acl:
    map
    (line: builtins.head (builtins.split ";" (builtins.substring 6 (builtins.stringLength line) line)))
    (builtins.filter (l: builtins.substring 0 6 l == "allow ") (lines acl));

  # The ACL's IPv4 blocks.  The IPv4 filter exists because the ACL is one table
  # for both address spaces (it carries the network's IPv6 ranges too, and the
  # tailnet's) while an nftables `ip` table can only name IPv4 blocks: an IPv6
  # entry in the set below is a rule nft will not parse, which nixpkgs' nftables
  # module would catch at build time but not at `nix flake check`.
  ipv4AclBlocks = acl: builtins.filter (b: !has ":" b) (aclBlocks acl);

  # The ACL's IPv6 half — the other side of the same split, so ./firewall.nix's
  # ip6 table is built from the same table by the same relation rather than by a
  # second filter someone has to keep in step.  ./lib/check-network-tables.nix
  # asserts the two halves partition the ACL, which is what makes "the ACL allows
  # it" mean the same thing in both address spaces.
  ipv6AclBlocks = acl: builtins.filter (b: has ":" b) (aclBlocks acl);

  # One string's lines, as a list (builtins.split returns the separators as
  # lists, so the strings are filtered out first — the same reason
  # common/check-printer-states.nix's `tokens` helper filters).
  lines = s: builtins.filter builtins.isString (builtins.split "\n" s);

  # --- the helpers ./../coexist.nix and its consumers need ------------------
  #
  # Co-existence is one fact about this box (it owns every VLAN's gateway
  # address except the number, because the router it is replacing is still
  # live), and these four helpers are what turn that fact into the table read
  # differently rather than into a second table.  They are here rather than in
  # ./../coexist.nix because ./../dns.nix, ./../networking.nix and
  # ./../firewall.nix all read the rewritten table and ./check-network-tables.nix
  # has to be able to ask the same questions of it — three renderers and a check
  # agreeing on one relation is the whole reason this file exists.

  # Is this address inside that address-notation block?  The same prefix
  # relation as "is this VLAN's block inside the ACL", which is the point: the
  # check and the firewall must not have two ideas about containment.  It is a
  # string test because the notation is what dnsmasq and nftables are handed.
  inBlock = addr: net: has (blockPrefix net) addr;

  # Replace an IPv4 address's host part, leaving its network part alone.  A
  # dotted quad is the only shape accepted (the assert is on the *split*, so a
  # block in CIDR notation or an IPv6 address fails here rather than coming out
  # as a one-and-a-half octet address in a rendered conf).
  hostAddr = host: addr: let
    q = quadsOf addr;
  in
    assert (builtins.match "[0-9]+" host != null);
    assert (builtins.length q == 4); "${builtins.elemAt q 0}.${builtins.elemAt q 1}.${builtins.elemAt q 2}.${host}";

  # A VLAN list with each VLAN's own address rewritten and nothing else: the ids,
  # blocks, dynamic ranges and comments are facts about the network and stay,
  # `ipv4` is a fact about this box and moves.  Every renderer downstream reads
  # the result, which is what makes the mode one substitution rather than a
  # branch in four files.
  hostAddrs = host: vlans: map (v: v // {ipv4 = hostAddr host v.ipv4;}) vlans;

  # Comment out a generated block instead of deleting it.  ./../dns.nix emits the
  # DHCP and RA blocks commented while another router is live: a block that is
  # *absent* is indistinguishable from a bug in the generator, and whoever runs
  # the cutover has to be able to read what they are about to enable.  dnsmasq's
  # comment character is '#', and blank lines stay blank so the blocks are still
  # blocks.
  commentBlock = text:
    builtins.concatStringsSep "\n"
    (map
      (l:
        (
          if l == ""
          then ""
          else "# "
        )
        + l)
      (builtins.filter builtins.isString (builtins.split "\n" text)));
}
