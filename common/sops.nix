# MODULE — NixOS module; in a machine's module list or a directory's imports, never read as data.
{
  config,
  pkgs,
  lib,
  ...
}: let
  # sops-nix (f140661) does not expose decrypted contents, so secrets that
  # are embedded into generated configs at build time are decrypted in a
  # small derivation (see sopsSecretText below).
  sopsFile = ../secrets/secrets.yaml;
in {
  options = {
    sopsSecretText = lib.mkOption {
      description = ''
        Takes a name of a string secret from the shared sops file
        (secrets/secrets.yaml) and returns a store path containing its
        decrypted value. Use this ONLY for a secret that has to be *inside* a
        generated file — embedded in a systemd unit, a rendered config, an
        initrd — because what it returns is a world-readable store path and
        what it decrypts is the whole secrets.yaml (see infra.buildTimeSecrets).
        Anything a service can read at runtime belongs in sops.secrets and
        /run/secrets instead.
      '';
      type = lib.types.unspecified; # name -> store path containing the value
    };

    infra.buildTimeSecrets = lib.mkOption {
      type = with lib.types; attrsOf str;
      default = {
        # One name, one sentence: why this secret has to be in a *built* file rather
        # than read from /run/secrets at runtime.  This attrset is the whole audit
        # surface — `sopsSecretText` refuses a name that is not in it — because the
        # list used to be a comment at the top of machines/services1/sops.nix, and a
        # comment listing exceptions is a list nobody updates: it named seven names
        # while ten were going down this path (audit §4.1).  Each of these is a
        # credential that is world-readable in the nix store, forever, by name; the
        # sentence is the reason someone accepted that.
        slack_url = "Slack webhook URL interpolated into alert ExecStart lines (postgres, backup, containers).";
        pg_pass = "Postgres password inside connection URIs (outline's databaseUrl, affine's, postgres.nix's own clauses).";
        nginx_sso_auth = "Cookie signing key inside nginx-sso's generated YAML config.";
        nginx_sso_client_secret = "authentik client secret inside nginx-sso's generated YAML config.";
        unifi_db_password = "Password inside the UniFi controller's mongodb URI.";
        mattermost_authentik_secret = "authentik client secret inside mattermost's config file.";
        status_dashboard_lan_token = "Restart token that must match in three places at once: the dashboard's --restart-token, the nginx header on the LAN vhosts, and services2's dashboard.";
        grafana_admin_password = "Grafana admin password, set via its provisioning env (the plaintext-store warning this emits is a known finding).";
        grafana_secret_key = "Grafana's signing secret key, same provisioning file.";
        strata_api_key = "Bearer key inside the Strata engines' own generated run configs (services2 and aibox) and inside the door's proxy to them; one secret for both because `strataBearer` in common/front-door.nix injects a single header, so a per-engine key would 401 the door's path.";
      };
      description = ''
        The names allowed through config.sopsSecretText, each with why it cannot be a
        runtime secret.  Adding a name here is the deliberate act; adding a call site
        without one is an eval error that says so.
      '';
    };

    sopsSecretsKeyFile = lib.mkOption {
      description = ''
        Age key file used to decrypt secrets at build time. It must be
        readable by the nix build user (nixbld), hence the group ownership
        the tmpfiles rules below repair (the decrypted secrets are
        already world-readable in the nix store, so this adds no exposure).

        READ THAT AS A REPAIR, NOT AN INSTALL: the rules are `z`, and `z` does
        nothing when the path is absent — verified on gw with a two-line
        fragment, where `z /tmp/zt-test` created nothing and an `f` rule in the
        same file did.  `just install-sops-key` is what creates the file; skip
        it and the failure is a sops error inside an unrelated build much
        later.  This option defaults to the path sops-nix itself uses
        (config.sops.age.keyFile), and that one sops-nix does need at runtime.
      '';
      type = lib.types.str;
      default = "/var/lib/sops-nix/key.txt";
    };
  };

  config = {
    sopsSecretText = name: let
      reason = config.infra.buildTimeSecrets.${name} or null;
      validName = builtins.match "[A-Za-z0-9_]+" name != null;
    in
      # Two guards on the name, both cheap and both about a class rather than an
      # instance.  An unlisted name means someone added a call site without writing the
      # sentence that justifies a world-readable store path for it.  A name that is not
      # a bare identifier would be interpolated into the shell command below — and into
      # the `yq` path expression, where a dot reads a subtree and a quote breaks the
      # command — so it is refused outright rather than escaped (audit §4.4: every
      # caller passes a literal today; this is what stops *today* from being a rule).
      assert (
        validName
        || throw "sopsSecretText: '${name}' is not a bare identifier, so it cannot be interpolated into the decryption command safely"
      );
      assert (
        reason
        != null
        || throw ''
          sopsSecretText: '${name}' is not in infra.buildTimeSecrets (common/sops.nix).
          That attrset is the list of secrets allowed to end up world-readable in the nix
          store because they must be *inside* a generated file.  Either declare the name
          there with the sentence that says why, or — for anything a service can open
          itself — declare it in sops.secrets and read it from /run/secrets/${name}.
        ''
      );
        pkgs.runCommand "sops-secret-${name}" {
          nativeBuildInputs = [
            pkgs.sops
            pkgs.yq
          ];
          # Nix does not pass the client environment into builds, so the key
          # location is fixed here; it must be readable by the sandbox user
          # (see sopsSecretsKeyFile).
          SOPS_AGE_KEY_FILE = config.sopsSecretsKeyFile;
        } ''
          tmp=$(mktemp -d)
          sops -d --output-type json ${sopsFile} > "$tmp/all.json"
          # `yq ... | tr -d '\n'`: yq ends its output with a newline, and a token
          # or URL read from the resulting store path would otherwise carry it —
          # nginx would get a header value with a trailing newline (a request
          # splitting risk) and the dashboard would compare tokens against a
          # value that can never match the header it receives.
          yq -r ".${name}" "$tmp/all.json" | tr -d '\r\n' > $out
        '';

    environment.systemPackages = with pkgs; [sops];

    environment.variables = {
      SOPS_AGE_KEY_FILE = config.sops.age.keyFile;
    };

    environment.sessionVariables = {
      SOPS_AGE_KEY_FILE = config.sops.age.keyFile;
    };

    # Make the key visible to sandboxed builds (see sopsSecretsKeyFile).
    nix.settings."extra-sandbox-paths" = ["/var/lib/sops-nix"];

    systemd.tmpfiles.rules = [
      # Repair the mode and group of the sops age key so nix builds can decrypt at
      # build time (see sopsSecretsKeyFile).  These are `z` rules, and `z` only acts on
      # a file that already exists — it creates nothing — so the file itself comes from
      # `just install-sops-key`.
      "z /var/lib/sops-nix 0550 root nixbld -"
      "z /var/lib/sops-nix/key.txt 0440 root nixbld -"
    ];

    sops = {
      age.keyFile = "/var/lib/sops-nix/key.txt";
      defaultSopsFile = sopsFile;
      secrets = {
        immich_oidc_client_secret = {};

        # Shared restart token for the status dashboards (see
        # machines/services1/services/status.nix and
        # machines/aibox/status-dashboard.nix).
        #
        # This one is embedded in the generated systemd units (via the
        # dashboard's --restart-token) and injected into the LAN vhosts as an
        # nginx header, so it lands in the world-readable nix store: it is a
        # LAN-only convenience credential, not a real secret.  Anything that
        # must stay out of the store belongs in CONFIG.ENV_FILE
        # (/run/secrets/env_file, root-only at runtime) instead.
        status_dashboard_lan_token = {};
      };
    };
  };
}
