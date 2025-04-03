{ door-entry-management-system, door-entry-bluetooth-web-app }:

{ config, pkgs, ... }:

{
  imports = [
    ./services

    ./ai.nix
    ./configuration.nix
    ./containers.nix
    ./hardware-configuration.nix
    ./http.nix
    ./networking.nix
    ./postgres.nix

    # Add packages for the door system to the system pkgs
    ({ config, pkgs, options, ... }: {
      nixpkgs.overlays = [
        (final: prev: {
          door-entry-management-system = door-entry-management-system.packages.${pkgs.system}.default;
          door-entry-bluetooth-web-app = door-entry-bluetooth-web-app.packages.${pkgs.system}.default;
        })
      ];
    })
  ];
}
