{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.05";
    nixos-hardware.url = "github:NixOS/nixos-hardware/master";
    door-entry-management-system.url = "github:leigh-hackspace/door-entry-system?dir=management-system";
    door-entry-bluetooth-web-app.url = "github:leigh-hackspace/door-entry-system?dir=bluetooth-web-app";
    llama-cpp-leigh.url = "github:leigh-hackspace/llama.cpp/master";
    gocardless-tools = {
      url = "git+file:///home/leigh-admin/Projects/gocardless-tools";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    pxe-server = {
      url = "git+file:///home/cjdell/Projects/pxe-server";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      nixos-hardware,
      door-entry-management-system,
      door-entry-bluetooth-web-app,
      llama-cpp-leigh,
      gocardless-tools,
      pxe-server,
    }@attrs:
    {
      nixosConfigurations =
        let
          system = "x86_64-linux";
          fix-nix-shell = {
            nix.registry.nixpkgs.flake = nixpkgs;
          }; # Make "nix-shell" use the flake version
        in
        {
          services1 = nixpkgs.lib.nixosSystem {
            inherit system;
            pkgs = import nixpkgs {
              inherit system;
              config = {
                allowUnfree = true;
              };
            };
            modules = [
              fix-nix-shell
              ./common/users.nix
              # Add packages for the door system to the system pkgs
              (
                {
                  config,
                  pkgs,
                  options,
                  ...
                }:
                {
                  nixpkgs.overlays = [
                    (final: prev: {
                      door-entry-management-system = door-entry-management-system.packages.${pkgs.system}.default;
                      door-entry-bluetooth-web-app = door-entry-bluetooth-web-app.packages.${pkgs.system}.default;
                      gocardless-tools = gocardless-tools.packages.${pkgs.system}.default;
                    })
                  ];
                }
              )

              (import ./machines/services1)
            ];
          };

          aibox =
            let
              # Local repo just for experiments...
              llama-cpp-local = builtins.getFlake (toString /home/cjdell/Projects/llama.cpp);
            in
            nixpkgs.lib.nixosSystem {
              inherit system;
              pkgs = import nixpkgs {
                inherit system;
                config = {
                  allowUnfree = true;
                };
              };
              modules = [
                fix-nix-shell
                ./common/users.nix
                (
                  {
                    config,
                    pkgs,
                    options,
                    ...
                  }:
                  {
                    nixpkgs.overlays = [
                      (final: prev: {
                        llama-cpp-leigh-rocm = llama-cpp-leigh.packages.${pkgs.system}.rocm;
                        llama-cpp-local-cpu = llama-cpp-local.packages.${pkgs.system}.default;
                      })
                    ];
                  }
                )

                (import ./machines/aibox pxe-server)
              ];
            };
        };
    };
}
