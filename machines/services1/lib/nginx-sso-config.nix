{ lib }:
let
  CONFIG = import ../config.nix;

  readSecret = file: lib.strings.trim (builtins.readFile file);
in
lib.generators.toYAML { } {
  login = {
    title = "Leigh Hackspace - Login";
    default_method = "oidc";
    hide_mfa_field = true;
    names.oidc = "Leigh Hackspace Account";
  };

  cookie = {
    domain = ".leighhack.org";
    authentication_key = readSecret CONFIG.NGINX_SSO_AUTH_KEY_FILE;
    expire = 86400;
  };

  listen = {
    addr = "0.0.0.0";
    port = 8082;
  };

  audit_log = {
    targets = [
      "fd://stdout"
      "file:///var/log/nginx-sso/audit.jsonl"
    ];
    events = [
      "access_denied"
      "login_success"
      "login_failure"
      "logout"
      "validate"
    ];
    headers = [ "x-origin-uri" ];
    trusted_ip_headers = [
      "X-Forwarded-For"
      "RemoteAddr"
      "X-Real-IP"
    ];
  };

  acl.rule_sets = [
    # Grant Authenticated Access
    {
      rules = [
        {
          field = "x-host";
          regexp = ".*";
        }
      ];
      allow = [ "@_authenticated" ];
    }
    # Grant Tailscale (IPv4)
    {
      rules = [
        {
          field = "x-real-ip";
          regexp = "^100.64.";
        }
      ];
      allow = [ "@_anonymous" ];
    }
    # Grant Tailscale (IPv6)
    {
      rules = [
        {
          field = "x-real-ip";
          regexp = "^fd7a:115c:a1e0";
        }
      ];
      allow = [ "@_anonymous" ];
    }
    # Grant Hackspace LAN (IPv4)
    {
      rules = [
        {
          field = "x-real-ip";
          regexp = "^10.3.";
        }
      ];
      allow = [ "@_anonymous" ];
    }
    # Grant Hackspace LAN (IPv6)
    {
      rules = [
        {
          field = "x-real-ip";
          regexp = "^2001:8b0:1d14";
        }
      ];
      allow = [ "@_anonymous" ];
    }
  ];

  providers.oidc = {
    client_id = "pyBuPjaPD7xaiy8hAQpy4W02A3G0aIKyUakPtSaD";
    client_secret = readSecret CONFIG.NGINX_SSO_CLIENT_SECRET_FILE;
    redirect_url = "https://login.leighhack.org/login";
    issuer_name = "Leigh Hackspace";
    issuer_url = "https://${CONFIG.AUTHENTIK_DOMAIN}/application/o/nginx-login/";
  };
}
