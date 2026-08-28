{
  networking.hostName = "services1"; # Define your hostname.

  imports = [
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
  ];
}
