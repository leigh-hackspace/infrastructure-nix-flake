{ config, pkgs, ... }:

{
  environment.systemPackages = with pkgs; [ sops ];

  environment.variables = {
    SOPS_AGE_KEY_FILE = config.sops.age.keyFile;
  };

  environment.sessionVariables = {
    SOPS_AGE_KEY_FILE = config.sops.age.keyFile;
  };

  sops = {
    age.keyFile = "/var/lib/sops-nix/key.txt";
    defaultSopsFile = ../secrets/secrets.yaml;
    secrets = {
      immich_oidc_client_secret = { };
    };
  };
}
