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

    whisper-ws = {
      url = "git+file:///home/leigh-admin/Projects/whisper-ws"; # WebSocket gateway for whisper.cpp
      inputs.nixpkgs.follows = "nixpkgs";
    };

    gocardless-tools = {
      url = "git+file:///home/leigh-admin/Projects/gocardless-tools"; # Private Git repo
      # url = "github:leigh-hackspace/gocardless-tools";
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

              ./common/sops.nix
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
            specialArgs = flakeInputs;
            modules = [
              fix-nix-shell

              nixos-utils.nixosModules.rollback
              nixos-utils.nixosModules.containers

              ./common/sops.nix
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
        devShells.${system}.default = let
          pkgs = import nixpkgs { inherit system; };
          rust = pkgs.rust.packages.stable;
        in
          pkgs.mkShell {
            packages = [
              rust.rustc
              rust.cargo
              rust.rustfmt
              # NOTE: not pkgs.wasm-bindgen-cli. frigate-monitor's crate is
              # pinned to wasm-bindgen 0.2.128 in Cargo.lock, and the wasm
              # bindgen *schema* must match the CLI version exactly, so
              # nixpkgs' 0.2.121 would refuse to process the output. The
              # shellHook below installs the matching 0.2.128 from crates.io
              # (cached after first run). `lld` is the linker the wasm32
              # target links with.
              pkgs.lld
              pkgs.just
              pkgs.git
            ];
            shellHook = ''
              if [ "$(wasm-bindgen --version 2>/dev/null)" != "wasm-bindgen 0.2.128" ]; then
                  echo "frigate-monitor: installing pinned wasm-bindgen-cli 0.2.128 (~1 min, cached afterwards)…"
                  cargo install -f wasm-bindgen-cli --version 0.2.128 --quiet
              fi
              # Put the installed CLI ahead of any nixpkgs one on PATH.
              export PATH="$HOME/.cargo/bin:$PATH"
            '';
          };
    };
}
