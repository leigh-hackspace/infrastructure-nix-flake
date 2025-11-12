{ config, pkgs, ... }:

{
  networking.hostName = "services1"; # Define your hostname.

  imports = [
    ./services

    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./hardware-configuration.nix
    ./http.nix
    ./networking.nix
    ./nfs-client.nix
    ./postgres.nix
  ];

  nix.settings.sandbox = "relaxed";

  system.activationScripts.protectSecrets = ''
    mkdir -p                    /var/lib/secrets
    chown -R root:secrets       /var/lib/secrets
    chmod -R +X,-w,u+r,g+r,o-rx /var/lib/secrets
  '';
}
