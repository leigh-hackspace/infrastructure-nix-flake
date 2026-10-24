# services1 as an **origin**: nginx with no certificate, no gate, no public ACL.
#
# `docs/router-front-door-migration.md` (the origin half). Everything that terminates
# TLS, authenticates or decides which network may have a name now lives on `gw`
# (`machines/gw/http.nix` + `common/front-door.nix`); this box keeps the vhosts whose
# *applications* are app-shaped (a `/srv` root, an auth rewrite, a filesystem mount)
# and answers them on plain HTTP for `10.3.1.1` only. Every other name the network has
# is either a `upstream = "origin"` row in the front-door table that lands here, or a
# direct row the door proxies to the box running the app — and in neither case does
# this box need a certificate.
#
# What is gone from this file and why it cannot come back by accident:
#
#   * `security.acme` and `useACMEHost`. The network has **one** issuer
#     (`machines/gw/http.nix`), which is also what closes the `_acme-challenge.int`
#     two-writer window in the migration doc's findings. A `useACMEHost` here would be
#     a vhost pointing at a cert directory this box no longer maintains.
#   * `systemd.services.nginx-sso`. The gate is on `gw`; its auth endpoint can finally
#     be loopback there because the only nginx that calls it is the one on the same
#     box (finding: the gate answered `*:8082` here).
#   * `users.groups.secrets.members = ["nginx"]`. Nothing in this file reads
#     `/run/secrets` any more; the two injected headers (`status_dashboard_lan_token`
#     on `services1-status`) are build-time store paths, and group `secrets` is read
#     access to every runtime secret on the box — not a grant to hand out for a vhost
#     that no longer needs it.
#   * the direct-row vhosts (`bar`, `alexandria`, `network-info`, `login`, `webhooks`,
#     `web-test`, `retro.leighhack.org`, `firewall`, `truenas`, `monster`,
#     `access-api`, `api`, `id`, `jenkins`, `user-tweaker`, `voron`, `immich`). The
#     door proxies those straight to the box that runs the app; keeping a second copy
#     here would be two configs that can disagree about one name.
#
# The guard that keeps this file honest is `./int-dns.nix`: the vhosts here must be
# exactly the `upstream = "origin"` rows in `common/front-door.nix`. A name moved to a
# direct row, or an origin added to the table, fails the evaluation until both halves
# move together (AGENTS.md rule 24).
{lib, ...}: let
  CONFIG = import ../../common/config.nix;
  mkOriginVhost = import ../../common/nginx/origin-vhost.nix {inherit lib;};
in {
  services.nginx = {
    enable = true;

    recommendedOptimisation = true;
    recommendedGzipSettings = true;

    # ~15 origin vhosts here; the longest name is nowhere near the 64-byte default but
    # the front door sets this too and the two boxes should not disagree about a
    # config that nginx only complains about at startup.
    appendHttpConfig = ''
      server_names_hash_bucket_size 128;
    '';

    virtualHosts = {
      # Home Assistant. Two locations, one of them an `/auth/authorize` rewrite with an
      # injected meta-refresh for the authentik round-trip — which is why this is an
      # origin row rather than a one-line proxy at the door (the door only needs the
      # name). The client's Host and the `X-WEBAUTH-USER` the gate set both arrive
      # through the door untouched.
      "ha.int.leighhack.org" = {
        extraConfig = CONFIG.ROUTER_ONLY;

        locations."/" = {
          proxyPass = "http://10.3.1.30:8123";
          recommendedProxySettings = true;
          proxyWebsockets = true;
        };

        # Authentik support
        locations."/auth/authorize" = {
          proxyPass = "http://10.3.1.30:8123";
          recommendedProxySettings = true;
          proxyWebsockets = true;
          extraConfig = ''
            sub_filter_once off;
            sub_filter_types text/html;
            sub_filter '<head>' '<head><script>setTimeout(function(){window.location.href="/auth/oidc/redirect";},1000);</script>';
            proxy_set_header Accept-Encoding "";
          '';
        };
      };

      # A static file root on this box (`/srv/ai-resources/resources/robot`). The root
      # stays where the files are; the door only needs the name.
      "robot.ai.leighhack.org" = {
        extraConfig = CONFIG.ROUTER_ONLY;
        root = "/srv/ai-resources/resources/robot";
      };

      # The NAS download share. `root = /sas-16tb/ds-downloads/Retro` exists because
      # services1 mounts the NAS and the router deliberately mounts nothing; this is
      # the row that makes the filesystem root stay where the mount is.
      "retro.int.leighhack.org" = {
        extraConfig = CONFIG.ROUTER_ONLY;
        root = "/sas-16tb/ds-downloads/Retro";
        locations."/".extraConfig = ''
          autoindex on;
        '';
      };
    };
  };
}
