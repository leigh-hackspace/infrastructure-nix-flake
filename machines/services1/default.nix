flakeInputs:

{
  networking.hostName = "services1"; # Define your hostname.

  imports = [
    ./sops.nix
    ./services

    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./dns-sync.nix
    ./hardware-configuration.nix
    ./http.nix
    ./network-status.nix
    ./networking.nix
    ./nfs-client.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}
