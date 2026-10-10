flakeInputs: {
  networking.hostName = "services2"; # Define your hostname.

  imports = [
    ./configuration.nix
    ./hardware-configuration.nix
    ./networking.nix

    flakeInputs.sops-nix.nixosModules.sops
  ];
}
