{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    nixos-hardware.url = "github:NixOS/nixos-hardware/master";

    nixos-utils = {
      url = "github:cjdell/nixos-utils";
      # url = "git+file:///home/leigh-admin/Projects/nixos-utils";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    door-entry-management-system.url = "github:leigh-hackspace/door-entry-system?dir=management-system";
    # door-entry-bluetooth-web-app.url = "github:leigh-hackspace/door-entry-system?dir=bluetooth-web-app";

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
      inputs.nixpkgs-new.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      nixos-utils,
      llama-cpp,
      ...
    }@flakeInputs:

    let
      system = "x86_64-linux";
    in
    {
      nixosConfigurations =
        let
          fix-nix-shell = {
            # Make "nix-shell" use the flake version
            nix.registry.nixpkgs.flake = nixpkgs;
          };
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
            specialArgs = flakeInputs;
            modules = [
              fix-nix-shell

              nixos-utils.nixosModules.rollback
              nixos-utils.nixosModules.containers

              ./common/tools.nix
              ./common/users.nix

              ./machines/services1
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

                nixos-utils.nixosModules.rollback

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

                ((import ./machines/aibox) flakeInputs)
              ];
            };
        };
    };
}
