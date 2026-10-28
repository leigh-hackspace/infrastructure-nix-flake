# HELPER — function of {lib} returning the origin-vhost builder services1's apps call.
#
# An **origin** is what a box behind the front door runs (`docs/router-front-door-migration.md`):
# the same vhost the app has always had,
# with everything that terminates, authenticates or decides a network removed from it.
# So this builder is `int-vhost.nix` minus three things and plus one:
#
#   * **no `useACMEHost`, no `addSSL`, no `forceSSL`.** An origin terminates nothing.
#     It listens on plain HTTP (nixpkgs' nginx default port 80) and `gw` is the only
#     client that will ever reach it. That is what makes `machines/services1/http.nix`
#     able to delete `security.acme` without leaving a vhost pointing at a certificate
#     that no longer exists.
#   * **no `nginx-sso`.** The gate is on `gw` (machines/gw/http.nix); a second
#     `auth_request` here would be a second opinion about who is signed in, and the
#     header `gw` sets (`X-WEBAUTH-USER`) is forwarded by nginx by default because
#     nothing in this builder replaces it.
#   * **no per-vhost LAN ACL.** `CONFIG.ROUTER_ONLY` goes at *server* level rather than
#     inside `location /`: the front door, and nothing else. This is the app-layer half
#     of the boundary whose packet half is the origin-reachability rules in
#     `machines/gw/firewall.nix`, and it is what still holds if a firewall rule is ever
#     widened at 04:00 (AGENTS.md rule 17 is the same argument about loopback).
#
# The one addition is `locations`: a few origins are app-shaped rather than
# one-line-shaped (`ai.leighhack.org`'s `/resources` root, `ha.int`'s auth rewrite,
# matrix's three API prefixes), and they need to keep those paths while losing the
# certificate. `lib.recursiveUpdate` merges them over the default `location /`, so a
# caller adds a path without restating the proxy it would have inherited.
#
# --- `preserveForwardedHeaders`, and why it is opt-in ---------------------
#
# `recommendedProxySettings` (nixpkgs) emits `X-Real-IP $remote_addr` and
# `X-Forwarded-Proto $scheme`. On an origin those two are lies: `$remote_addr` is the
# front door (`10.3.1.1`) and `$scheme` is `http`, because the door→origin hop is plain
# HTTP on :80 by design — while the door itself had already set both truthfully for the
# client's real connection. So every origin behind the door has, since 2026-10-24,
# silently replaced the client's address with the router's and https with http.
#
# Most apps never notice because they take their public URL from their own config. Apps
# that build absolute URLs from the request notice immediately: authentik rendered
# `base: "http://id.leighhack.org/"` into an HTTPS page and the browser blocked
# `/api/v3/core/users/me/` as mixed content (2026-10-28, the day `id.int` became an
# origin). Verified by fetching the same page twice — through the door (`http`) and
# straight to the container with `X-Forwarded-Proto: https` (`https`).
#
# `preserveForwardedHeaders = true` passes the door's values through instead, and it is
# safe to do at all because of an invariant this box already enforces twice over:
# **there is no path to an origin that does not go through the front door**
# (`originReachabilityRules` in `machines/gw/firewall.nix` is
# `ip daddr services1 tcp dport { 80, 443 } ip saddr != 10.3.1.1 counter drop`, and
# `CONFIG.ROUTER_ONLY` is the app-layer half). So `$http_x_real_ip` and friends are
# always the door's honest values, never a client-supplied forgery — which is the same
# shape of argument `crates/switch-gui/src/routes.rs` makes about its loopback socket.
#
# It is still **opt-in** rather than the default, for one reason: it is also correct for
# every other origin, and flipping the default changes what seven running apps believe
# about who is calling. An origin that applies its own IP allowlist currently sees
# `10.3.1.1` for *everyone* and so lets the whole internet through a LAN rule; after the
# flip it sees real client addresses and could start refusing traffic that works today.
# That is a bug being fixed and a behaviour change at the same moment, so each origin
# should get it deliberately, with its own allowlists read first. The audit is an open
# item in `docs/follow-ups.md`.
#
# What `id.int` was flipped on (2026-10-28, measured against the migrated database rather
# than reasoned about): **0** `GeoIPPolicy` rows, and **0 of 11** expression policies read
# the client address (`2` read `request.user`/session, which is unaffected) — so no
# authentik policy decision can change. What it does change is exactly the two lies: the
# emitted base URL, and the client IP in authentik's event log.
#
#   mkOriginVhost = import common/nginx/origin-vhost.nix { inherit lib; };
#
#   services.nginx.virtualHosts."foo.int.leighhack.org" = mkOriginVhost {
#     proxyPass = "http://127.0.0.1:8090";
#     websockets = true;
#     bodySize = "2048M";
#     timeouts = "3600s";
#     proxyBuffering = false;
#     proxyHeaderBuffers = false;
#     serverAliases = [];
#     locations = {};          # extra locations, merged over "/"
#     extraConfig = "";        # appended to the default location, not the server
#     preserveForwardedHeaders = false;  # true = forward the door's X-Real-IP/XFF/XFP
#   };
#
# A vhost with no proxy at all (`element.leighhack.org`'s built SPA, `retro.int`'s
# filesystem root) is written out by hand with `CONFIG.ROUTER_ONLY` as its
# server-level `extraConfig`; the helper is for the proxied majority.
{lib}: let
  CONFIG = import ../config.nix;
