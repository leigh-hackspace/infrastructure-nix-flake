# Network-wide facts shared by every machine: runtime secret paths, the domains
# the SSO/apps redirect to, the nginx ACL for the internal vhosts, and the
# Postgres pg_hba table.
#
# This used to live in machines/services1/config.nix, which meant a second box
# could not use any of it — the ACL and the pg_hba rules describe the network, not
# one machine.  Machine addresses are in ./hosts.nix.
#
# Runtime secret files, provisioned by sops-nix from secrets/secrets.yaml
# (see sops.nix). Secrets embedded into configs at build time are read via
# config.sopsSecretText instead (see common/sops.nix).
{
  ENV_FILE = "/run/secrets/env_file";
  HTTP_BASIC_AUTH_FILE = "/run/secrets/http_basic_auth";
  WIREGUARD_KEY_FILE = "/run/secrets/wg_key";

  AUTHENTIK_DOMAIN = "id.leighhack.org";

  # authentik on services1 (machines/services1/services/authentik.nix).  The
  # container env files are passed to podman with `--env-file`, which keeps
  # quotes and `$` literal, so these files are unquoted `KEY=value` lines —
  # unlike the shared ENV_FILE above, whose values are quoted for systemd.
  AUTHENTIK_ENV_FILE = "/run/secrets/authentik_env_file";
  AUTHENTIK_RADIUS_OUTPOST_ENV_FILE = "/run/secrets/authentik_radius_outpost_env";
  AUTHENTIK_LDAP_OUTPOST_ENV_FILE = "/run/secrets/authentik_ldap_outpost_env";

  HEADSCALE_DOMAIN = "tailscale.leighhack.org";
  HEADPLANE_PRE_AUTHKEY_FILE = "/run/secrets/headplane_pre_authkey";
  HEADPLANE_API_KEY_FILE = "/run/secrets/headplane_api_key";
  HEADPLANE_CLIENT_SECRET_FILE = "/run/secrets/headplane_client_secret";

  OUTLINE_CLIENT_SECRET_FILE = "/run/secrets/outline_client_secret";
  GRAFANA_OIDC_CLIENT_SECRET_FILE = "/run/secrets/grafana_oidc_client_secret";

  BACKUP_KEY_FILE = "/run/secrets/backup_key";

  # NGINX Firewall for "*.int.leighhack.org"
  # Allow only LAN access for internal services
  LOCAL_NETWORK = ''
    allow 10.3.0.0/16;              # Hackspace Internal
    allow 192.168.2.0/24;           # CS_VLAN Internal
    allow 100.64.0.0/16;            # Tailscale Tailnet (IPv4)
    allow fd7a:115c:a1e0::0/48;     # Tailscale Tailnet (IPv6)
    allow 2001:8b0:1d14::0/48;      # Hackspace AAISP range
    allow fd99:dead:beef::0/48;     # Hackspace IPv6 LAN range

    # Chris stuff...
    allow 192.168.49.0/24;          # Chris Home Internal
    allow 51.148.168.145/32;        # Chris Zen (IPv4)
    allow 2a02:8010:6680::0/48;     # Chris Zen (IPv6)
    allow 2a0a:ef40:154a::0/48;     # Chris 2
    allow 2001:4860:7::0/48;        # Society1

    allow 217.155.231.1/32;         # Kian

    deny all;
  '';

  PG_AUTH = ''
    #type   database  DBuser  auth-method
    local   all       all     trust
    host    sameuser  all     127.0.0.1/32            scram-sha-256
    host    sameuser  all     ::1/128                 scram-sha-256
    host    sameuser  all     10.3.0.0/16             scram-sha-256
    host    sameuser  all     10.88.0.0/16            scram-sha-256
    host    sameuser  all     100.64.0.0/16           scram-sha-256
    host    sameuser  all     fd7a:115c:a1e0::0/48    scram-sha-256
    host    sameuser  all     10.47.0.0/16            scram-sha-256
    host    sameuser  all     192.168.49.0/24         scram-sha-256
    host    sameuser  all     2001:8b0:1d14::0/48     scram-sha-256
    host    sameuser  all     2a02:8010:6680::0/48    scram-sha-256
    host    sameuser  all     2a00:23c8:b0ac::0/48    scram-sha-256
    host    sameuser  all     2001:4860:7::0/48       scram-sha-256
  '';

  # The postgres password (pg_pass) and the status-dashboard LAN restart
  # token (status_dashboard_lan_token) are sops secrets: read them via
  # config.sopsSecretText, never hard-code them here.
}
