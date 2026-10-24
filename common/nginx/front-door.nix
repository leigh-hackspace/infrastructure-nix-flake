# HELPER — function of {lib, pkgs, config, hostName, certName, anubisSocket} returning the
# front door's `services.nginx.virtualHosts`, rendered from `common/front-door.nix`.
#
# One row in, one vhost out, with no per-app knowledge anywhere in between. The point is
# that the door's config *is* the table, so two boxes cannot hold an opinion about one name
# (`docs/router-front-door-migration.md`, consequence 3 and phase 2). It has one consumer
# today: `machines/gw/http.nix`.
#
# It deliberately **reuses `common/nginx/int-vhost.nix`** rather than re-deriving nginx
# snippets: that helper is the code that already knows how this network writes a proxied
# location — recommended proxy settings, the ACL first, the optional body/timeout/buffering
# lines, the header-buffer bump. A second implementation of those six lines is a second
# thing to get subtly wrong on the box that terminates everything.
#
#   import common/nginx/front-door.nix {
#     inherit lib pkgs config hostName;
#     certName = "int.leighhack.org";                  # this box's useACMEHost
#     anubisSocket = "/run/anubis/front-door.sock";
#   }
#
# -> { virtualHosts, rows, originHost; }
#
# What the renderer decides, so that the table does not have to:
#
#   * **`forceSSL = true` for every name.** A front door terminates: HTTP is the redirect,
#     HTTPS is the answer. Three of services1's vhosts did not — `retro.int` and
#     `retro.leighhack.org` answered plain HTTP, `ha.int` had `addSSL` without `forceSSL` —
#     and all three redirect now. That is the intended shape of a door, not an accident of
#     the move, and the two rows say so.
#   * **the ACL text**, from the row's `"lan" | "router" | "none"`, defaulting by namespace:
#     an `*.int` name is LAN-only unless it opts out, a public name is open unless it opts
#     in. Safe direction in both cases, and the same rule `mkIntVhost` already applies.
#   * **the secret-bearing headers** (`lanToken`, `strataBearer`). A row may name a header
#     but may not name a credential: `common/front-door.nix` is DATA read by `import` and
#     must not be able to decrypt anything. The *names* are in the table; the values come
#     through `config.sopsSecretText` here, which is the audited gate — `common/sops.nix`
#     throws on a name absent from `infra.buildTimeSecrets`.
#
# And the invariant the table states and this file enforces: **an upstream is an address.**
# dnsmasq runs on this box, so an nginx that resolves its own upstreams at startup is asking
# the thing it is starting to start itself.
{
  lib,
  pkgs,
  config,
  hostName,
  certName,
  anubisSocket,
}: let
  CONFIG = import ../config.nix;
  TABLE = import ../front-door.nix {
    hosts = import ../hosts.nix;
  };
  mkIntVhost = import ./int-vhost.nix {inherit lib;};

  zone = CONFIG.LOCAL_DOMAIN;
  isInt = name: lib.hasSuffix ".${zone}" name;

  # `upstream = "origin"`: the origin box's own nginx, matched there by `Host`, which
  # `recommendedProxySettings` forwards untouched. Port 80, on purpose — the origin holds
  # no certificate, so TLS ends at this box and the internal hop is plain HTTP. That is the
  # design ("front door + origin"), and it is also why the origin's forward rule in
  # machines/gw/firewall.nix is what protects the apps rather than TLS being on.
  originUrl = "http://${TABLE.originHost}";

  aclText = acl:
    if acl == "lan"
    then CONFIG.LOCAL_NETWORK
    else if acl == "router"
    then CONFIG.ROUTER_ONLY
    else if acl == "none"
    then ""
    else
      throw ''
        common/front-door.nix: acl = ${lib.showString acl} is not one of
        "lan" | "router" | "none". The ACL is the vhost half of the network's boundary
        (./firewall.nix polices packets, this polices names), so "leave it out" is not
        an option it takes silently.'';

  readSecret = name: lib.strings.trim (builtins.readFile (config.sopsSecretText name));

  # ---------------------------------------------------------------------------
  # The gates, as location fragments.
  # ---------------------------------------------------------------------------

  # `nginx-sso`, on this box's loopback. Co-locating the gate with the nginx that calls it
  # is what lets its listener be `127.0.0.1`: on services1 it answers `*:8082` and decides
  # LAN-or-tailnet from an `X-Real-IP` that only our nginx sets honestly (migration doc,
  # finding 4). Every caller of this socket *is* this nginx, which is the sentence the
  # `X-WEBAUTH-USER` trust downstream depends on.
  ssoLocationConfig = ''
    auth_request /sso-auth;

    # Keep the session cookie fresh on every request
    auth_request_set $cookie $upstream_http_set_cookie;
    add_header Set-Cookie $cookie;

    # The signed-in user, for the app behind us. Who may assert it is the whole security
    # question (AGENTS.md), and the answer here is: only this nginx, because the apps bind
    # loopback on their own box and that box is reachable from this one only
    # (`CONFIG.ROUTER_ONLY` on the origins + the forward rules in ./firewall.nix).
    auth_request_set $username $upstream_http_x_username;
    proxy_set_header X-WEBAUTH-USER $username;

    # No cache
    add_header Cache-Control "no-cache, no-store, must-revalidate";
    add_header Pragma "no-cache";
    add_header Expires "0";
  '';

  ssoLocations = {
    # The gate's auth endpoint. `internal` = nginx will not serve this location to a client,
    # only to an auth_request subrequest.
    "/sso-auth" = {
      proxyPass = "http://127.0.0.1:8082/auth";
      extraConfig = ''
        internal;
        proxy_pass_request_body off;
        proxy_set_header Content-Length "";

        # The gate's own ACL matches on these fields: `x-host` is the name the client
        # dialled and `x-real-ip` is the client, taken from $remote_addr by the box that
        # terminates — which is now this one, and is precisely why the gate's answer means
        # something here and meant less on services1, where any LAN client could dial
        # :8082 directly and put anything in the field.
        proxy_set_header X-Origin-URI $request_uri;
        proxy_set_header X-Host $http_host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # The renewed SSO cookie exceeds nginx's default 4k proxy header buffer ("upstream
        # sent too big header"). busy >= buffer_size, busy < total buffers - one buffer.
        proxy_buffer_size       32k;
        proxy_buffers           16 8k;
        proxy_busy_buffers_size 32k;
      '';
    };

    # Where an unauthenticated request goes. Assets redirect to the root rather than to
    # themselves, or a 404 on one .css becomes a login loop for the whole page.
    "@error401" = {
      extraConfig = ''
        set $redirect_uri $scheme://$http_host$request_uri;
        if ($request_uri ~* "\.(js|css|png|jpg|jpeg|gif|ico|svg|woff|woff2|ttf|eot)$") {
          set $redirect_uri $scheme://$http_host/;
        }
        return 302 https://login.${CONFIG.PUBLIC_DOMAIN}/login?go=$redirect_uri;
      '';
    };

    # Replace the app's service worker with one that claims immediately, so a cached SW
    # cannot serve an app past its own logout.
    "~ /sw\\.js$" = {
      extraConfig = ''
        return 200 'self.addEventListener("install", function(e){e.waitUntil(self.skipWaiting());}); self.addEventListener("activate", function(e){e.waitUntil(self.clients.claim());});';
        add_header Content-Type application/javascript;
        add_header Cache-Control no-cache;
      '';
    };
  };

  # The page a `page = "bar"` row serves. It names the box and the certificate because the
  # question an operator has during the migration is "which front door answered?", and a
  # canary that cannot answer it is worth less than the curl that prints it.
  pageHtml = page: ''
    <!doctype html>
    <html lang="en">
      <head>
        <meta charset="utf-8">
        <title>${page}.${zone}</title>
      </head>
      <body>
        <h1>${page}.${zone}</h1>
        <p>Served by <strong>${hostName}</strong> — the front door's nginx, on the
           <code>${certName}</code> certificate. The record comes from
           <code>common/int-dns.nix</code> and this box's dnsmasq answers it.</p>
      </body>
    </html>
  '';

  renderRow = name: row: let
    acl =
      row.acl or (
        if isInt name
        then "lan"
        else "none"
      );
    # Is this name gated? (Bound once: it decides the limits below *and* which locations the
    # vhost gets, and those two must never disagree.)
    sso = row.sso or false;

    # Where the door sends this name. `anubis` rewrites it to the gate's socket because the
    # bot gate is a proxy, not a subrequest: it holds the connection and forwards to its own
    # TARGET, which machines/gw/http.nix sets from the row's real upstream.
    upstream =
      if (row.anubis or false)
      then "http://unix:${anubisSocket}"
      else if row.upstream == "origin"
      then originUrl
      else row.upstream;

    # Headers carrying a credential to an app that cannot hold one. Both were build-time
    # `readFile`s on services1 (`status.nix`, `strata-proxy.nix`); moving the name has to
    # move the header with it, because it is injected *towards* the app and only the box
    # that proxies to the app can add it.
    secretHeaders =
      lib.optionalString (row.lanToken or false) ''
        proxy_set_header X-Status-Token ${readSecret "status_dashboard_lan_token"};
      ''
      + lib.optionalString (row.strataBearer or false) ''
        # `proxy_set_header` *replaces* what the client sent, so the engine sees exactly one
        # key — the one nginx holds — rather than two.
        proxy_set_header Authorization "Bearer ${readSecret "strata_api_key"}";
      '';

    # A row may add locations (`tailscale.leighhack.org`'s `/admin` split is the
    # first). `recursiveUpdate` so a row that only adds a path keeps the default
    # `location /`'s proxy and limits rather than replacing them, and so the merge is
    # the same operation here as in `common/nginx/origin-vhost.nix`.
    base = mkIntVhost {
      proxyPass = upstream;
      useACMEHost = certName;
      forceSSL = true;
      acl = aclText acl;
      websockets = row.websockets or false;
      # An SSO row inherits the gate's own limits unless it says otherwise:
      # `mkSSOVirtualHost` has always applied 1024M/3600s to every name it fronts, so a door
      # that gates the same name with nginx's 1 MiB default would refuse uploads the old gate
      # allowed — a regression that only appears when someone attaches a file, and one the
      # rendered-config comparison caught on four names.
      bodySize =
        row.bodySize or (
          if sso
          then "1024M"
          else null
        );
      timeouts =
        row.timeouts or (
          if sso
          then "3600s"
          else null
        );
      proxyBuffering = row.proxyBuffering or null;
      proxyHeaderBuffers = row.proxyHeaderBuffers or false;
      serverAliases = row.serverAliases or [];
      extraConfig = secretHeaders + lib.optionalString (row.sso or false) ssoLocationConfig;
    };

    # A row may add locations (`tailscale.leighhack.org`'s `/admin` split is the
    # first). Each added location inherits the vhost's ACL: the ACL is the row's
    # statement about *who may have the name*, and a location that forgot it would be a
    # hole in the middle of a LAN-only vhost. `"/"` is the one exception — the base
    # location already carries the ACL, and a row that restated it would double it.
    extraLocations =
      lib.mapAttrs (
        name: loc:
          loc
          // {
            extraConfig =
              lib.optionalString (name != "/") (aclText acl)
              + (loc.extraConfig or "");
          }
      )
      (row.locations or {});

    proxied =
      base
      // lib.optionalAttrs (row ? locations) {
        locations = lib.recursiveUpdate base.locations extraLocations;
      };
  in
    if row ? page
    then {
      useACMEHost = certName;
      forceSSL = true;
      serverAliases = row.serverAliases or [];
      locations."/" = {
        root = "${pkgs.writeTextDir "${row.page}/index.html" (pageHtml row.page)}/${row.page}";
        extraConfig = aclText acl;
      };
    }
    else if row.sso or false
    then
      proxied
      // {
        extraConfig = ''
          # Send an unauthenticated request to the login page
          error_page 401 = @error401;
        '';
        locations = proxied.locations // ssoLocations;
      }
    else proxied;
in {
  virtualHosts = lib.mapAttrs renderRow TABLE.names;

  # Exported for the consumer's assertions, for anubis's TARGET, and for the certificate —
  # so that `machines/gw/http.nix` never re-derives a fact the table already owns. The SAN
  # set and the rows are one decision (a name in the table with no SAN behind it is a TLS
  # handshake that fails on that name), so they live in one file.
  rows = TABLE.names;
  inherit (TABLE) originHost certDomains;
}
