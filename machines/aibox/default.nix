flakeInputs:

{ config, pkgs, ... }:

{
  networking.hostName = "aibox"; # Define your hostname.

  imports = [
    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./hardware-configuration.nix
    ./networking.nix
    ((import ./pxe-server.nix) flakeInputs)
    ./sso.nix
    ./sunshine.nix
  ];
}
