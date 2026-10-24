# HELPER — function of {lib} returning the internal-vhost builder used by mkVirtualHost; not a module.
# Helper for the LAN-only "*.int.leighhack.org" virtual hosts — the internal
# counterpart of sso-vhost.nix (which is for public, SSO-protected apps).
# Every one of these is the same shape: the wildcard ACME cert, forceSSL, and one
# reverse-proxied location with the recommended proxy settings plus the
# LOCAL_NETWORK ACL.
#
# Shared, not services1-only: services1 is the TLS terminator for every machine
# in this network, so a vhost here can proxy to a service running on another box
# (see the status dashboards in machines/services1/services/status.nix).  That is
# why this and sso-vhost.nix live in common/ rather than machines/services1/lib/.
#
# Two boxes call it today, and they do **not** share a certificate: `useACMEHost`
# defaults to services1's wildcard, and `machines/gw/http.nix` overrides it to the
# `int.leighhack.org` cert that box issues (only `*.int` names, so the router cannot
# answer a public name *in this phase*).  A vhost added on `gw` therefore passes
# `useACMEHost` explicitly — the default is the name that works on services1, not the
# name that exists everywhere.  Phase 3 of `docs/router-front-door-migration.md` ends
# with one certificate on `gw` and services1 holding no acme config at all; when it does,
# this default is the thing to delete rather than to keep overriding, because a default
# that points at a cert which no longer exists is a broken vhost on two boxes.
#
#   mkIntVhost = import common/nginx/int-vhost.nix { inherit lib; };
#
#   services.nginx.virtualHosts."foo.int.leighhack.org" = mkIntVhost {
#     proxyPass = "http://127.0.0.1:8090";
#     websockets = true;              # proxyWebsockets
#     bodySize = "2048M";             # client_max_body_size
#     timeouts = "3600s";             # proxy_{connect,send,read}_timeout + send_timeout
#     proxyBuffering = false;
#     acl = "";                       # "" disables the LAN ACL (public vhost)
#     extraConfig = "...";            # anything else, appended to location "/"
#   };
#
# The defaults are deliberately the LAN-restricted ones: a vhost that is
# reachable from outside has to opt out with `acl = ""`.
{lib}: {
  proxyPass,
  acl ? (import ../config.nix).LOCAL_NETWORK,
  websockets ? false,
  bodySize ? null,
  timeouts ? null,
  proxyBuffering ? null,
  proxyHeaderBuffers ? false,
  extraConfig ? "",
  serverAliases ? [],
  forceSSL ? true,
  useACMEHost ? "leighhack.org",
}: {
  inherit useACMEHost forceSSL serverAliases;

  locations."/" = {
    inherit proxyPass;
    recommendedProxySettings = true;
    proxyWebsockets = websockets;

    extraConfig =
      acl
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
        # buffer ("upstream sent too big header").  busy >= buffer_size and
        # busy < total buffers - one buffer.
        proxy_buffer_size       32k;
        proxy_buffers           16 8k;
        proxy_busy_buffers_size 32k;
      ''
      + extraConfig;
  };
}
