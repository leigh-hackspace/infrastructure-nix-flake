# The vhost for the Strata engine that runs on services2 (10.3.1.21:8080).
#
# services1 is the TLS terminator for the whole network — the wildcard ACME cert
# and nginx-sso live here (see ../http.nix) — so the vhost stays here even though
# the engine does not.  Same arrangement as the observability apps in
# ./monitoring.nix and the status dashboards in ./status.nix: the app on the
# other box, the vhost here, proxied to the fixed address in common/hosts.nix.
#
# LAN-only, like every other *.int vhost: this is an unauthenticated inference
# endpoint (the engine has no api_key — /health says "api_key": false), so it
# stays inside the network.  There is no public strata.leighhack.org: a public
# name needs a hand-added record in the DigitalOcean zone (dns-sync only manages
# the *.int ones) and services2 has no public IP to terminate at anyway.
#
# ⚠️ The name here must ALSO appear in services2's allowed_hosts
# (machines/services2/strata.nix).  Upstream's server has a DNS-rebinding Host
# check (serve/server.py: host_allowed) that answers 403 for any Host it does
# not answer to, and nginx forwards the incoming one (recommendedProxySettings
# sets `Host $host`), so a vhost alone is not enough — every request comes back
# as `Host 'strata.int.leighhack.org' is not allowed (DNS rebinding protection)`.
# Reproduce without nginx:
#
#   curl -H 'Host: strata.int.leighhack.org' http://10.3.1.21:8080/v1/models   # 403
#   curl http://10.3.1.21:8080/v1/models                                      # 200 (an IP passes on trust)
#
# Timeouts and body size: a chat completion holds the connection open for as
# long as the model thinks — a long prefill at this engine's 131072 context is
# minutes, well over nginx's 60 s proxy_read_timeout — and a prompt of that
# context is far over nginx's 1 MiB default body limit.  proxy_buffering off so
# an SSE stream reaches the client token by token rather than at the end.
{
  config,
  lib,
  ...
}: let
  HOSTS = import ../../../common/hosts.nix;
  mkIntVhost = import ../../../common/nginx/int-vhost.nix {inherit lib;};
in {
  services.nginx.virtualHosts."strata.int.leighhack.org" = mkIntVhost {
    proxyPass = "http://${HOSTS.services2}:8080";
    bodySize = "1024M";
    timeouts = "3600s";
    proxyBuffering = false;
  };
}
