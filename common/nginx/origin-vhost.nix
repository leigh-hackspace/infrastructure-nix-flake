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
  }: {
    inherit serverAliases;

    # Server-level, so it covers every location a caller adds as well as `/`.
    extraConfig = CONFIG.ROUTER_ONLY;

    locations =
      lib.recursiveUpdate {
        "/" = {
          inherit proxyPass;
          recommendedProxySettings = true;
          proxyWebsockets = websockets;

          extraConfig =
            lib.optionalString (bodySize != null) ''
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
