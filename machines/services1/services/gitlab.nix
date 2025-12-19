{
  ...
}:

let
  configToGitlab = import ../lib/config-to-gitlab.nix;
in
{
  users.users.gitlab = {
    uid = 998;
    group = "gitlab";
    isSystemUser = true;
  };

  users.users.gitlab-pg = {
    uid = 996;
    group = "gitlab-pg";
    isSystemUser = true;
  };

  users.users.gitlab-redis = {
    uid = 997;
    group = "gitlab";
    isSystemUser = true;
  };

  users.groups.gitlab = {
    gid = 998;
  };

  users.groups.gitlab-pg = {
    gid = 996;
  };

  users.groups.gitlab-workhorse = {
    gid = 999;
  };

  # journalctl -u podman-gitlab -f
  virtualisation.oci-containers.containers = {
    gitlab = {
      hostname = "gitlab";
      image = "gitlab/gitlab-ce:18.6.2-ce.0";
      autoStart = true;
      ports = [
        "8580:80"
        "8543:443"
        "8522:22"
      ];
      volumes = [
        "/srv/gitlab/config:/etc/gitlab"
        "/srv/gitlab/logs:/var/log/gitlab"
        "/srv/gitlab/data:/var/opt/gitlab"
      ];
      environment = {
        GITLAB_OMNIBUS_CONFIG = (
          configToGitlab {
            external_url = "https://gitlab.leighhack.org";
            gitlab_rails = {
              lfs_enabled = true;
              gitlab_shell_ssh_port = 8522; # GitLab needs to know outside ports so it can correctly generate download links
              gitlab_email_from = "no-reply@gitlab.leighhack.org";
              omniauth_enabled = true;
              omniauth_allow_single_sign_on = [ "saml" ];
              omniauth_sync_email_from_provider = "saml";
              omniauth_sync_profile_from_provider = [ "saml" ];
              omniauth_sync_profile_attributes = [ "email" ];
              omniauth_auto_sign_in_with_provider = "saml";
              omniauth_block_auto_created_users = false;
              omniauth_auto_link_saml_user = true;
              omniauth_providers = [
                {
                  name = "saml";
                  args = {
                    assertion_consumer_service_url = "https://gitlab.leighhack.org/users/auth/saml/callback";
                    idp_cert_fingerprint = "d0:7a:a2:b4:8c:ba:28:01:57:a7:60:5e:99:6b:da:45:6e:b8:bc:b6";
                    idp_sso_target_url = "https://id.leighhack.org/application/saml/gitlab/sso/binding/redirect/";
                    issuer = "https://gitlab.leighhack.org";
                    name_identifier_format = "urn:oasis:names:tc:SAML:2.0:nameid-format:persistent";
                    attribute_statements = {
                      email = [ "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress" ];
                      first_name = [ "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/name" ];
                      nickname = [ "http://schemas.goauthentik.io/2021/02/saml/username" ];
                    };
                    label = "authentik";
                  };
                }
              ];
            };
          }
        );
        TZ = "Europe/London";
      };
      extraOptions = [
        "--shm-size=256m"
        "--privileged"
        "--no-healthcheck"
      ];
    };
  };

  services.nginx.virtualHosts = {
    "gitlab.leighhack.org" = {
      useACMEHost = "leighhack.org";
      forceSSL = true;

      locations."/" = {
        proxyPass = "https://localhost:8543";
        recommendedProxySettings = true;
      };
    };

    # Redirect gitlab.int.leighhack.org to gitlab.leighhack.org
    "gitlab.int.leighhack.org" = {
      forceSSL = true;
      useACMEHost = "leighhack.org";

      locations."/" = {
        return = "301 https://gitlab.leighhack.org$request_uri";
      };
    };
  };

  system.activationScripts.initGitlab = ''
    mkdir -p                /srv/gitlab/config
    mkdir -p                /srv/gitlab/data
    mkdir -p                /srv/gitlab/logs
    chown gitlab:gitlab     /srv/gitlab
    chmod u+rw,g+rw,o-rw    /srv/gitlab
  '';
}
