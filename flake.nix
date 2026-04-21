{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    nixos-hardware.url = "github:NixOS/nixos-hardware/master";

    nixos-utils = {
      url = "github:cjdell/nixos-utils";
      # url = "git+file:///home/leigh-admin/Projects/nixos-utils";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    door-entry-management-system.url = "github:leigh-hackspace/door-entry-system?dir=management-system";
    door-entry-bluetooth-web-app.url = "github:leigh-hackspace/door-entry-system?dir=bluetooth-web-app";

    llama-cpp = {
      url = "github:ggml-org/llama.cpp";
      # url = "github:leigh-hackspace/llama.cpp/new-webui-build";
      # url = "git+file:///home/leigh-admin/Projects/llama.cpp";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    gocardless-tools = {
      url = "git+file:///home/leigh-admin/Projects/gocardless-tools"; # Private Git repo
      # url = "github:leigh-hackspace/gocardless-tools";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    pxe-server = {
      url = "git+file:///home/leigh-admin/Projects/pxe-server";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    headplane = {
      url = "github:tale/headplane/v0.6.2";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      nixos-utils,
      llama-cpp,
      pxe-server,
      headplane,
      ...
    }@flakeInputs:
    let
      system = "x86_64-linux";
    in
    {
      listGenerations =
        let
          pkgs = import nixpkgs { inherit system; };
        in
        (pkgs.writers.writeNuBin "list-generations" ./nu/list-generations.nu);

      nixosConfigurations =
        let
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
                permittedInsecurePackages = [
                  "jitsi-meet-1.0.8792"
                ];
              };
            };
            modules = [
              fix-nix-shell

              nixos-utils.nixosModules.rollback
              nixos-utils.nixosModules.containers

              ./common/tools.nix
              ./common/users.nix

              # provides `services.headplane.*` NixOS options.
              headplane.nixosModules.headplane

              {
                # provides `pkgs.headplane`
                nixpkgs.overlays = [ headplane.overlays.default ];
              }

              ((import ./machines/services1) { inherit flakeInputs; })
            ];
          };

          aibox =
            let
              # Local repo just for experiments...
              llama-cpp-local = builtins.getFlake (toString /home/leigh-admin/Projects/llama.cpp);
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

                nixos-utils.modules.rollback

                ./common/tools.nix
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
                        # llama-cpp-leigh-rocm = llama-cpp.packages.${pkgs.system}.rocm;
                        llama-cpp-leigh-vulkan = llama-cpp.packages.${pkgs.system}.vulkan;
                        # llama-cpp-cpu = llama-cpp.packages.${pkgs.system}.default;
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
