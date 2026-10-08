{ config, pkgs, lib, ... }:

let
  # sops-nix (f140661) does not expose decrypted contents, so secrets that
  # are embedded into generated configs at build time are decrypted in a
  # small derivation (see sopsSecretText below).
  sopsFile = ../secrets/secrets.yaml;
in
{
  options = {
    sopsSecretText = lib.mkOption {
      description = ''
        Takes a name of a string secret from the shared sops file
        (secrets/secrets.yaml) and returns a store path containing its
        decrypted value. Use this for secrets that are embedded into
        generated configs at build time; runtime files are declared under
        sops.secrets instead.
      '';
      type = lib.types.unspecified; # name -> store path containing the value
    };

    sopsSecretsKeyFile = lib.mkOption {
      description = ''
        Age key file used to decrypt secrets at build time. It must be
        readable by the nix build user (nixbld), hence the group ownership
        enforced by the tmpfiles rules below (the decrypted secrets are
        already world-readable in the nix store, so this adds no exposure).
        Install it with `just install-sops-key`.
      '';
      type = lib.types.str;
      default = "/var/lib/sops-nix/key.txt";
    };
  };

  config = {
    sopsSecretText =
      name:
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

    environment.systemPackages = with pkgs; [ sops ];

    environment.variables = {
      SOPS_AGE_KEY_FILE = config.sops.age.keyFile;
    };

    environment.sessionVariables = {
      SOPS_AGE_KEY_FILE = config.sops.age.keyFile;
    };

    # Make the key visible to sandboxed builds (see sopsSecretsKeyFile).
    nix.settings."extra-sandbox-paths" = [ "/var/lib/sops-nix" ];

    systemd.tmpfiles.rules = [
      # Make the sops age key readable by the nix build user so secrets can
      # be decrypted at build time (see sopsSecretsKeyFile).
      "z /var/lib/sops-nix 0550 root nixbld -"
      "z /var/lib/sops-nix/key.txt 0440 root nixbld -"
    ];

    sops = {
      age.keyFile = "/var/lib/sops-nix/key.txt";
      defaultSopsFile = sopsFile;
      secrets = {
        immich_oidc_client_secret = { };

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
        status_dashboard_lan_token = { };
      };
    };
  };
}
