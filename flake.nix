{
  inputs = {
    nixpkgs.url                       = "github:nixos/nixpkgs/nixos-24.11";
    nixos-hardware.url                = "github:NixOS/nixos-hardware/master";
    door-entry-management-system.url  = "github:leigh-hackspace/door-entry-system?dir=management-system";
    door-entry-bluetooth-web-app.url  = "github:leigh-hackspace/door-entry-system?dir=bluetooth-web-app";
  };

  outputs = { self, nixpkgs, nixos-hardware, door-entry-management-system, door-entry-bluetooth-web-app }@attrs: {
    nixosConfigurations.services1 =
      let
        system = "x86_64-linux";
      in
      nixpkgs.lib.nixosSystem {
        inherit system;
        pkgs = import nixpkgs { inherit system; config = { allowUnfree = true; }; };
        modules = [
          { nix.registry.nixpkgs.flake = nixpkgs; }   # Make "nix-shell" use the flake version
          (import ./machines/services1 { inherit door-entry-management-system door-entry-bluetooth-web-app; })
        ];
      };
  };
}
