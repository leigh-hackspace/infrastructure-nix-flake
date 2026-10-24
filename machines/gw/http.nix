# The router as a TLS endpoint: nginx, the network's **`*.int` certificate**, and the
# first two internal vhosts.  This is phase 1 of
# `docs/router-front-door-migration.md` — read that file's "The decision" before
# changing anything here, because the two things this file *deliberately does not*
# do are the interesting part:
#
#   1. **It does not serve the public names.** `*.leighhack.org`, `*.ai.leighhack.org`
#      and the `nginx-sso` gate in front of them stay terminated on services1, and
#      the internet keeps arriving there through ./firewall.nix's DNAT rows.  This box
#      is the only thing between every device in the building and its own traffic;
#      making it also the thing the internet dials is a different bet, and the
#      migration doc says why the bet was not taken.  What that buys, besides a
#      smaller blast radius: `login.int`/`*.leighhack.org` and the whole
#      `X-WEBAUTH-USER` chain (`../services1/lib/nginx-sso-config.nix`) do not move,
#      so the gate cannot end up behind one front door while the apps it gates are
#      behind another — consequence 4 of that doc, designed out rather than managed.
#   2. **It does not hold the network's wildcard.** The cert issued here covers
#      `int.leighhack.org` and `*.int.leighhack.org` and nothing else, so this box
#      *cannot* answer a public name even if a rule is opened by mistake.  It is also
#      what keeps two boxes from renewing one certificate against each other:
#      lego writes the TXT for a DNS-01 challenge at `_acme-challenge.<name>`, and two
#      issuers of overlapping names stomp on one record.  Overlap is still not zero —
#      services1's wildcard covers `*.int` until the flip deletes those vhosts from
#      it, and the window, its receipts and how to close it are in the migration doc.
#
# What has to be true of anything added to this file, and why:
#
#   * **Every upstream is an address, never a name.** dnsmasq runs on this box, so an
#     nginx that resolves its own upstreams at startup is asking the thing it is
#     starting to start itself, and a router whose front door waits on its own DNS is
#     a boot-order loop with no timeout.  Read them from `common/hosts.nix`
#     (10.3.1.1, 10.3.1.20, …) rather than typing them.
#   * **Keep `CONFIG.LOCAL_NETWORK` on the internal vhosts.** ./firewall.nix polices
#     the *network*; this ACL polices the *vhost*, and it is the half that keeps a
#     tailnet client out of `truenas.int` and `monster.int`.  A vhost reached from the
#     router's own interfaces still arrives with a client address the ACL can read, so
#     nothing about the move weakens it — but the ACL only works where the client's
#     real address survives, which is a thing to re-check per hop as the migration
#     proceeds (see the migration doc's consequence 2).
#   * **A new `*.int` name here is a row in `common/int-dns.nix`** and is enforced by
#     ./int-dns.nix, the guard that reads this file's `virtualHosts` and refuses the
#     evaluation if the two disagree.
{
  config,
  lib,
  pkgs,
  ...
}: let
  CONFIG = import ../../common/config.nix;
  HOSTS = import ../../common/hosts.nix;
  mkIntVhost = import ../../common/nginx/int-vhost.nix {inherit lib;};

  # The zone this box answers `*.int` for — the same string dnsmasq's `domain=` and
  # `local=` use in ./dns.nix, read rather than retyped.
  zone = CONFIG.LOCAL_DOMAIN;

  # The cert's name, which is also its primary domain and the directory under
  # /var/lib/acme.  Named after the zone it covers rather than reusing
  # `leighhack.org` — services1's cert — because two boxes with one cert directory
  # name and one overlapping SAN set is the race this file's header describes.
  intCert = zone;
