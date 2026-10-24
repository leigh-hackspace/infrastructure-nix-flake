# The router as a TLS endpoint: nginx, the network's **`*.int` certificate**, and the
# first two internal vhosts.  This is phase 1 of
# `docs/router-front-door-migration.md` first: it records the decision (every piece of
# nginx the network has **one** of terminates here — TLS and its renewal, the public names,
# the `nginx-sso` gate, `anubis`, the `LOCAL_NETWORK` ACLs — and the boxes behind `gw` are
# *origins*: they serve, hold no certificate, and are reachable from this box and nowhere
# else, so they can be ignorant of where their traffic comes from because something in front
# of them is not).  The two things this file has not done *yet* are staging, not design:
#
#   1. **The public names are not here yet.** Today `*.leighhack.org` is still terminated on
#      services1 and ./firewall.nix's DNAT rows carry the internet there.  When they arrive
#      they arrive *with* the gate in front of them: `nginx-sso`'s `X-WEBAUTH-USER`
#      injection is trusted by whatever serves the name, so gate and names cannot move in
#      separate changes — and co-locating them is what lets the gate's listener stop being
#      `0.0.0.0:8082`, which it is on services1 today (consequence 4).
#   2. **This cert is not yet the network's only cert.** It covers `int.leighhack.org` +
#      `*.int.leighhack.org`, so while services1 still issues for `*.int` as part of its
#      wildcard, two boxes write one TXT record (`_acme-challenge.int`) — the shape
#      AGENTS.md says must fail rather than race, with its window quantified in the
#      migration doc's findings and closed by phase 3, where services1 loses
#      `security.acme` altogether.  Grow this SAN set in the change that takes the public
#      vhosts, and do not inherit `*.ai.int`/`*.test` while growing it: no vhost serves
#      either, and every extra SAN is an extra TXT at every renewal for 60 days.
#
# And the invariant that outlives all of it: **whoever can reach an origin can assert an
# identity to it.** The apps behind the origins trust `X-WEBAUTH-USER` because they bind
# loopback today — `crates/switch-gui/src/routes.rs` says so in the comment on the field —
# so nothing in this file may be deployed ahead of the origin-reachability rule that keeps
# that sentence true once the bind is no longer the boundary.
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
#     tailnet client out of `truenas.int` and `monster.int`.  This is also the last place
#     `$remote_addr` still names the client: when the origins arrive, the ACL does not get
#     copied downstream, it stays here, and the origin's rule becomes "`10.3.1.1`, nothing
#     else" (consequence 2).  If an app genuinely needs the peer, nginx's realip module is
#     built but un-optioned in this nixpkgs — `set_real_ip_from 10.3.1.1;` in
#     `extraConfig`, and it names the *only* address whose `X-Forwarded-For` may be believed.
#   * **A new `*.int` name here is a row in `common/int-dns.nix`** and is enforced by
#     ./int-dns.nix, the guard that reads this file's `virtualHosts` and refuses the
#     evaluation if the two disagree.  The public names need the same relationship and do
#     not have it yet: `common/public-names.nix` is phase 2's file, and until it exists a
#     public vhost added here is checked by nothing but gatus, hours later.
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

      # **Stop asking a resolver whether the TXT has propagated; wait.** The line
      # above fixes *which* resolvers are asked, and that was not enough: the phase-1
      # deploy lost the challenge anyway, on the second of this cert's two SANs.
      #
      # What happened, off the journal of the first `just switch-gw` (2026-10-24):
      # lego validated `*.int.leighhack.org` at +38 s — the TXT existed, 1.1.1.1 was
      # answering it, LE said "The server validated our request" — then began the
      # check for `int.leighhack.org`, which is the **same TXT name**
      # (`_acme-challenge.int.leighhack.org`: an apex and its wildcard share one
      # challenge record), and asked 1.1.1.1 about it for the next 60 s. Every answer
      # was NXDOMAIN, the window closed and the order died:
      #
      #   ERROR … [int.leighhack.org: dns01: time limit exceeded: last error:
      #         recursive nameservers: NS 1.1.1.1:53 returned NXDOMAIN for
      #         _acme-challenge.int.leighhack.org.]
      #
      # So the resolver that had just confirmed the record spent the next minute
      # denying it ever existed, while the record was still up (lego cleaned it only
      # afterwards). `docs/router-front-door-migration.md`'s "two boxes, one TXT
      # record" finding is about two *boxes* writing that name; this is the same race
      # with one writer, and it is not a thing that can be reasoned about further —
      # it is a cache answering from a negative it was told about earlier, which is
      # what a resolver is for.
      #
      # Hence the wait: `--dns.propagation.wait` tells lego to skip both propagation
      # checks (authoritative and recursive) and simply let the zone settle before it
      # asks the CA. Two domains × 120 s, which is minutes against a unit with
      # `TimeoutStartSec=infinity` on a daily timer, in exchange for not depending on
      # which answer a public resolver happens to have cached. `dnsResolver` stays:
      # lego still uses those resolvers to find the zone apex and any CNAME, and the
      # reason it must not use *this box's* dnsmasq for that is unchanged above.
      #
      # Spliced into the lego command **unescaped** (`extraLegoFlags` is
      # `concatStringsSep " "` in this nixpkgs' module), so: one argument, no spaces,
      # and read the rendered script back after a switch rather than trusting this
      # comment (AGENTS.md rule 10).
      extraLegoFlags = ["--dns.propagation.wait=120s"];
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
                <p>Served by <strong>${config.networking.hostName}</strong> — the
                   router's nginx, on the <code>${intCert}</code> certificate. The
                   record comes from common/int-dns.nix and is answered by this box's
                   dnsmasq.</p>
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

  # ---------------------------------------------------------------------------
  # The check that can tell an issued certificate from a *present* one.
  #
  # `security.acme` here does not fail when the CA does not answer. The
  # "Ensure certificate" unit writes **minica's own self-signed certificate** into
  # the cert directory when lego exits non-zero — nginx then starts, the vhosts
  # serve, `acme-int.leighhack.org.service` reports `Result=success` and the whole
  # box looks healthy behind a certificate no client trusts. Measured on the phase-1
  # deploy: every check `docs/router-front-door-migration.md`'s runbook listed passed
  # — `Result=success`, a populated `/var/lib/acme/int.leighhack.org/`, `nginx -t`, a
  # 200 from the canary, the `After=acme-…` pairing — and
  # `openssl x509 -issuer` said `CN=minica root ca 56254e`. Nothing distinguishes the
  # two states except the issuer, so the issuer is what gets checked, and the runbook
  # now says so.
  #
  # A timer rather than a one-shot because the fallback is offered to the renewal too,
  # every day, for the rest of this box's life: a renewal that fails at 04:00 should
  # be a red unit that night and not an expired-certificate dialog in March. It runs
  # 15 min after boot and daily after that — after the acme units have finished (they
  # run at boot) and, since the renewal timer here fires at 19:53 UTC, within a day of
  # any renewal that fell back. `systemctl start front-door-cert-check` is the
  # after-a-switch form; it is cheap enough to be part of the deploy rather than a
  # thing someone remembers.
  systemd.services.front-door-cert-check = {
    description = "The front door's certificate is CA-issued, not the minica fallback";

    serviceConfig = {
      Type = "oneshot";

      # Root, because that is who may read it: the cert directory is `acme:nginx
      # 0750`/`0640` and this is a check *about* that directory, not a consumer of the
      # key. Reads the issued files; never copies, prints or logs them.
      ExecStart = pkgs.writeShellScript "front-door-cert-check" ''
        set -euo pipefail

        certDir="/var/lib/acme/${intCert}"
        cert="$certDir/cert.pem"

        if [ ! -r "$cert" ]; then
          echo "no readable certificate at $cert — is security.acme deployed and acme-${intCert}.service run?" >&2
          exit 1
        fi

        issuer=$("${pkgs.openssl}/bin/openssl" x509 -noout -in "$cert" -issuer)
        case "$issuer" in
          *minica*)
            echo "$intCert is the minica self-signed fallback ($issuer): lego never issued." >&2
            echo "  why:  sudo journalctl -u acme-order-renew-${intCert}.service -n 50" >&2
            echo "  fix:  sudo systemctl start acme-order-renew-${intCert}.service" >&2
            echo "        sudo systemctl restart nginx"
            exit 1
            ;;
        esac

        # Loud while there is still a week to be loud about it. The renewal is due at
        # 60 days and the daily timer retries it, so 14 days is not a warning about a
        # problem, it is a warning that the retries have been failing for a while.
        if ! "${pkgs.openssl}/bin/openssl" x509 -noout -in "$cert" -checkend $((14 * 86400)) >/dev/null; then
          echo "$intCert expires within 14 days: $issuer" >&2
          exit 1
        fi

        echo "$intCert is CA-issued and not near expiry: $issuer"
      '';
    };

    # Ordering, not a dependency: a check that runs while lego is mid-flight is a
    # false alarm, and `After=` is what makes the two units line up when they are in
    # the same transaction (a boot, a switch). It must not *require* lego, or a failed
    # issuance would take the check that reports it down with it.
    startAfter = ["acme-order-renew-${intCert}.service"];
  };

  systemd.timers.front-door-cert-check = {
    wantedBy = ["timers.target"];
    timerConfig = {
      OnBootSec = "15min";
      OnUnitActiveSec = "1d";
      AccuracySec = "1min";
    };
  };
}
