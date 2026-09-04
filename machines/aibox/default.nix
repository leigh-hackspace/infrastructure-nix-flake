flakeInputs:

{ config, pkgs, ... }:

{
  networking.hostName = "aibox"; # Define your hostname.

  imports = [
    ./ai.nix
    # ./alexandria.nix
    ./configuration.nix
    ./containers.nix
    ./hardware-configuration.nix
    ./immich.nix
    ./monitoring.nix
    ./netboot.nix
    ./networking.nix
    ./nvidia.nix
    ./nfs-client.nix
    ./sso.nix
    ./status-dashboard.nix
    ./whisper.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}
