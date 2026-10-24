# headscale + headplane: the network's own tailnet control server and its admin UI.
#
# Moved from services1 (`machines/services1/services/headscale.nix`) in phase 4 of
# `docs/router-front-door-migration.md`. The reason it belongs here is the same one
# that moved nginx: the router already owns the network's names (dnsmasq), its routes
# (nftables, the AAISP tunnel) and its addresses (DHCP), and headscale owns the tailnet's
# *node* addresses and ACLs — so keeping the control server on a box the tailnet reaches
# *through* the router was the arrangement that needed the router to be a client of a
# service behind it.
#
# What moved with it, and what did not:
#
#   * **`/var/lib/headscale` was copied, not recreated.** `noise_private.key` is the
#     control server's node identity; a fresh one unenrols every device. The migration
#     order is services1 stops → the whole directory (WAL included) is copied → this
#     box starts. `machines/services1/README.md` and the migration doc have the
#     procedure.
#   * **the vhosts did not move here.** `tailscale.leighhack.org` and
#     `headplane.leighhack.org` are rows in `common/front-door.nix`, rendered by
#     `machines/gw/http.nix`; this module only runs the two daemons they proxy to
#     (8085/8086, loopback). The `/admin` split is in the table row's `locations`.
#   * **the tailnet client stayed in `./uplink.nix`.** This box was already a tailnet
#     node pointed at `https://tailscale.leighhack.org`; that URL now resolves to this
#     box's own nginx instead of services1's, which is the only thing that changed.
#
# The DNS half is *not* duplicated here. dnsmasq (./dns.nix) is authoritative for
# `int.leighhack.org` and headscale's own resolver is only for `ts.leighhack.org` +
# magic DNS; the search domain points at 10.3.1.1, which is this same box.
{
  pkgs,
  lib,
  config,
  ...
}: let
  CONFIG = import ../../common/config.nix;
in {
  # headscale reads its client secret and headplane its API key from /run/secrets;
  # both are root:secrets 0440.
  users.groups.secrets.members = ["headscale"];

  services.headscale = {
    enable = true;
    # Loopback, like the gate's listener: every client of this HTTP server is `gw`'s
    # nginx, and the DERP/STUN socket that does need a public listener is a separate
    # bind (`stun_listen_addr` below, DNAT'd by ./firewall.nix). It was `0.0.0.0` on
    # services1, where nftables was the only thing keeping it off the LAN.
    address = "127.0.0.1";
    port = 8085;

    settings = {
      server_url = "https://${CONFIG.HEADSCALE_DOMAIN}";

      log.level = "debug";

      dns = {
        override_local_dns = false;
        base_domain = "ts.leighhack.org";
        magic_dns = true;
        search_domains = ["int.leighhack.org"];
        nameservers = {
          global = [
            "9.9.9.9"
            "8.8.8.8"
            "1.1.1.1"
          ];
          split = {
            "int.leighhack.org" = ["10.3.1.1"];
          };
        };
      };

      # The tailnet's allocation pool, read from the one copy of the fact
      # (common/config.nix) — this is the string that decides what address a node
      # gets, and the router's forward rules, every `*.int` vhost's nginx ACL and
      # `pg_hba` all trust the same block.
      ip_prefixes = [
        CONFIG.TAILNET_V4
        CONFIG.TAILNET_V6
      ];

      derp = {
        server = {
          enabled = true;
          stun_listen_addr = "0.0.0.0:3479"; # Unify uses default STUN port of 3478
          # The public addresses this box answers inbound on. The IPv4 /29 is routed to
          # the AAISP line and `machines/gw/firewall.nix` DNATs 3478 to this box's 3479;
          # the IPv6 address is this box's own on VLAN 225 (it used to be services1's
          # `.d::1020` alias, which is exactly the kind of fact that had to move with
          # the service).
          ipv4 = "81.187.195.17";
          ipv6 = "2001:8b0:1d14:225::1";
        };
      };

      oidc = {
        enable = true;
        issuer = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/headplane/";
        client_id = "uvJnUrNJuaUXw3K8cJhT9fX7IMQ9amcBY5vTYJTQ";
        client_secret_path = CONFIG.HEADPLANE_CLIENT_SECRET_FILE;
        allowed_groups = ["Members"];
      };
    };
  };

  # The state directory is copied, not recreated (phase 4 of the migration doc):
  # `noise_private.key` and the SQLite db (WAL included) are the tailnet's enrolment.
  # The copy comes from another box where the `headscale` UID is unrelated, so the files
  # need re-owning. `tmpfiles` and not `preStart`: ExecStartPre runs as the unit's
  # `User=headscale`, and chown needs root — measured on the first deploy, where the
  # preStart failed with `Read-only file system`/`Permission denied` and took the unit
  # into a restart loop. `Z` is recursive, `d` creates the directory if the copy has not
  # happened yet, and both run at activation as root.
  systemd.tmpfiles.rules = [
    "d /var/lib/headscale 0750 headscale headscale -"
    "Z /var/lib/headscale 0750 headscale headscale -"
    "d /var/lib/headplane 0750 headscale headscale -"
    "Z /var/lib/headplane 0750 headscale headscale -"
  ];

  services.headplane = let
    format = pkgs.formats.yaml {};

    # A workaround to generate a valid Headscale config accepted by Headplane when
    # `config_strict == true`.
    settings = lib.recursiveUpdate config.services.headscale.settings {
      tls_cert_path = "/dev/null";
      tls_key_path = "/dev/null";
      policy.path = "/dev/null";
    };

    headscaleConfig = format.generate "headscale.yml" settings;
  in {
    enable = true;
    settings = {
      server = {
        host = "127.0.0.1";
        port = 8086;
        cookie_secret_path = pkgs.writeText "cookie_secret_path" "12345678123456781234567812345678";
        base_url = "https://${CONFIG.HEADSCALE_DOMAIN}/admin";
      };
      headscale = {
        url = "https://${CONFIG.HEADSCALE_DOMAIN}";
        config_path = "${headscaleConfig}";
        api_key_path = CONFIG.HEADPLANE_API_KEY_FILE;
      };
      # The agent persists its own tailnet identity under `work_dir`
      # (/var/lib/headplane/agent) and reauthenticates, so it needs no pre-auth key.
      integration.agent = {
        enabled = true;
      };
      oidc = {
        issuer = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/headplane/";
        client_id = "uvJnUrNJuaUXw3K8cJhT9fX7IMQ9amcBY5vTYJTQ";
        client_secret_path = CONFIG.HEADPLANE_CLIENT_SECRET_FILE;
        # Only support login through Authentik (go straight to login)
        disable_api_key_login = true;
        token_endpoint_auth_method = "client_secret_post";
      };
    };
  };
}
