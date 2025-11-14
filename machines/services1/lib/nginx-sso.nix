let
  CONFIG = import ../config.nix;
in
{ lib }:
''
  ---

  login:
    title: "Leigh Hackspace - Login"
    default_method: "oidc"
    # default_method: "simple"
    hide_mfa_field: true
    names:
      oidc: "Leigh Hackspace Account"

  cookie:
    domain: ".leighhack.org"
    # Generated with: cat /dev/urandom | tr -dc 'A-Za-z0-9' | dd bs=1 count=60
    authentication_key: "${lib.strings.trim (builtins.readFile CONFIG.NGINX_SSO_AUTH_KEY_FILE)}"
    expire: 86400

  listen:
    addr: "0.0.0.0"
    port: 8082

  audit_log:
    targets:
      - fd://stdout
      - file:///var/log/nginx-sso/audit.jsonl
    events: ['access_denied', 'login_success', 'login_failure', 'logout', 'validate']
    headers: ['x-origin-uri']
    trusted_ip_headers: ["X-Forwarded-For", "RemoteAddr", "X-Real-IP"]

  acl:
    rule_sets:
    - rules:
      - field: "x-host"
        regexp: ".*"
      allow: ["@_authenticated"]

  providers:
    oidc:
      client_id: "pyBuPjaPD7xaiy8hAQpy4W02A3G0aIKyUakPtSaD"
      client_secret: "${lib.strings.trim (builtins.readFile CONFIG.NGINX_SSO_CLIENT_SECRET_FILE)}"
      redirect_url: "https://login.leighhack.org/login"
      # Optional, defaults to "OpenID Connect"
      issuer_name: "Leigh Hackspace"
      issuer_url: "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/nginx-login/"

      # Optional, defaults to no limitations
      # require_domain: "example.com"
      # Optional, defaults to "user-id"
      # user_id_method: "full-email"
''
