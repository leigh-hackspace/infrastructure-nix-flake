# The router as the network's **front door**: one certificate, every name, both gates.
#
# Read `docs/router-front-door-migration.md` before changing anything here. It owns the
# decision (every piece of nginx the network has **one** of terminates here — TLS and its
# renewal, the public and `*.int` vhosts, the `nginx-sso` gate, `anubis`, the
# `LOCAL_NETWORK` ACLs) and the order the move has to happen in. The boxes behind this one
# are *origins*: they serve, they hold no certificate, and they are reachable from this box
# and nowhere else, so they can be ignorant of where their traffic comes from because
# something in front of them is not.
#
# What this file is now, phase by phase:
#
#   * **The vhosts are not written here.** They are rendered from `common/front-door.nix`
#     by `common/nginx/front-door.nix`, so the door's config *is* the table: a name the
#     network resolves and a name this box serves can only disagree by failing an evaluation
#     (`./int-dns.nix`), and a vhost cannot exist in two places and be right in one of them.
#   * **The certificate is the network's.** `certDomains` in the table is the SAN set, and
#     it is not the union of what services1's cert happened to carry: `*.ai.int` cannot be
#     resolved (this box is authoritative for `int.leighhack.org` and no two-label name is
#     in `common/int-dns.nix`) and `*.test` is served by nothing, and every extra SAN is
#     another `_acme-challenge` TXT to win at every renewal for 60 days.
#   * **Both gates are here**, and that is a bug class removed rather than a service
#     relocated: `nginx-sso`'s auth endpoint answers `*:8082` on services1 and decides
#     LAN-or-tailnet from an `X-Real-IP` field only our nginx sets honestly. Next to the
#     nginx that calls it, its listener can be loopback.
#
# The invariant that outlives all of it: **whoever can reach an origin can assert an
# identity to it.** The apps behind the origins trust `X-WEBAUTH-USER` because they bind
# loopback (`crates/switch-gui/src/routes.rs` says so in the comment on the field), so the
# origin-reachability rules in ./firewall.nix and `CONFIG.ROUTER_ONLY` on the origins are
# what keep that sentence true once the bind is no longer the boundary. Nothing in this file
# may be deployed ahead of them.
#
# What has to be true of anything added to the table, and why:
#
#   * **Every upstream is an address, never a name.** dnsmasq runs on this box, so an nginx
#     that resolves its own upstreams at startup is asking the thing it is starting to start
#     itself, and a front door that waits on its own DNS is a boot-order loop with no
#     timeout. The table reads `common/hosts.nix` for the same reason.
#   * **`CONFIG.LOCAL_NETWORK` stays on the internal names.** ./firewall.nix polices the
#     *network*; this ACL polices the *name*, and it is the half that keeps a tailnet client
#     out of `truenas.int` and `monster.int`. This is also the last place `$remote_addr`
#     still names the client. If an app genuinely needs the peer, nginx's realip module is
#     built but un-optioned in this nixpkgs — `set_real_ip_from 10.3.1.1;` in `extraConfig`,
#     and it names the *only* address whose `X-Forwarded-For` may be believed.
#   * **A new name is a row in the table**, and for `*.int` also in `common/int-dns.nix`,
#     which ./int-dns.nix enforces in both directions once `frontDoor` names this box.
{
  config,
  lib,
  pkgs,
  ...
}: let
  CONFIG = import ../../common/config.nix;
  HOSTS = import ../../common/hosts.nix;

  # Both gates talk to each other and to nginx over this box's loopback. The anubis socket
  # lives where its module asserts it must (a path systemd creates, not one we choose).
  anubisSocket = "/run/anubis/anubis-front-door/front-door.sock";

  # The renderer, and the one place its arguments are allowed to appear: `certName` is the
  # `useACMEHost` of every vhost the door serves, and anubis's TARGET is the row that names
  # the app it fronts.
  door = import ../../common/nginx/front-door.nix {
    inherit
      lib
      pkgs
      config
      anubisSocket
      ;
    hostName = config.networking.hostName;
    certName = certDomain;
  };

  # The cert's name, which is also its primary SAN and the directory under /var/lib/acme.
  # Named after the registered domain rather than after the `*.int` half phase 1 started
  # with, because from here on this is the only certificate the network has: phase 3's last
  # step deletes services1's, and a fleet with exactly one cert should not have it named
  # after a subdomain of the other one.
  certDomain = CONFIG.PUBLIC_DOMAIN;

  # The first SAN of `certDomains` is the one nixpkgs takes as the cert's primary name, so
  # the rest are the extras.
  certExtraDomains = builtins.filter (d: d != certDomain) door.certDomains;

  anubisTarget = door.rows."speed.leighhack.org".upstream;
