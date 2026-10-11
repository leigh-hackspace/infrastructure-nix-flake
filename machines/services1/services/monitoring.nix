# The vhosts for the observability apps, which run on services2.
#
# services1 is the TLS terminator for this network — the wildcard ACME cert and
# nginx-sso live here (see ../http.nix) — so the vhosts stay on services1 even
# though gatus, Prometheus and Grafana do not.  Same arrangement as the status
# dashboards: the app on the other box, the vhost here.
#
# This file also carries the drift guard that used to sit inside gatus.nix
# (docs/repo-audit-2026-10-07.md §3.7).  It belongs on services1 rather than with
# gatus, because the thing it checks is services1's own vhost list: every https
# `*.int.leighhack.org` endpoint in common/monitoring-endpoints.nix has to be a
# name this nginx actually serves, otherwise a renamed or removed app leaves gatus
# alerting on a name that no longer exists, forever.  It fires on every eval, so
# `just check` catches it before a deploy.
{
  config,
  lib,
  ...
}: let
  HOSTS = import ../../../common/hosts.nix;
  mkIntVhost = import ../../../common/nginx/int-vhost.nix {inherit lib;};
  intVhostNames = import ../../../common/nginx/int-vhost-names.nix {inherit lib;};
  ENDPOINTS = import ../../../common/monitoring-endpoints.nix {inherit lib;};

  # https *.int endpoints that are not names this nginx serves; the assertion at
  # the bottom requires this to be empty.
  unbackedIntEndpoints = let
    hosts =
      builtins.filter (h: h != null && lib.hasSuffix ".int.leighhack.org" h)
      (map (e: ENDPOINTS.httpsHost (e.url or "")) ENDPOINTS.list);
  in
    lib.subtractLists (intVhostNames config.services.nginx.virtualHosts) hosts;
in {
  assertions = [
    {
      assertion = unbackedIntEndpoints == [];
      message = "gatus: these https *.int.leighhack.org endpoints are not vhosts this nginx serves: ${builtins.toJSON unbackedIntEndpoints}.  The app was renamed or removed and its gatus endpoint was left behind — remove or retarget it in common/monitoring-endpoints.nix.";
    }
  ];

  services.nginx.virtualHosts = {
    # kuma's status page was LAN/tailnet only; gatus stays the same.
    "gatus.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://${HOSTS.services2}:8999";
    };

    # Replaces the old grafana.int vhost in http.nix which proxied to the
    # (now superseded) Grafana on 10.3.1.30.
    "grafana.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://${HOSTS.services2}:3000";
      websockets = true;
    };

    "prometheus.int.leighhack.org" = mkIntVhost {
      proxyPass = "http://${HOSTS.services2}:9091";
    };
  };
}
