# Runtime secret files for services1, provisioned by sops-nix from
# secrets/secrets.yaml into /run/secrets. The *_FILE paths in config.nix
# point there.
#
# Secrets that are embedded into generated configs at build time (slack_url,
# unifi_db_password, mattermost_authentik_secret, nginx_sso_client_secret,
# nginx_sso_auth, pg_pass, status_dashboard_lan_token) are read via
# config.sopsSecretText instead and are deliberately not declared here.
{
  sops.secrets = {
    # acme, door-entry-management-system, gocardless-authentik-sync
    env_file = {
      group = "secrets";
      mode = "0440";
    };

    http_basic_auth = {
      group = "secrets";
      mode = "0440";
    };

    # WireGuard key for tailscale (networking.nix)
    wg_key = {
      group = "secrets";
      mode = "0440";
    };

    headplane_pre_authkey = {
      group = "secrets";
      mode = "0440";
    };

    headplane_api_key = {
      group = "secrets";
      mode = "0440";
    };

    headplane_client_secret = {
      group = "secrets";
      mode = "0440";
    };

    outline_client_secret = {
      group = "secrets";
      mode = "0440";
    };

    # OIDC client secret for Grafana via authentik (services/monitoring.nix)
    grafana_oidc_client_secret = {
      group = "secrets";
      mode = "0440";
    };

    # Synapse authentik config (services/matrix.nix)
    synapse_authentik = {
      group = "secrets";
      mode = "0440";
    };

    # Borg/ssh backup key (services/backup.nix); keep private.
    backup_key = {
      mode = "0400";
    };
  };
}
