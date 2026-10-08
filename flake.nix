{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
    nixos-hardware.url = "github:NixOS/nixos-hardware/master";
    # Rust build helpers (cached dependency derivations); see common/crane.nix.
    crane.url = "github:ipetkov/crane";

    nixos-utils = {
      url = "github:cjdell/nixos-utils";
      # url = "git+file:///home/leigh-admin/Projects/nixos-utils";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    door-entry-management-system.url = "github:leigh-hackspace/door-entry-system?dir=management-system";
    # door-entry-bluetooth-web-app.url = "github:leigh-hackspace/door-entry-system?dir=bluetooth-web-app";

    llama-cpp = {
      url = "github:ifm-ai/llama.cpp/model/K2Horizon";
      # url = "github:leigh-hackspace/llama.cpp/new-webui-build";
      # url = "git+file:///home/leigh-admin/Projects/llama.cpp";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    whisper-ws = {
      url = "git+file:///home/leigh-admin/Projects/whisper-ws"; # WebSocket gateway for whisper.cpp
      inputs.nixpkgs.follows = "nixpkgs";
    };

    pi-room-sys = {
      url = "git+file:///home/leigh-admin/Projects/pi-room-sys";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      nixos-utils,
      llama-cpp,
      whisper-ws,
      ...
    }@flakeInputs:

    let
      system = "x86_64-linux";

      # Helper sets handed to every module through specialArgs (modules read them
      # as `INFRA`), so shared helpers are not re-imported in each file.
      extraArgs = {
        INFRA = import ./common/systemd.nix { lib = nixpkgs.lib; };
      };
    in
    {
      nixosConfigurations =
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
            specialArgs = flakeInputs // extraArgs;
            modules = [
              # Make "nix-shell" use the flake version
              { nix.registry.nixpkgs.flake = nixpkgs; }

              nixos-utils.nixosModules.rollback
              nixos-utils.nixosModules.containers

              ./common/sops.nix
              ./common/containers.nix
              ./common/nas.nix
              ./common/tools.nix
              ./common/users.nix

              ((import ./machines/services1) flakeInputs)
            ];
          };

          aibox = nixpkgs.lib.nixosSystem {
            inherit system;
            pkgs = import nixpkgs {
              inherit system;
              config = {
                allowUnfree = true;
              };
            };
            specialArgs = flakeInputs // extraArgs;
            modules = [
              # Make "nix-shell" use the flake version
              { nix.registry.nixpkgs.flake = nixpkgs; }

              nixos-utils.nixosModules.rollback
              nixos-utils.nixosModules.containers

              ./common/sops.nix
              ./common/containers.nix
              ./common/nas.nix
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
                      # llama-cpp-leigh-rocm = llama-cpp.packages.${pkgs.stdenv.hostPlatform.system}.rocm;
                      llama-cpp-leigh-vulkan = llama-cpp.packages.${pkgs.stdenv.hostPlatform.system}.vulkan;
                      # llama-cpp-cpu = llama-cpp.packages.${pkgs.stdenv.hostPlatform.system}.default;
                      whisper-ws = whisper-ws.packages.${pkgs.stdenv.hostPlatform.system}.default;
                    })
                  ];
                }
              )

              ((import ./machines/aibox) flakeInputs)
            ];
          };
        };

      # `nix develop`
      #
      # Toolchain for building the frigate-monitor web UI locally, mirroring
      # what machines/aibox/frigate-monitor.nix does in its frontendDist
      # derivation: cargo/rustc/rustfmt, `lld` (the wasm32 linker / wasm-ld),
      # `just` and `git`. The stable toolchain already ships the
      # wasm32-unknown-unknown std library, so `cargo build --target
      # wasm32-unknown-unknown` needs no rustup or network — see rustc.nix's
      # `--target` list. `dist/` is git-ignored and rebuilt by the flake; run
      # `cd frigate-monitor/frontend && nix develop --command bash build.sh`
      # to iterate, or just `just build-frontend` (below).
      devShells.${system}.default =
        let
          pkgs = import nixpkgs { inherit system; };
          rust = pkgs.rust.packages.stable;
          CRANE = import ./common/crane.nix {inherit pkgs; crane = flakeInputs.crane;};
        in
        pkgs.mkShell {
          packages = [
            rust.rustc
            rust.cargo
            rust.rustfmt
            rust.clippy
            # NOTE: not pkgs.wasm-bindgen-cli. The SPAs are pinned to
            # wasm-bindgen 0.2.128 in their Cargo.locks, and the wasm bindgen
            # *schema* must match the CLI version exactly, so nixpkgs' 0.2.121
            # would refuse to process the output. This is the same pinned CLI
            # derivation the flake uses to build the SPA bundles
            # (common/crane.nix), so it is already in the store.
            CRANE.wasmBindgenCli
            pkgs.lld
            pkgs.just
            pkgs.git
            # node is needed by the headless-browser test suite (filestore/tests)
            pkgs.nodejs
            # handy for debugging (zip/struct inspection of generated archives, etc.)
            pkgs.python3
          ];
          shellHook = ''
            # In-repo path dependencies (common-rs/, frontend/dto) are git-ignored
            # symlinks; cargo needs them, the Nix build does not.  Run the
            # working-tree copy, not the one copied into the shell's store path.
            if [ -f ./common/shared-rs-links.sh ]; then
                bash ./common/shared-rs-links.sh >/dev/null
            fi
          '';
        };
    };
}
