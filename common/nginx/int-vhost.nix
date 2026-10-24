# HELPER — function of {lib} returning the TLS vhost builder used by the front-door renderer.
#
# The front door's internal/public name builder: the ACME certificate, `forceSSL`, and
# one reverse-proxied location with the recommended proxy settings plus an ACL. It is
# **not** the origin builder — `origin-vhost.nix` is that, and the difference is the
# point: an origin terminates nothing.
#
# The one caller is `common/nginx/front-door.nix`, which passes the door's certificate
# (`useACMEHost = certName`) and its per-row ACL. `useACMEHost` is therefore **required**:
# it used to default to services1's wildcard, and services1 has no certificate any more
# (`docs/router-front-door-migration.md`). A caller that forgets it is a vhost pointing
# at a cert directory nothing renews, and there is no longer a sensible default to give it.
#
#   mkIntVhost = import common/nginx/int-vhost.nix { inherit lib; };
#
#   services.nginx.virtualHosts."foo.int.leighhack.org" = mkIntVhost {
#     useACMEHost = "leighhack.org";
#     proxyPass = "http://127.0.0.1:8090";
#     websockets = true;              # proxyWebsockets
#     bodySize = "2048M";             # client_max_body_size
#     timeouts = "3600s";             # proxy_{connect,send,read}_timeout + send_timeout
#     proxyBuffering = false;
#     acl = "";                       # "" disables the ACL (public vhost)
#     extraConfig = "...";            # anything else, appended to location "/"
#   };
#
# The default ACL is the LAN-restricted one: a vhost reachable from outside has to opt
# out with `acl = ""`.
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
  useACMEHost,
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