in
  {
    proxyPass,
    websockets ? false,
    bodySize ? null,
    timeouts ? null,
    proxyBuffering ? null,
    proxyHeaderBuffers ? false,
    extraConfig ? "",
    serverAliases ? [],
    locations ? {},
    preserveForwardedHeaders ? false,
  }: {
    inherit serverAliases;

    # Server-level, so it covers every location a caller adds as well as `/`.
    extraConfig = CONFIG.ROUTER_ONLY;

    locations =
      lib.recursiveUpdate {
        "/" = {
          inherit proxyPass;
          # When `preserveForwardedHeaders` is on, `recommendedProxySettings` is turned
          # *off* rather than overridden: nixpkgs' recommended set and our own lines would
          # be two `proxy_set_header` directives for the same field in the same location,
          # and which one wins is not something to leave an app's URLs to guess about.  The
          # set below is that same set — including `Host $host`, which is *not* optional,
          # because nginx's own default is `$proxy_host` and an app that routes tenants or
          # vhosts by `Host` would silently get `127.0.0.1:9000`.
          recommendedProxySettings = !preserveForwardedHeaders;
          proxyWebsockets = websockets;

          extraConfig =
            lib.optionalString preserveForwardedHeaders ''
              # The front door already set these three honestly: `X-Forwarded-Proto` from
              # the client's real TLS connection, `X-Real-IP` to the client.  Re-deriving
              # them from *this* hop turns `https` into `http` (the door→origin hop is
              # plain HTTP on :80, by design) and every client into `10.3.1.1`.  For an app
              # that renders absolute URLs into its own HTML that is a hard failure:
              # authentik emitted `base: "http://id.leighhack.org/"` into an HTTPS page and
              # the browser blocked its API calls as mixed content.
              proxy_set_header Host              $host;
              proxy_set_header X-Real-IP         $http_x_real_ip;
              proxy_set_header X-Forwarded-For   $http_x_forwarded_for;
              proxy_set_header X-Forwarded-Proto $http_x_forwarded_proto;
              proxy_set_header X-Forwarded-Host  $http_x_forwarded_host;
              proxy_set_header X-Forwarded-Server $hostname;
            ''
            + lib.optionalString (bodySize != null) ''
              client_max_body_size ${bodySize};
            ''
            + lib.optionalString (timeouts != null) ''
              proxy_connect_timeout ${timeouts};
              proxy_send_timeout    ${timeouts};
              proxy_read_timeout    ${timeouts};
              send_timeout          ${timeouts};
            ''
            + lib.optionalString (proxyBuffering != null) ''
              proxy_buffering ${
                if proxyBuffering
                then "on"
                else "off"
              };
            ''
            + lib.optionalString proxyHeaderBuffers ''
              # The upstream's response headers exceed nginx's default 4k proxy header
              # buffer ("upstream sent too big header").
              proxy_buffer_size       32k;
              proxy_buffers           16 8k;
              proxy_busy_buffers_size 32k;
            ''
            + extraConfig;
        };
      }
      locations;
  }
