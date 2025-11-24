{
  # May need to change if the repo is cloned to a different home folder
  ENV_FILE                          = "/var/lib/secrets/.env";
  HTTP_BASIC_AUTH_FILE              = "/var/lib/secrets/http_basic_auth";
  WIREGUARD_KEY_FILE                = "/var/lib/secrets/wg.key";
  SLACK_URL_FILE                    = "/var/lib/secrets/slack_url.txt";

  AUTHENTIK_DOMAIN                  = "id.leighhack.org";

  MATTERMOST_AUTHENTIK_SECRET_FILE  = "/var/lib/secrets/mattermost_authentik_secret.key";

  HEADSCALE_DOMAIN                  = "tailscale.leighhack.org";
  HEADPLANE_PRE_AUTHKEY_FILE        = "/var/lib/secrets/headplane_pre_authkey.key";
  HEADPLANE_API_KEY_FILE            = "/var/lib/secrets/headplane_api_key.key";
  HEADPLANE_CLIENT_SECRET_FILE      = "/var/lib/secrets/headplane_client_secret.key";

  OUTLINE_CLIENT_SECRET_FILE        = "/var/lib/secrets/outline_client_secret.key";

  NGINX_SSO_CLIENT_SECRET_FILE      = "/var/lib/secrets/nginx_sso_client_secret.key";
  NGINX_SSO_AUTH_KEY_FILE           = "/var/lib/secrets/nginx_sso_auth.key";

  # Creating the `http_basic_auth` file
  # nix-shell --packages apacheHttpd --run 'htpasswd -B -c /var/lib/secrets/http_basic_auth leighhack'

  # NGINX Firewall for "*.int.leighhack.org"
  # Allow only LAN access for internal services
  LOCAL_NETWORK = ''
    allow 10.3.0.0/16;              # Hackspace Internal
    allow 192.168.2.0/24;           # CS_VLAN Internal
    allow 100.64.0.0/16;            # Tailscale Tailnet (IPv4)
    allow fd7a:115c:a1e0::0/48;     # Tailscale Tailnet (IPv6)
    allow 10.47.0.0/16;             # Chris VPN
    allow 2001:8b0:1d14::0/48;      # Hackspace AAISP range

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

  PG_PASS = "leighhack1234";
}
