# HELPER — function of {lib, sopsSecretText, listenAddr} returning nginx-sso's YAML.
#
# The front door's gate, generated where the gate runs. It moved here from
# `machines/services1/lib/nginx-sso-config.nix` with the duty rather than being copied, so
# there is one cookie key, one OIDC client and one ACL for the network's logins.
#
# `listenAddr` is the argument that is the whole point of the move. It was `0.0.0.0`, and
# services1's `ss -lntp` read `*:8082`: the auth endpoint the whole network's
# authentication depends on answered any LAN client, while the ACL below decides "is this
# caller LAN or tailnet" from an `x-real-ip` **field** that only our nginx sets honestly and
# any other caller can put anything in. Co-locating the gate with the nginx that calls it is
# what lets the caller be this box alone, so `machines/gw/http.nix` passes `127.0.0.1`.
#
# services1 has no gate any more; the migration is deployed
# (`docs/router-front-door-migration.md`).
#
# Both secrets go in through `sopsSecretText`, which puts them in a world-readable store
# path; `common/sops.nix`'s `infra.buildTimeSecrets` carries the sentence justifying each
# one, and re-justifies it per box rather than inheriting it.
{
  lib,
  sopsSecretText,
  listenAddr,
}: let
  CONFIG = import ./config.nix;

  readSecret = name: lib.strings.trim (builtins.readFile (sopsSecretText name));
in
  lib.generators.toYAML {} {
    login = {
      title = "Leigh Hackspace - Login";
      default_method = "oidc";
      hide_mfa_field = true;
      names.oidc = "Leigh Hackspace Account";
    };

    # The cookie domain is the registered domain, so one login serves every name the door
    # answers — and renaming the login host would invalidate every session at once.
    cookie = {
      domain = ".${CONFIG.PUBLIC_DOMAIN}";
      authentication_key = readSecret "nginx_sso_auth";
      expire = 86400;
    };

    listen = {
      addr = listenAddr;
      port = 8082;
    };

    audit_log = {
      targets = [
        "fd://stdout"
        "file:///var/log/nginx-sso/audit.jsonl"
      ];
      events = [
        "access_denied"
        "login_success"
        "login_failure"
        "logout"
        "validate"
      ];
      headers = ["x-origin-uri"];
      trusted_ip_headers = [
        "X-Forwarded-For"
        "RemoteAddr"
        "X-Real-IP"
      ];
    };

    acl.rule_sets = [
      # Grant Authenticated Access
      {
        rules = [
          {
            field = "x-host";
            regexp = ".*";
          }
        ];
        allow = ["@_authenticated"];
      }
      # Grant Tailscale (IPv4)
      {
        rules = [
          {
            field = "x-real-ip";
            regexp = "^100.64.";
          }
        ];
        allow = ["@_anonymous"];
      }
      # Grant Tailscale (IPv6)
      {
        rules = [
          {
            field = "x-real-ip";
            regexp = "^fd7a:115c:a1e0";
          }
        ];
        allow = ["@_anonymous"];
      }
      # Grant Hackspace LAN (IPv4)
      {
        rules = [
          {
            field = "x-real-ip";
            regexp = "^10.3.";
          }
        ];
        allow = ["@_anonymous"];
      }
      # Grant Hackspace LAN (IPv6)
      {
        rules = [
          {
            field = "x-real-ip";
            regexp = "^2001:8b0:1d14";
          }
        ];
        allow = ["@_anonymous"];
      }
      # Grant Chris (IPv6)
      {
        rules = [
          {
            field = "x-real-ip";
            regexp = "^2a02:8010:6680";
          }
        ];
        allow = ["@_anonymous"];
      }
    ];

    providers.oidc = {
      client_id = "pyBuPjaPD7xaiy8hAQpy4W02A3G0aIKyUakPtSaD";
      client_secret = readSecret "nginx_sso_client_secret";
      redirect_url = "https://login.${CONFIG.PUBLIC_DOMAIN}/login";
      issuer_name = "Leigh Hackspace";
      issuer_url = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/nginx-login/";
    };
  }
