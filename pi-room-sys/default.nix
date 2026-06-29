{ lib, pkgs }:

let
  nfsServer = "10.3.1.32"; # DNS is too slow so use IP address
  sys = (
    lib.nixosSystem {
      inherit pkgs;
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        ((import ./netboot.nix) { inherit nfsServer; })
      ];
    }
  );
in
sys
