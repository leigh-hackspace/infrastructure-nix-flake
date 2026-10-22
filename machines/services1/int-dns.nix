# The front door's half of the network's DNS: the *guard* on
# `common/int-dns.nix`, which is the table the router answers `*.int` from.
#
# Two things have to agree and neither can see the other at build time: this box's
# nginx, which serves the `*.int.leighhack.org` vhosts, and
# `common/int-dns.nix`, which is rendered into `/etc/dnsmasq-int-vhosts.hosts` on
# gw (`machines/gw/dns.nix`).  This module derives the first from
# `services.nginx.virtualHosts` — vhost names plus their serverAliases, the same
# derivation `common/nginx/int-vhost-names.nix` gives the gatus endpoint guard, so
# the two cannot disagree about what "an internal app on this box" means — and
# refuses to evaluate unless it equals the second.
#
# What that replaces is worth a line, because the tool is gone: until 2026-10-22
# `crates/dns-sync` ssh'd to the router from this box and appended lines to a
# mutable `/var/lib/dnsmasq/names.hosts`, when someone remembered to run
# `just dns-sync-sync`.  Nothing was broken the day it was removed — all 34 `*.int`
# vhosts the deployed nginx serves had their records — and that is exactly the
# shape of the debt: a vhost deployed without the command is live, linked,
# TLS-served and unresolvable, and the only thing that would notice is the same
# manual command, run by hand, comparing two files.  Now the record is a store path
# on the router and the drift is an eval error here.
#
# The check runs wherever this box is evaluated, which is every `just switch`,
# `nixos-rebuild dry-run` and `just check-build` — not `just check`, which
# deliberately evaluates no machine config at all (AGENTS.md: never cold-evaluate
# this flake to ask a question; the deploy answers it cheaper).
#
# `environment.etc."int-dns/served-int-names"` is the same list rendered, for the
# moment it isn't agreeing: diff it against the router's
# `/etc/dnsmasq-int-vhosts.hosts` and the difference is the name and its address.
{
  config,
  lib,
  ...
}: let
  CONFIG = import ../../common/config.nix;
  INT = import ../../common/int-dns.nix;
  intVhostNames = import ../../common/nginx/int-vhost-names.nix {inherit lib;};

  zone = CONFIG.LOCAL_DOMAIN;

  # Every `*.int` name this box's nginx is configured to serve, FQDNs, sorted.
  served = intVhostNames config.services.nginx.virtualHosts;

  # What the network is told this box serves: the front-door list, plus the names
  # that are answered somewhere else *on purpose*.  Both halves are needed for the
  # comparison — without `exempt` the honest answer to "why isn't authentik.int in
  # the table?" would be "because it would answer twice", which is not a thing a
  # failing assertion should make someone deduce.
  declared =
    lib.sort (a: b: a < b)
    (
      (map (n: "${n}.${zone}") INT.names)
      ++ (map (n: "${n}.${zone}") (builtins.attrNames INT.exempt))
    );

  missing = builtins.filter (n: !builtins.elem n declared) served;
  extra = builtins.filter (n: !builtins.elem n served) declared;
in {
  assertions = [
    {
      assertion = served == declared;
      message = ''
        common/int-dns.nix and this box's nginx do not serve the same `*.int` names.

          served by nginx, missing from the table: ${builtins.toJSON missing}
          in the table, served by nothing here:    ${builtins.toJSON extra}

        Fix the table, then deploy *both* boxes: `common/int-dns.nix` is rendered
        into /etc/dnsmasq-int-vhosts.hosts on gw, so a name this box starts serving
        resolves only once the router has been switched too (`just switch-gw`), and
        a name it stops serving keeps resolving until then.  A `*.int` vhost that
        is served but deliberately not answered at the front door goes in `exempt`,
        with the reason.
      '';
    }
  ];

  environment.etc."int-dns/served-int-names".text = lib.concatStringsSep "\n" served + "\n";
}
