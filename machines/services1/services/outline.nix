{
  pkgs,
  lib,
  config,
  ...
}: let
  CONFIG = import ../../../common/config.nix;
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};
in {
  # Necessary for secret access
  users.groups.secrets.members = ["outline"];

  services.outline = {
    enable = true;
    port = 8400;
    publicUrl = "https://outline.leighhack.org";
    forceHttps = true;
    secretKeyFile = "/srv/outline/secret_key";
    utilsSecretFile = "/srv/outline/utils_secret";
    storage = {
      storageType = "local";
      localRootDir = "/srv/outline/data";
    };
    databaseUrl = "postgresql://outline:${lib.strings.trim (builtins.readFile (config.sopsSecretText "pg_pass"))}@127.0.0.1:5432/outline";
    redisUrl = "redis://127.0.0.1:8401";
    oidcAuthentication = {
      authUrl = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/authorize/";
      tokenUrl = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/token/";
      userinfoUrl = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/userinfo/";
      clientId = "pP0YLbCtnHYNJmMs6MUTeQ1WEFOMX4vA36eTHCFQ";
      clientSecretFile = CONFIG.OUTLINE_CLIENT_SECRET_FILE;
      scopes = [
        "openid"
        "email"
        "profile"
      ];
      usernameClaim = "preferred_username";
      displayName = "Leigh Hackspace";
    };
  };

  services.nginx.virtualHosts = {
    # This origin was the network's only site that no client could load at all: an
    # endless 301 at the same URL, `ERR_TOO_MANY_REDIRECTS`.
    #
    # The cause is `recommendedProxySettings` re-deriving `X-Forwarded-Proto` from *this*
    # hop, i.e. `http`, because the door→origin hop is plain HTTP on :80 by design — and
    # Outline 1.12.0 turning HTTPS enforcement on unless told otherwise
    # (`FORCE_HTTPS … ?? "true"`, `share/outline/build/server/env.js`). Its middleware
    # (`services/web.js`) 301s every request that does not look TLS-terminated to the
    # https URL *with the path preserved*; the door answers that over TLS, re-proxies it
    # as http, and the loop is closed. Measured on the box: `:8400` answers 301 bare and
    # **200 with `X-Forwarded-Proto: https`**, which is the whole diagnosis — and the
    # reason `forceHttps = false` is not the fix: it stops the loop but leaves Outline
    # convinced it is not behind TLS termination, which its own code warns weakens CSRF
    # protection (a cookie a sibling subdomain can write), and Outline builds its OIDC
    # `redirect_uri` from the same request, so login would then emit `http://`.
    #
    # `preserveForwardedHeaders` (see `common/nginx/origin-vhost.nix` before copying this
    # to another origin) is what makes the header honest. Checked against the gate that
    # doc asks for: Outline has **no inbound IP allowlist** — auth is OIDC only, so
    # nothing can start refusing traffic — and `PROXY_HEADERS_TRUSTED` already defaults
    # true, so `ctx.ip` has been reading the real client out of `X-Forwarded-For` all
    # along. What the flip actually changes for this app is that its audit log and its
    # per-IP rate limiter stop keying the whole internet to `10.3.1.1`.
    "outline.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8400";
      websockets = true; # Outline's collaborative editing
      preserveForwardedHeaders = true;
    };
  };
}
