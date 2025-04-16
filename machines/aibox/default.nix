{ config, pkgs, ... }:

{
  networking.hostName = "aibox"; # Define your hostname.

  imports = [
    ./ai.nix
    ./configuration.nix
    ./hardware-configuration.nix
    ./networking.nix
  ];
}
