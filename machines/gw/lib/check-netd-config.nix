# CHECK — gw's rendered netd configuration against the tables it came from.
#
# Run by `just check-eval` and by `nix flake check` (as `netd-config` in
# `flake.nix`); returns the string `"ok\n"` or throws with the finding, the same
# seam every other checker here uses (`machines/gw/lib/check-network-tables.nix`).
#
# This is NOT a second copy of the daemon's schema rules. Those live in
# `crates/netd/src/config/validate.rs` and run at build time over the document
# itself (`netd check`, from `common/netd/module.nix`), because only the daemon
# can decide what a valid document is — and a rule written twice is a rule that
# will disagree with itself. What can *only* be checked here is the projection:
# whether the renderer dropped something on the way from a table to the document.
# A VLAN with a range and no pool, a record with no name, a front-door name that
# never arrived — those are renderer bugs, and they are silent, because the daemon
# would happily serve a complete-looking document that is missing one row.
let
  CONFIG = import ../../../common/config.nix;
  VLANS = import ../../../common/vlans.nix;
  RECORDS = import ../../../common/dns-records.nix;
  HOSTS = import ../../../common/hosts.nix;
  INT = import ../../../common/int-dns.nix;
  cfg = import ../netd-config.nix;

  withPool = builtins.filter (v: v.dynFrom != null) VLANS;
  withRa = builtins.filter (v: v.ra != []) VLANS;

  # An address, as a number, so a range can be compared without a CIDR library.
  quads = s: map builtins.fromJSON (builtins.filter builtins.isString (builtins.split "\\." s));
  addr = ip: builtins.foldl' (acc: q: acc * 256 + q) 0 (quads ip);
  inside = a: p: addr a >= addr p.from && addr a <= addr p.to;

  renderedNames = map (r: r.name) cfg.dns.records;
  nameOf = label: "${label}.${CONFIG.LOCAL_DOMAIN}";
  missingDoorNames = builtins.filter (label: !(builtins.elem (nameOf label) renderedNames)) INT.names;
  orphanIgnored = builtins.filter (a: !(builtins.any (p: inside a p) cfg.dhcp.pools)) cfg.dhcp.ignored_addresses;

  findings = builtins.filter (x: x != null) [
    (
      if builtins.length cfg.dhcp.pools == builtins.length withPool
      then null
      else "${toString (builtins.length cfg.dhcp.pools)} DHCP pool(s) for ${toString (builtins.length withPool)} VLAN(s) with a dynamic range — a VLAN whose range has no pool hands out nothing"
    )
    (
      if builtins.length cfg.dhcp.interfaces == builtins.length VLANS
      then null
      else "${toString (builtins.length cfg.dhcp.interfaces)} DHCP interface(s) for ${toString (builtins.length VLANS)} VLAN(s) — an interface with no pool must still be answered (or refused) deliberately, and common/vlans.nix is the list"
    )
    (
      if builtins.length cfg.dhcp.reservations == builtins.length (builtins.attrNames RECORDS)
      then null
      else "${toString (builtins.length cfg.dhcp.reservations)} reservation(s) for ${toString (builtins.length (builtins.attrNames RECORDS))} record(s) in common/dns-records.nix — a dropped reservation is a device that gets a pooled address instead of its own"
    )
    (
      if builtins.all (r: r.macs != [] || r.client_ids != []) cfg.dhcp.reservations
      then null
      else "a reservation has neither a MAC nor a client-id, so it can never match a request (common/dns-records.nix's header explains why such a record is refused rather than rendered)"
    )
    (
      if builtins.length cfg.ra.interfaces == builtins.length withRa
      then null
      else "${toString (builtins.length cfg.ra.interfaces)} router-advertising interface(s) for ${toString (builtins.length withRa)} VLAN(s) with an `ra` range — an unadvertised range is a VLAN with no IPv6 default route"
    )
    (
      if builtins.all (i: i.prefixes != []) cfg.ra.interfaces
      then null
      else "a router advertisement has no prefixes, so it would announce a default route to nowhere"
    )
    (
      if missingDoorNames == []
      then null
      else "no DNS record rendered for ${builtins.concatStringsSep ", " missingDoorNames} — a front-door name that does not answer is a vhost nobody can reach by name"
    )
    (
      if orphanIgnored == []
      then null
      else "${builtins.concatStringsSep ", " (map toString orphanIgnored)} is in ignored_addresses but inside no dynamic range, so nothing would hand it out anyway (machines/gw/netd-config.nix)"
    )
  ];
in
  if findings == []
  then "ok\n"
  else throw "netd config (machines/gw/netd-config.nix):\n  - ${builtins.concatStringsSep "\n  - " findings}\n"
