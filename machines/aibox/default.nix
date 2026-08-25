flakeInputs:

{ config, pkgs, ... }:

{
  networking.hostName = "aibox"; # Define your hostname.

  imports = [
    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./hardware-configuration.nix
    ./immich.nix
    ./netboot.nix
    ./networking.nix
    ./nvidia.nix
    ./nfs-client.nix
    ./sso.nix
    ./status-dashboard.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}
