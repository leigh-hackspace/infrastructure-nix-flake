# The vhost for the Strata engine that runs on services2 (10.3.1.21:8080).
#
# services1 is the TLS terminator for the whole network — the wildcard ACME cert
# and nginx-sso live here (see ../http.nix) — so the vhost stays here even though
# the engine does not.  Same arrangement as the observability apps in
# ./monitoring.nix and the status dashboards in ./status.nix: the app on the
# other box, the vhost here, proxied to the fixed address in common/hosts.nix.
#
# Two vhosts, one upstream, two gates.  `strata.int.leighhack.org` keeps the
# LOCAL_NETWORK ACL like every other *.int vhost and stays keyless for its
# clients: nginx presents the engine's api_key on their behalf (see
# strataApiKey below), which is what keeps the dashboard at / working from a
# browser.  `strata.leighhack.org` is public (acl = "") and safe only because the
# engine carries that same api_key (machines/services2/strata.nix) and a public
# client has to bring its own: the gate is the key, not the network.  Take the
# key out of the engine and the public name answers 403 for every request —
# upstream's DNS-rebinding Host check runs only when there is no key, and the
# public name is deliberately not in the engine's allowed_hosts.  Never make it
# public by adding it there: that is an unauthenticated inference engine on a
# public URL, minutes of GPU time per request.
#
# The public name works even though services2 has no public IP: services1 is the
# TLS terminator for the network, so nginx here answers on its own public IPs
# (81.187.195.17 / 2001:8b0:1d14:225::20, the DO record `nginx`) and proxies over
# the LAN to 10.3.1.21:8080.  The record is hand-made in the DigitalOcean zone:
# nothing in this repo writes public DNS any more (`*.int` is answered by the
# router from common/int-dns.nix, and the zone carries one `*.int` CNAME to
# nginx.int so off-LAN clients resolve the same way), so adding this one was a
# web-UI job and deleting it is too:
#
#   strata.leighhack.org  CNAME -> nginx.leighhack.org  (TTL 60, like the rest)
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
  # Same upstream and same limits for both names: a chat completion holds the
  # connection open for as long as the model thinks, and a prompt of that context
  # is far over nginx's 1 MiB default body limit (see the note above).
  strataProxy = {
    proxyPass = "http://${HOSTS.services2}:8080";
    bodySize = "1024M";
    timeouts = "3600s";
    proxyBuffering = false;
  };

  # The engine's api_key — the same sops name services2 feeds the engine in
  # machines/services2/strata.nix — handed to the engine by nginx instead of by
  # the client, for the LAN name only.
  #
  # Why it has to be nginx: the key gates every /v1/* endpoint
  # (serve/server.py: _authorized), and the dashboard the engine serves at / is a
  # page whose every call is a /v1/* call.  With the key set and not injected,
  # https://strata.int.leighhack.org/ loads and then 401s on everything, and a
  # browser on the LAN cannot hold a secret anyway — the gate on that name is the
  # LOCAL_NETWORK ACL.  The public vhost deliberately does not do this: nginx
  # forwards the client's own Authorization untouched
  # (recommendedProxySettings never sets it), so a stranger still needs a key.
  #
  # Build-time readFile puts the key in the world-readable nix store like every
  # other build-time secret here (common/sops.nix); it protects nothing against a
  # local reader, who could equally read the engine's own config on services2.
  # Rotating it means changing the sops value and re-deploying BOTH machines.
  strataApiKey = builtins.readFile (config.sopsSecretText "strata_api_key");
in {
  # proxy_set_header replaces whatever the client sent in that field, so a LAN
  # client's own Authorization is overwritten rather than duplicated — the engine
  # sees exactly one key, the one nginx holds.
  services.nginx.virtualHosts."strata.int.leighhack.org" = mkIntVhost (strataProxy
    // {
      extraConfig = ''
        proxy_set_header Authorization "Bearer ${strataApiKey}";
      '';
    });

  # Public: acl = "" drops the LOCAL_NETWORK allow-map (common/nginx/int-vhost.nix
  # keeps the LAN-restricted default on purpose — a vhost reachable from outside
  # has to opt out).
  services.nginx.virtualHosts."strata.leighhack.org" =
    mkIntVhost (strataProxy // {acl = "";});
}
