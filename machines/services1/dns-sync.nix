# DNS-sync tooling: keeps the router's dnsmasq and DigitalOcean DNS in step
# with the *.int.leighhack.org vhosts this box's nginx serves.
#
# Rust source: ../../dns-sync (zero external crates, see status-dashboard/).
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
#                                  # (and any stale rename leftovers)
#   sudo dns-sync sync             # add missing records (router + DO)
#   sudo dns-sync prune            # remove stale rename leftovers the tool
#                                  # manages (never records that predate it)
#
# `sync` is strictly additive: it only adds missing names (router
# host-override aliases / DO CNAMEs) and never deletes or rewrites existing
# records — many *.int.leighhack.org names legitimately point at other
# hosts, so other records are never touched. `prune` is the explicit
# exception and is scoped tightly: it removes only (a) services1 override
# aliases and (b) DO CNAMEs -> nginx.int, and only when the name was in the
# previous expected list (/var/lib/dns-sync/last-expected) but is no longer.
{
  config,
  lib,
  pkgs,
  CRANE,
  ...
}: let
  dnsSync = CRANE.cached {
    pname = "dns-sync";
    version = "0.1.0";
    src = ../../dns-sync;
    cargoLock = CRANE.lockFile ../../dns-sync/Cargo.lock;
    # `common-json` path dependency (see common-rs/ and common/crane.nix).
    sharedCrates = ["json"];
  };

  # Every *.int.leighhack.org name this nginx serves (vhost names + aliases).
  intNames = lib.sort (a: b: a < b) (
    lib.unique (
      lib.filter (n: lib.hasSuffix ".int.leighhack.org" n) (
        lib.flatten (
          lib.mapAttrsToList (
            name: vh:
              [name] ++ (lib.toList (vh.serverAliases or []))
          )
          config.services.nginx.virtualHosts
        )
      )
    )
  );
in {
  environment.etc."dns-sync/expected-int-names".text =
    lib.concatStringsSep "\n" intNames + "\n";

  # Pinned router host keys — dns-sync sshes to the router as root with
  # StrictHostKeyChecking=yes (see dns-sync/src/main.rs), so a router reinstall
  # must be an explicit act here rather than something accepted silently.
  # Regenerate with `just router-known-hosts` and paste the result.
  # network-status points at this same file rather than keeping its own list.
  #
  # All three key types the router offers, so any KEX picks a pinned key.
  environment.etc."dns-sync/known_hosts".text = ''
    10.3.1.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJsSlRavpS3tNlWtYQogSqAIJRBPWT5MDwikeUPuS1aO
    10.3.1.1 ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEvD2vzBFQPrAyudJsakBDVjQLszqFZgbwzXKPJcECFzG/xyYtXNAo1vSJ6YLLIxGrZj6PSrfFZAf4v/lKRBsvY=
    10.3.1.1 ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCmMFcJ/Z0AlGOPnxRRAAl6MNVx3qtVGtk8drdbyTEhF1u/UgHWBxwoQPU+IDig67T/c5+NAt72LG3nf3HDXjH/JcmHuc+g7XjMQgnaooKKZ+qFl/rW0o5UO1RN/T+Kgf0OzlkPHW/n8MpbvlnK0EBwTZorOx2JdGW0pbJBxEXzkimgV4B63kv1Jt44KPLZfgHs5R0XVuE4cz8RVOOfr+PlmDUMfmRZj01wddi+lzHZKBQxKuZ/BI2b6WzpJHC3criCDLA7aIcVEkRDIkFVHT8nFnBfrBw3UkkaJAtGHiG+vu2o2Y2+RAsDdQJ6uDpmts223qHzpC8Mra/lGlss+RBi2cX5vFmh/SZhDqG4Kq53bjy+1fEDdLl6/srPekPPvhCTJamuyDrtQEbTZEpPL5mRhXKpCz2Qq4iR6mkxEWtN4wKcWSExTLkC8TTynuvtsTwNm3YJrbKoynNROq07WN/e00totHeU8r6K7IgVjWn9U8G+wzH20cvIkOopyLOUrrU=
  '';

  environment.systemPackages = [dnsSync];
}
