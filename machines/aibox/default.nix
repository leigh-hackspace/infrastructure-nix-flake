flakeInputs:

{ config, pkgs, ... }:

{
  networking.hostName = "aibox"; # Define your hostname.

  imports = [
    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./hardware-configuration.nix
    ./netboot.nix
    ./networking.nix
    ./sso.nix
  ];
}
