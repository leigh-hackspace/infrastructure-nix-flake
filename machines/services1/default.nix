{ flakeInputs }:

{
  networking.hostName = "services1"; # Define your hostname.

  imports = [
    ((import ./services) { inherit flakeInputs; })

    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./hardware-configuration.nix
    ./http.nix
    ./networking.nix
    ./nfs-client.nix
  ];
}
