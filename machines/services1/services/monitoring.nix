# The vhosts for the observability apps: gatus, Prometheus and Grafana.
#
# services1 is the TLS terminator for this network — the wildcard ACME cert and
# nginx-sso live here (see ../http.nix) — so the vhosts have always lived here.
# Since 2026-10-17 the apps do too (./gatus.nix, ./observability.nix): services2
# keeps only strata, so these proxy to a loopback bind on this box rather than
# across the LAN.  The ports are the apps' own — 8999 gatus, 3000 Grafana,
# 9091 Prometheus — and none of them collide with anything else services1 runs
# (checked against the rendered units, not against this file).
#
# This file also carries the drift guard that used to sit inside gatus.nix
# (docs/repo-audit-2026-10-07.md §3.7).  It belongs here rather than with gatus,
# because the thing it checks is this box's own vhost list: every https
# `*.int.leighhack.org` endpoint in common/monitoring-endpoints.nix has to be a
# name this nginx actually serves, otherwise a renamed or removed app leaves gatus
# alerting on a name that no longer exists, forever.  It fires on every eval, so
# `just check` catches it before a deploy.
{
  config,
  lib,
  ...
}: let
  mkOriginVhost = import ../../../common/nginx/origin-vhost.nix {inherit lib;};
  INT = import ../../../common/int-dns.nix;
  CONFIG = import ../../../common/config.nix;
  ENDPOINTS = import ../../../common/monitoring-endpoints.nix {inherit lib;};

  # https *.int endpoints that the **front door** does not answer. The guard used to
  # compare against this box's vhosts, which was right while services1 was the front
  # door. After the move the names live in `common/int-dns.nix` (the router renders
  # them into dnsmasq) and this box only keeps the origin vhosts for a handful of
  # them, so reading the vhost list here would report every direct row as unbacked.
  # The table is the fact that actually decides whether the name resolves; the
  # origin-vs-direct split is `machines/services1/int-dns.nix`'s guard.
  declaredIntNames = map (n: "${n}.${CONFIG.LOCAL_DOMAIN}") INT.names;
  unbackedIntEndpoints = let
    hosts =
      builtins.filter (h: h != null && lib.hasSuffix ".int.leighhack.org" h)
      (map (e: ENDPOINTS.httpsHost (e.url or "")) ENDPOINTS.list);
  in
    lib.subtractLists declaredIntNames hosts;
in {
  assertions = [
    {
      assertion = unbackedIntEndpoints == [];
      message = "gatus: these https *.int.leighhack.org endpoints are not answered at the front door (common/int-dns.nix): ${builtins.toJSON unbackedIntEndpoints}.  The app was renamed or removed and its gatus endpoint was left behind — remove or retarget it in common/monitoring-endpoints.nix.";
    }
  ];

  services.nginx.virtualHosts = {
    # kuma's status page was LAN/tailnet only; gatus stays the same.
    "gatus.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:8999";
    };

    # Replaces the old grafana.int vhost in http.nix which proxied to the
    # (now superseded) Grafana on 10.3.1.30.
    "grafana.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:3000";
      websockets = true;
    };

    "prometheus.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://127.0.0.1:9091";
    };
  };
}
