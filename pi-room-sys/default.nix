{
  lib,
  pkgs,
  nix-software-center,
}:

let
  nfsServer = "10.3.1.32"; # DNS is too slow so use IP address
  system = "x86_64-linux";
  sys = (
    lib.nixosSystem {
      inherit pkgs system;

      modules = [
        ./configuration.nix
        ((import ./netboot.nix) { inherit nfsServer; })
        {
          environment.systemPackages = [ nix-software-center.packages.${system}.nix-software-center ];
        }
      ];
    }
  );
in
sys
