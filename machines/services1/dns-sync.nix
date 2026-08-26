# DNS-sync tooling: keeps the router's dnsmasq and DigitalOcean DNS in step
# with the *.int.leighhack.org vhosts this box's nginx serves.
#
# Rust source: ../../dns-sync (zero external crates, see status-dashboard/).
# Docs: ROUTER.md + DO.md at the repo root.
#
# The source of truth is `services.nginx.virtualHosts` (all vhost names plus
# their serverAliases that end in .int.leighhack.org — those are the DNS
# records that must exist).  The list is rendered to
# /etc/dns-sync/expected-int-names at build time, and the tool compares it
# against:
#
#   * the router: /var/etc/dnsmasq-hosts on 10.3.1.1 (OPNsense, ssh root),
#   * DigitalOcean: the leighhack.org zone (the DNS DoH users see; token in
#     CONFIG.ENV_FILE).
#
# Usage on services1 (via the justfile recipes, or directly):
#
#   sudo dns-sync check            # report whether every vhost is in DNS
#   sudo dns-sync sync             # add missing records (router + DO)
#
# The tool is strictly additive: it only adds missing names (router
# host-override aliases / DO CNAMEs) and never deletes or rewrites existing
# records — many *.int.leighhack.org names legitimately point at other
# hosts, so the router/DO zones are never cleaned up.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  dnsSync = pkgs.rustPlatform.buildRustPackage {
    pname = "dns-sync";
    version = "0.1.0";
    src = ../../dns-sync;
    cargoLock.lockFile = ../../dns-sync/Cargo.lock;
    doCheck = false;
  };

  # Every *.int.leighhack.org name this nginx serves (vhost names + aliases).
  intNames = lib.sort (a: b: a < b) (
    lib.unique (
      lib.filter (n: lib.hasSuffix ".int.leighhack.org" n) (
        lib.flatten (
          lib.mapAttrsToList (name: vh:
            [ name ] ++ (lib.toList (vh.serverAliases or [ ]))
          ) config.services.nginx.virtualHosts
        )
      )
    )
  );
in
{
  environment.etc."dns-sync/expected-int-names".text =
    lib.concatStringsSep "\n" intNames + "\n";

  environment.systemPackages = [ dnsSync ];
}
