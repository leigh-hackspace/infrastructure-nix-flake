# gitlab-config-example.nix
#
# The attrset services/gitlab.nix passes to lib/config-to-gitlab.nix, kept here so
# lib/check-config-to-gitlab.nix can assert over the real config while the
# gitlab module is disabled in services/default.nix.  Keep the two in step if
# the module is re-enabled.
{
  external_url = "https://gitlab.leighhack.org";
  # Each address is added to the NGINX config as 'set_real_ip_from <address>;'
  nginx = {
    real_ip_trusted_addresses = ["10.88.0.0/24"];
    real_ip_header = "X-Forwarded-For";
    real_ip_recursive = "on";
  };
  gitlab_rails = {
    lfs_enabled = true;
    gitlab_shell_ssh_port = 8522; # GitLab needs to know outside ports so it can correctly generate download links
    gitlab_email_from = "no-reply@gitlab.leighhack.org";
    omniauth_enabled = true;
    omniauth_allow_single_sign_on = ["saml"];
    omniauth_sync_email_from_provider = "saml";
    omniauth_sync_profile_from_provider = ["saml"];
    omniauth_sync_profile_attributes = ["email"];
    omniauth_auto_sign_in_with_provider = "saml";
    omniauth_block_auto_created_users = false;
    omniauth_auto_link_saml_user = true;
    omniauth_providers = [
      {
        name = "saml";
        label = "authentik";
        groups_attribute = "Groups";
        required_groups = ["Members"];
        admin_groups = ["Infra"];
        args = {
          assertion_consumer_service_url = "https://gitlab.leighhack.org/users/auth/saml/callback";
          idp_cert_fingerprint = "d0:7a:a2:b4:8c:ba:28:01:57:a7:60:5e:99:6b:da:45:6e:b8:bc:b6";
          idp_sso_target_url = "https://id.leighhack.org/application/saml/gitlab/sso/binding/redirect/";
          issuer = "https://gitlab.leighhack.org";
          name_identifier_format = "urn:oasis:names:tc:SAML:2.0:nameid-format:persistent";
          attribute_statements = {
            email = ["http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress"];
            first_name = ["http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name"];
            nickname = ["http://schemas.goauthentik.io/2021/02/saml/username"];
          };
        };
      }
    ];
  };
}
