# DNS-sync tooling: keeps the router's dnsmasq and DigitalOcean DNS in step
# with the *.int.leighhack.org vhosts this box's nginx serves.
#
# Rust source: ../../crates/dns-sync (zero external crates, see
# crates/status-dashboard/).
#
# The source of truth is `services.nginx.virtualHosts` (all vhost names plus
# their serverAliases that end in .int.leighhack.org — those are the DNS
# records that must exist).  The list is rendered to
# /etc/dns-sync/expected-int-names at build time, and the tool compares it
# against:
#
#   * the router: `gw` (10.3.1.1), whose mutable /var/lib/dnsmasq/names.hosts
#     is one of dnsmasq's two addn-hosts files.  ssh as leigh-admin (the
#     router has no root login) and sudo for the write — machines/gw/dns.nix.
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
# `sync` is strictly additive: it only adds missing names (router hosts lines /
# DO CNAMEs) and never deletes or rewrites existing records — many
# *.int.leighhack.org names legitimately point at other hosts, so other records
# are never touched.  On the router a record is one line *per address*, so a
# vhost gets an A and two AAAA lines from /etc/dns-sync/services1-addresses;
# completing a name that only has its IPv4 is the same additive edit.  `prune`
# is the explicit exception and is scoped tightly: it removes only (a) services1
# lines in names.hosts and (b) DO CNAMEs -> nginx.int, and only when the name
# was in the previous expected list (/var/lib/dns-sync/last-expected) but is no
# longer.
{
  config,
  lib,
  pkgs,
  CRANE,
  ...
}: let
  dnsSync = CRANE.crate {
    pname = "dns-sync";
    version = "0.1.0";
    member = "dns-sync";
    # `common-json` path dependency (see crates/common-rs/ and common/crane.nix).
    sharedCrates = ["json"];
  };

  # Every *.int.leighhack.org name this nginx serves (vhost names + aliases).
  # Shared with the gatus endpoint check: see common/nginx/int-vhost-names.nix.
  intVhostNames = import ../../common/nginx/int-vhost-names.nix {inherit lib;};
  intNames = intVhostNames config.services.nginx.virtualHosts;

  # What each of those names must answer with.  Three addresses, because the
  # router's file has always carried three lines per vhost (the old box's
  # dnsmasq-hosts did, and the file restored from it on 2026-10-21 does) and a
  # name without its IPv6 half is the failure machines/gw/README.md documents:
  # a dual-stack client tries IPv6, hangs, and only then falls back.  Read from
  # the reservation table, never repeated here — common/dns-records.nix owns
  # services1's addresses, exactly as it owns every other device's.
  RECORDS = import ../../common/dns-records.nix;
in {
  environment.etc."dns-sync/expected-int-names".text =
    lib.concatStringsSep "\n" intNames + "\n";

  # services1's addresses, one per line, IPv4 first (the order dnsmasq's hosts
  # file is grouped in).  dns-sync reads this instead of holding its own copy.
  environment.etc."dns-sync/services1-addresses".text =
    lib.concatStringsSep "\n" ([RECORDS.services1.ipv4] ++ RECORDS.services1.ipv6)
    + "\n";

  # Pinned router host keys — dns-sync sshes to the router and edits the DNS the
  # whole network resolves through, with StrictHostKeyChecking=yes (see
  # dns-sync/src/main.rs), so a router reinstall must be an explicit act here
  # rather than something accepted silently.  Regenerate with
  # `just router-known-hosts` and paste the result.  (network-status used to share
  # this pin; it now runs on gw and reads /proc locally, so dns-sync is its only
  # reader.)
  #
  # Two key types: these are the two host keys `gw` has (/etc/ssh/ssh_host_*)
  # as of the 2026-10-21 cutover.  The OPNsense box offered an ECDSA key too and
  # this file carried three lines; a key type the router does not offer is not a
  # pin, and a new one it starts offering fails loudly rather than silently.
  environment.etc."dns-sync/known_hosts".text = ''
    10.3.1.1 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGJtLDxvMHNXZqciSHB2KsBqsaO4iyp0yo6t9LEp9u5S
    10.3.1.1 ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQDolwIGnLdCI8U9wHxiKxxJBxBRdsSTyDBc9rjSUl2wwUVB3C8tGcmCxKTQGciB9pt9Zl70dgG2QsoNUl2uNXkIfN4tTory1HJsq3jaJVMEwuuw9B/g7iLH3UXrvQLYScsz01pwWZqbdtuyxklitgG1OcW7JTJ8BZbBFcZEyi/FXwlF8JlsyU7iY24c2PgAJpX0n7fXExm82dFMy7A0aejlc7vrDrHcHixSbRN5QpWfXtrWaZYvFJkvf2x+EvT36RgxKmorKtpeSqvC5eksCRBaaIanafr9gv3WsSc8IFvItuVusFK2IsyyO8TQhVjZ9KqZ5973DWhF3rLMCU7vbR2+sPTuJNURrv8mSl4K0ivKJxyDD5mCnFIbY9pwXSeqO7NogMU5qovB4O5aTBLopAdUxEg8Bzh7nAHzIxPM1mUduYl504ZAgEkGOrLg02CCssq5TIiul8Zmwr9bcqOYadfwGguMuZfp/d6nwxbhdE00QHYquKTLSBVdz4OPbsHOSq4DyGtwDsv55psGDdAp619ThFWK1kbFF2AZ0lf7FwNKPad8d178CL0VnJF8rgJGXdDXPDW6QK07P/tJTjj+9Lp6aejXeICAq8CwJMlILTBfoZ4MQTK867EjBXXIwrc5Semwvzm2k2xxo/M+F1P3MtXw5mFM+EsX1VBMkePWrPRltw==
  '';

  environment.systemPackages = [dnsSync];
}
