# Runtime secret files for services2, provisioned by sops-nix from
# secrets/secrets.yaml into /run/secrets. The *_FILE paths in common/config.nix
# point there.
#
# Only the secrets this box's services actually read at runtime are declared; the
# ones embedded into configs at build time (slack_url, grafana_admin_password,
# grafana_secret_key, status_dashboard_lan_token) are read via
# config.sopsSecretText instead and are deliberately not listed here.
{
  sops.secrets = {
    # Grafana's authentik OIDC client secret (monitoring.nix).
    #
    # The same secret as on services1: the authentik provider (pk 3) is one
    # client regardless of which box Grafana runs on, so there is one secret and
    # two readers. Client secrets are write-only in authentik — it can never be
    # read back, only re-generated.
    grafana_oidc_client_secret = {
      group = "secrets";
      mode = "0440";
    };
  };
}