in {
  # `users.groups.secrets.members = ["nginx"]` is deliberately **not** copied from
  # machines/services1/http.nix. That line exists so services1's nginx can open a rendered
  # secret file under /run/secrets; nothing here opens one — the gate's config and the two
  # injected headers are build-time store paths, which nginx reads as any user — and group
  # `secrets` is read access to every runtime secret on the box.

  # ---------------------------------------------------------------------------
  # The certificate, and the two lines that make issuing it possible on the box
  # that runs the zone.
  # ---------------------------------------------------------------------------
  security.acme = {
    acceptTerms = true;

    defaults = {
      dnsProvider = "digitalocean";
      email = "admin@leighhack.org";
      dnsPropagationCheck = true;

      # **The one pair of lines that is not a copy of services1, and the reason they are
      # both here.** lego has to ask whether the TXT it wrote to DigitalOcean has arrived,
      # and on this box the naive question is unanswerable twice over.
      #
      # (1) *Whom it asks.* With no resolver configured lego asks the system resolver, the
      # system resolver is dnsmasq, and dnsmasq is authoritative for `int.leighhack.org`
      # (`local = "/int.leighhack.org/"` in ./dns.nix) and never forwards a name in the
      # zone — so `_acme-challenge.int.leighhack.org` is, from this box's point of view, a
      # name that authoritatively has no record, forever:
      #
      #   dig @127.0.0.1 _acme-challenge.int.leighhack.org TXT
      #   -> status: NXDOMAIN, flags: qr rd ra        (measured on gw, 2026-10-24)
      #
      # Hence naming the public resolvers, which are the ones DigitalOcean's answers are
      # visible through. The network's own resolver is *right* to answer that way; the fix
      # is on the asking side.
      #
      # (2) *Whether asking works at all.* Naming the resolvers does not un-teach them what
      # they cached, and an apex and its wildcard are solved by **one** TXT name — so the
      # phase-1 deploy lost the race alone: lego validated `*.int` (+38 s, Let's Encrypt
      # said so), then spent 60 s being told `NXDOMAIN` by 1.1.1.1 for `int` while the
      # record was still up (lego cleans up after the run), and the whole order died.
      # `--dns.propagation.wait` stops the asking: present the TXT, let the zone settle,
      # then go to the CA. ~2 min per SAN pair on a unit with `TimeoutStartSec=infinity`
      # behind a daily timer, in exchange for not depending on which answer a public
      # resolver happens to be holding.
      #
      # Both lines are load-bearing on a renewal timer, and neither failure is loud:
      # `security.acme` writes a minica self-signed certificate when lego fails, so nginx
      # starts, serves, and reports success. That is what `front-door-cert-check` below is
      # for, and why it reads the *issuer*.
      dnsResolver = "1.1.1.1:53,8.8.8.8:53";
      extraLegoFlags = ["--dns.propagation.wait=120s"];
    };

    certs.${certDomain} = {
      # The DO token is `DO_AUTH_TOKEN` inside the shared env-file secret, read by the acme
      # unit from this path at runtime. ./sops.nix renders it; the token is never printed,
      # copied, or embedded in a derivation (AGENTS.md, "Secrets") — which is the whole
      # reason this is `environmentFile` and not `config.sopsSecretText`.
      environmentFile = CONFIG.ENV_FILE;

      # nginx reads the key from this group.
      group = config.services.nginx.group;

      extraDomainNames = certExtraDomains;
    };
  };

  # ---------------------------------------------------------------------------
  # The only check that can tell an issued certificate from a *present* one.
  # ---------------------------------------------------------------------------
  #
  # `security.acme` does not fail when the CA does not answer: it writes a minica
  # self-signed certificate so "dependent services will still start", nginx comes up, and
  # `acme-<cert>.service` reports `Result=success`. Measured on the phase-1 deploy —
  # `Result=success`, a full cert directory, `nginx -t` clean, the canary answering 200,
  # and `issuer=CN=minica root ca 56254e`. Nothing distinguishes the two states except the
  # issuer, so the issuer is what gets checked (and expiry at 14 days, because the fallback
  # is offered to every renewal too and a renewal that fails at 04:00 should be red that
  # night rather than an expired-certificate dialog in March).
  #
  # **Wall-clock, and that is not a detail.** The first version was
  # `OnBootSec = "15min"; OnUnitActiveSec = "1d";` and it cost a failed deploy: a monotonic
  # timer whose window has already passed fires the instant it is created, so on a box two
  # days into its uptime the first switch started this service *inside the activation*, the
  # check exited 1 (correctly), and `switch-to-configuration` counts a unit failing during
  # activation as a failed switch — exit 4, `auto-rollback` armed, and on this machine the
  # rollback is a router reboot five minutes later. **A health check whose failure can arm a
  # rollback is a check that turns a warning into an outage**, so it stays out of the
  # activation transaction: `OnCalendar` cannot fire on appearance, and `Persistent` is off
  # because catching up is the same trick from the other direction.
  systemd.services.front-door-cert-check = {
    description = "The front door's certificate is CA-issued, not the minica fallback";

    serviceConfig = {
      Type = "oneshot";

      # Root, because that is who may read it: the cert directory is `acme:nginx 0750`, and
      # this is a check *about* that directory, not a consumer of the key. It reads the
      # issued files and never copies, prints, or logs them.
      ExecStart = pkgs.writeShellScript "front-door-cert-check" ''
        set -euo pipefail

        certDir="/var/lib/acme/${certDomain}"
        cert="$certDir/cert.pem"

        if [ ! -r "$cert" ]; then
          echo "no readable certificate at $cert — is security.acme deployed and acme-${certDomain}.service run?" >&2
          exit 1
        fi

        # `${certDomain}` is Nix interpolation and `$certDir` is the shell's; the difference
        # is one pair of characters, because in an indented Nix string a bare `$foo` is
        # literal text. The first version of this script wrote `$certDomain`, shipped it,
        # and died here — on the `set -u`, in the *minica branch* — with
        # `line 31: certDomain: unbound variable`, while trying to report that the
        # certificate was self-signed. A check that first runs its failure branch on the day
        # the cert goes wrong is a check nobody has ever run, so: braces for the strings that
        # come from Nix, and read the rendered script once
        # (`systemctl cat front-door-cert-check`).
        issuer=$("${pkgs.openssl}/bin/openssl" x509 -noout -in "$cert" -issuer)
        case "$issuer" in
          *minica*)
            echo "${certDomain} is the minica self-signed fallback ($issuer): lego never issued." >&2
            echo "  why:  sudo journalctl -u acme-order-renew-${certDomain}.service -n 50" >&2
            echo "  fix:  sudo systemctl start acme-order-renew-${certDomain}.service" >&2
            echo "        sudo systemctl restart nginx"
            exit 1
            ;;
        esac

        # Loud while there is still a week to be loud about it: the renewal is due at 60
        # days and the daily timer retries it, so 14 days means the retries have been failing
        # for a while.
        if ! "${pkgs.openssl}/bin/openssl" x509 -noout -in "$cert" -checkend $((14 * 86400)) >/dev/null; then
          echo "${certDomain} expires within 14 days: $issuer" >&2
          exit 1
        fi

        echo "${certDomain} is CA-issued and not near expiry: $issuer"
      '';
    };

    # Ordering, not a dependency: a check that runs while lego is mid-flight is a false
    # alarm, and it must not *require* lego — or a failed issuance would take down with it
    # the one unit that reports the failure. Hence `after`, not `requires`/`wants`.
    after = ["acme-order-renew-${certDomain}.service"];
  };

  systemd.timers.front-door-cert-check = {
    wantedBy = ["timers.target"];
    timerConfig = {
      # After this box's renewal timer (19:53 UTC at this generation), before anyone dials
      # the door. See the service for what a monotonic version did.
      OnCalendar = "*-*-* 06:30";
      RandomizedDelaySec = "15m";
    };
  };

  # ---------------------------------------------------------------------------
  # The gate: `nginx-sso`, next to the nginx that calls it.
  # ---------------------------------------------------------------------------
  #
  # Moved from services1 rather than duplicated: one gate, one cookie key, one OIDC client,
  # and the callback on loopback. Its config (`common/nginx-sso-config.nix`) is generated
  # with two build-time secrets in it, which is why the names have to be justified in
  # `common/sops.nix` for *this* box and not inherited — the generated file is a
  # world-readable store path like every other build-time secret.
  systemd.services.nginx-sso = let
    gateConfig = pkgs.writeText "nginx-sso-config" (import ../../common/nginx-sso-config.nix {
      inherit lib;
      sopsSecretText = config.sopsSecretText;
      listenAddr = "127.0.0.1";
    });
  in {
    description = "NGINX SSO (the front door's gate)";
    wantedBy = ["multi-user.target"];

    # No network ordering, and not a leftover: the gate listens on loopback and is reached
    # only by this box's nginx `auth_request`, so an `After=network-online.target` here
    # would be a unit ordered after something it does not require — which is exactly the
    # warning `just check` is meant to stay free of (AGENTS.md rule 9). It needs no
    # certificate either: TLS is nginx's half, and nginx's own `After=` on the acme unit
    # comes from `useACMEHost`.
    serviceConfig = {
      ExecStart = "${pkgs.nginx-sso}/bin/nginx-sso --frontend-dir=${pkgs.nginx-sso}/share/frontend -c ${gateConfig}";

      # The config's `audit_log.targets` writes here. Declared rather than hoped for: on
      # services1 nothing creates the directory, so the audit half of the gate has been
      # failing quietly.
      LogsDirectory = ["nginx-sso"];
      Restart = "always";
      RestartSec = "5s";
      StartLimitIntervalSec = 0;
    };
  };

  # ---------------------------------------------------------------------------
  # The bot gate.
  # ---------------------------------------------------------------------------
  #
  # One instance, fronting the one name that has ever needed it (`speed`, whose target is a
  # row in the table). Moving it moves the unix socket, and nginx must be in the `anubis`
  # group to open it — on services1 that line carried the comment "required due to unix
  # socket permissions"; without it every request through the gate is a 502 whose only
  # evidence is in nginx's error log.
  # No `enable`: this nixpkgs' anubis module has no enable option — an instance *is* the
  # unit, which is why services1's librespeed.nix sets only `defaultOptions`/`instances`.
  #
  # The socket directory is `/run/anubis/anubis-<instance>/` because the module asserts
  # exactly that when the settings mention a unix socket — it is the module telling us the
  # path systemd will create, not a convention to choose.
  services.anubis = {
    defaultOptions.settings = {
      SLOG_LEVEL = "debug";
      OG_PASSTHROUGH = true;
      OG_EXPIRY_TIME = "1h";
    };

    instances.front-door = {
      settings = {
        TARGET = anubisTarget;
        BIND = anubisSocket;
        METRICS_BIND = "/run/anubis/anubis-front-door/front-door-metrics.sock";
      };
    };
  };

  users.users.nginx.extraGroups = [config.users.groups.anubis.name];

  services.nginx = {
    enable = true;

    recommendedTlsSettings = true;
    recommendedOptimisation = true;
    recommendedGzipSettings = true;

    # ~60 vhosts, several of them long (`services1-status.int` is not the longest); nginx's
    # own failure here is at startup, which on this box means the front door is down while
    # the router keeps answering DNS and looks healthy. Inherited from services1 before it
    # was needed rather than after.
    appendHttpConfig = ''
      server_names_hash_bucket_size 128;
    '';

    virtualHosts = door.virtualHosts;
  };

  # ---------------------------------------------------------------------------
  # Table hygiene the renderer cannot express as a type.
  # ---------------------------------------------------------------------------
  assertions = [
    {
      # nginx's failure mode for a duplicated `server_name` is a config that does not load,
      # and on this box a config that does not load is every name the network has. The
      # table is keyed, so row keys cannot collide — but an *alias* can repeat a name, and
      # the renderer happily emits two servers with it.
      assertion = let
        all = builtins.concatLists (
          map (n: [n] ++ (door.rows.${n}.serverAliases or [])) (builtins.attrNames door.rows)
        );
      in
        all == lib.unique all;
      message = ''
        common/front-door.nix: a name is served by more than one row (as a key or as
        another row's serverAlias). nginx would get two `server_name`s for one name and
        refuse to start; fold the two rows into one, or drop the alias.'';
    }
    {
      # A row that points at a *name* rather than an address is the boot-order loop this
      # file's header describes. The table reads common/hosts.nix precisely so this cannot
      # happen by accident, and this is what keeps it from happening by amendment.
      # The first version of this assertion required a port and was right about the intent
      # and wrong about the fleet: `voron` is `http://10.3.2.50`, port 80 implied. The thing
      # being forbidden is a *hostname*, so that is what the pattern asks about.
      assertion =
        builtins.filter (n: let
          u = door.rows.${n}.upstream or "page";
        in
          u != "origin" && u != "page" && builtins.match "https?://[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+(:[0-9]+)?" u == null)
        (builtins.attrNames door.rows)
        == [];
      message = ''
        common/front-door.nix: every upstream must be `origin`, a page, or a URL with a
        dotted-quad address and a port. dnsmasq runs on this box: an upstream that is a
        name is nginx asking its own resolver to start itself. Read the address from
        common/hosts.nix.'';
    }
  ];
}
