{
  # May need to change if the repo is cloned to a different home folder
  ENV_FILE              = "/var/lib/secrets/.env";
  HTTP_BASIC_AUTH_FILE  = "/var/lib/secrets/http_basic_auth";
  WIREGUARD_KEY_FILE    = "/var/lib/secrets/wg.key";

  # Creating the `http_basic_auth` file
  # nix-shell --packages apacheHttpd --run 'htpasswd -B -c /var/lib/secrets/http_basic_auth leighhack'

  # NGINX Firewall for "*.int.leighhack.org"
  # Allow only LAN access for internal services
  LOCAL_NETWORK = ''
    allow 10.3.0.0/16;
    allow 10.47.0.0/16;
    allow 51.148.168.145/32;    # Chris Zen
    allow 2001:8b0:1d14::0/48;  # AAISP range
    allow 2a02:8010:6680::0/48; # Chris Zen
    allow 2a00:23c8:b0ac::0/48; # Chris 2
    deny all;
  '';

  PG_AUTH = ''
    #type   database  DBuser  auth-method
    local   all       all     trust
    host    sameuser  all     127.0.0.1/32          scram-sha-256
    host    sameuser  all     ::1/128               scram-sha-256
    host    sameuser  all     10.3.0.0/16           scram-sha-256
    host    sameuser  all     10.47.0.0/16          scram-sha-256
    host    sameuser  all     2001:8b0:1d14::0/48   scram-sha-256
    host    sameuser  all     2a02:8010:6680::0/48  scram-sha-256
    host    sameuser  all     2a00:23c8:b0ac::0/48  scram-sha-256
  '';

  PG_PASS = "leighhack1234";
}
