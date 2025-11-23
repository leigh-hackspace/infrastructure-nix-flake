{ config, pkgs, ... }:

{
  security.pki.certificates = [
    # leighhack.org (Root CA)
    ''
      -----BEGIN CERTIFICATE-----
      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
      fBbCwgkaci4r5pMqKjOy7DHKcoQRerA=
      -----END CERTIFICATE-----
    ''
  ];

  ## Test LDAP with:
  # ldapsearch -x -b "dc=ldap,dc=goauthentik,dc=io" -H ldaps://authentik.int.leighhack.org -D "cn=pgina,dc=ldap,dc=goauthentik,dc=io" -W

  ## Test TLS with:
  # openssl s_client -connect authentik.int.leighhack.org:636 -CAfile leighhack.org.crt

  services.sssd = {
    enable = true;
    config = ''
      [sssd]
      config_file_version = 2
      services = nss, pam
      domains = authentik
      debug_level = 8

      [nss]
      filter_users = root,nixbld
      filter_groups = root,wheel,nixbld
      debug_level = 8

      [pam]
      debug_level = 8

      [domain/authentik]
      id_provider = ldap
      auth_provider = ldap
      access_provider = permit

      # Connection
      ldap_uri = ldaps://authentik.int.leighhack.org
      ldap_search_base = dc=ldap,dc=goauthentik,dc=io

      # Bind credentials
      ldap_default_bind_dn = cn=pgina,ou=users,dc=ldap,dc=goauthentik,dc=io
      ldap_default_authtok = pgina

      # Search bases
      ldap_user_search_base = ou=users,dc=ldap,dc=goauthentik,dc=io
      ldap_group_search_base = ou=groups,dc=ldap,dc=goauthentik,dc=io

      # User attributes - matched to Authentik's schema
      ldap_user_object_class = user
      ldap_user_name = cn
      ldap_user_uid_number = uidNumber
      ldap_user_gid_number = gidNumber
      ldap_user_home_directory = homeDirectory
      ldap_user_shell = loginShell
      ldap_user_gecos = displayName

      # CRITICAL: Provide default shell since Authentik doesn't set loginShell
      default_shell = /run/current-system/sw/bin/bash
      fallback_homedir = /home/%u

      # Group attributes
      ldap_group_object_class = group
      ldap_group_name = cn
      ldap_group_gid_number = gidNumber   # Members = 16534, Infra = 21159, Public = 28164
      ldap_group_member = member

      # Schema
      ldap_schema = rfc2307bis
      ldap_id_mapping = false

      # Don't query rootDSE anonymously
      ldap_disable_rootdse = true

      # TLS settings
      ldap_id_use_start_tls = true
      ldap_tls_reqcert = demand
      ldap_tls_cacert = /etc/static/pki/tls/certs/ca-bundle.crt

      # Timeouts
      ldap_network_timeout = 5
      ldap_opt_timeout = 5

      # Caching
      # cache_credentials = true
      cache_credentials = false
      enumerate = false
    '';
  };

  security.pam.services = {
    sddm = {
      makeHomeDir = true;
    };

    login.makeHomeDir = true;
    sshd.makeHomeDir = true;
  };

  # Keep local users working
  users.mutableUsers = true;

  # Give sudo access to anyone in the Infra LDAP group
  security.sudo.extraRules = [
    {
      groups = [
        "infra"
        "Infra"
      ]; # SSSD will lowercase it
      commands = [
        {
          command = "ALL";
          options = [ "NOPASSWD" ];
        }
      ];
    }
  ];
}
