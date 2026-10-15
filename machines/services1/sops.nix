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

    # Switch login + password and the switch-gui OIDC client pair, as a
    # KEY=value file consumed by systemd's EnvironmentFile (see
    # services/switch-gui.nix, which lists the keys).  Read by PID 1 only: the
    # service runs as a dynamic user and never opens it.
    switch_gui_env_file = {
      group = "secrets";
      mode = "0440";
    };

    # Synapse authentik config (services/matrix.nix)
    synapse_authentik = {
      group = "secrets";
      mode = "0440";
    };

    # The authentik container stack's environment (services/authentik.nix):
    # POSTGRES_PASSWORD, AUTHENTIK_SECRET_KEY, AUTHENTIK_POSTGRESQL__PASSWORD
    # and the SMTP keys, as unquoted KEY=value lines.  Read by PID 1 only —
    # podman opens it with `--env-file` when it starts each container, so no
    # process inside a container can list it.  Values must not be quoted and
    # must not contain `$`: podman does not strip quotes or expand in env files
    # (the opposite of systemd's EnvironmentFile).
    authentik_env_file = {
      group = "secrets";
      mode = "0440";
    };

    # Borg/ssh backup key (services/backup.nix); keep private.
    backup_key = {
      mode = "0400";
    };
  };
}