in {
  # `users.groups.secrets.members = ["nginx"]` is deliberately **not** copied from
  # machines/services1/http.nix.  That line exists so services1's nginx can open a
  # rendered secret file; nothing here opens one (no basic auth, no SSO gate), and
  # group `secrets` is read access to every runtime secret on the box.
  security.acme = {
    acceptTerms = true;

    defaults = {
      dnsProvider = "digitalocean";
      email = "admin@leighhack.org";
      dnsPropagationCheck = true;

      # **The one line that is not a copy of services1, and the reason it is here.**
      # lego's propagation check asks "is the TXT record answering yet?" and, with no
      # resolver configured, asks *the system resolver* — which on this box is
      # dnsmasq, and dnsmasq is authoritative for `int.leighhack.org`
      # (`local = "/int.leighhack.org/"` in ./dns.nix) and never forwards a name in
      # the zone.  So the TXT lego wrote to DigitalOcean for
      # `_acme-challenge.int.leighhack.org` is, from this box's point of view, a name
      # that authoritatively has no record:
      #
      #   dig @127.0.0.1 _acme-challenge.int.leighhack.org TXT
      #   -> status: NXDOMAIN, flags: qr rd ra     (measured on gw, 2026-10-24)
      #
      # forever, and the challenge fails at propagation rather than at issuance.  The
      # network's own resolver is right to answer that way — that is what makes it
      # authoritative for the network — so the fix is on the asking side: name the
      # public resolvers, which are the ones DigitalOcean's answers are visible
      # through.  Comma-separated because lego takes a list in one argument.
      #
      # It is also the check the *service*'s renewal does every day for the rest of
      # this cert's life, so the line is load-bearing on a 60-day timer and belongs in
      # ./README.md's list of things that only break at night.
      dnsResolver = "1.1.1.1:53,8.8.8.8:53";
    };

    certs.${intCert} = {
      # The DO token is `DO_AUTH_TOKEN` inside the shared env-file secret, read by the
      # acme unit from this path at runtime.  `machines/gw/sops.nix` is what renders
      # it, and the token is never printed, copied or embedded in a derivation
      # (AGENTS.md, "Secrets") — which is the whole reason this is
      # `environmentFile` and not `config.sopsSecretText`.
      environmentFile = CONFIG.ENV_FILE;

      # nginx reads the key from this group, the same handoff services1 uses.
      group = config.services.nginx.group;

      # One wildcard, because one wildcard is all the router can be dialled for:
      # every name `common/int-dns.nix` answers is a single label under
      # `int.leighhack.org`, and a name with two labels under it (`foo.ai.int`) is not
      # in that table, so it is not in the zone the router answers either — dnsmasq
      # would be authoritatively silent about it.  services1's cert carries
      # `*.ai.int.leighhack.org` and `*.test.leighhack.org`; nothing in the tree
      # serves a name in either space, and the cleanup is a follow-up rather than
      # something to inherit here.
      extraDomainNames = ["*.${zone}"];
    };
  };

  services.nginx = {
    enable = true;

    recommendedTlsSettings = true;
    recommendedOptimisation = true;
    recommendedGzipSettings = true;

    # Inherited from services1's http.nix before it is needed, not after.  Two names
    # do not need 128 hash buckets; the ~34 that arrive with the flip do, and the
    # deploy that moves the front door is the wrong one to discover it in.  nginx's
    # own failure here is at startup, which on this box means the front door is down
    # while the router keeps answering DNS and looks healthy.
    appendHttpConfig = ''
      server_names_hash_bucket_size 128;
    '';

    virtualHosts = {
      # The DNS canary, and the reason it is the first vhost here rather than one of
      # the apps: it has no upstream, so a 200 from it is proof of exactly the four
      # things this box is newly responsible for and nothing else — the record
      # (common/int-dns.nix → /etc/dnsmasq-int-vhosts.hosts), this nginx, this
      # certificate, this ACL.
      #
      # The page says which box served it, on purpose.  During the migration the name
      # resolves at services1 and is served by both boxes, so "which front door
      # answered?" is the question an operator actually has, and a canary that cannot
      # answer it is worth less than the curl that prints it.
      "bar.${zone}" = {
        useACMEHost = intCert;
        forceSSL = true;

        locations."/" = {
          root = "${pkgs.writeTextDir "bar/index.html" ''
            <!doctype html>
            <html lang="en">
              <head>
                <meta charset="utf-8">
                <title>bar.int.leighhack.org</title>
              </head>
              <body>
                <h1>bar.int.leighhack.org</h1>
                <p>Served by <strong>gw</strong> — the router's nginx
                   (${config.networking.hostName}), on the <code>${intCert}</code>
                   certificate. The record comes from common/int-dns.nix and is
                   answered by this box's dnsmasq.</p>
              </body>
            </html>
          ''}/bar";
          extraConfig = CONFIG.LOCAL_NETWORK;
        };
      };

      # The router's own dashboard (./network-status.nix, 10.3.1.1:8091).  The first
      # app vhost to be fronted here is the one whose upstream is this box: services1's
      # copy of the vhost proxies *out* to 10.3.1.1, and after the move the request
      # never leaves.  It is also the only vhost on the network that can be tested with
      # the router as the subject rather than the object — `curl --resolve
      # network-info.int.leighhack.org:443:10.3.1.1 https://…` asks the front door about
      # the front door.
      #
      # An address, not a name: see this file's header.
      "network-info.${zone}" = mkIntVhost {
        useACMEHost = intCert;
        proxyPass = "http://${HOSTS.router}:8091";
      };
    };
  };
}
