# Helper for the LAN-only "*.int.leighhack.org" virtual hosts — the internal
# counterpart of nginx-sso-helper.nix (which is for public, SSO-protected
# apps). Every one of these is the same shape: the wildcard ACME cert, forceSSL,
# and one reverse-proxied location with the recommended proxy settings plus the
# LOCAL_NETWORK ACL.
#
#   mkIntVhost = import ./lib/nginx-int-vhost-helper.nix { inherit lib; };
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
{ lib }:
{
  proxyPass,
  acl ? (import ../config.nix).LOCAL_NETWORK,
  websockets ? false,
  bodySize ? null,
  timeouts ? null,
  proxyBuffering ? null,
  proxyHeaderBuffers ? false,
  extraConfig ? "",
  serverAliases ? [ ],
  forceSSL ? true,
  useACMEHost ? "leighhack.org",
}:
{
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
        proxy_buffering ${if proxyBuffering then "on" else "off"};
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
