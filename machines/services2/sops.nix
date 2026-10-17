# Runtime secret files for services2, provisioned by sops-nix from
# secrets/secrets.yaml into /run/secrets. The *_FILE paths in common/config.nix
# point there.
#
# Only the secrets this box's services actually read at runtime are declared; the
# ones embedded into configs at build time (slack_url, grafana_admin_password,
# grafana_secret_key, status_dashboard_lan_token) are read via
# config.sopsSecretText instead and are deliberately not listed here.
#
# There are none now.  The only runtime secret this box used was Grafana's
# authentik OIDC client secret, and Grafana moved to services1 on 2026-10-17,
# which declares it (machines/services1/sops.nix) — the authentik provider (pk 3)
# is one client regardless of which box Grafana runs on, so there is still one
# secret, it just has one reader now.  What is left here — the status dashboard
# and strata — reads its secrets via config.sopsSecretText, at build time.
{
  sops.secrets = {};
}
