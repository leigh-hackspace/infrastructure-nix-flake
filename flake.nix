{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
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

    # The software centre the Pi Room netboot image ships in its
    # `environment.systemPackages`.  It used to be an input of that tree's own
    # flake (machines/pi-room-sys, `nixosConfigurations.pi-room-sys` below), which
    # is why the network has an input for a machine nobody can SSH into.
    nix-software-center = {
      url = "github:snowfallorg/nix-software-center";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Strata (https://github.com/Niko1221/Strata), the ggml-based engine that
    # serves Qwen3.8-Flash-Next across GPU VRAM, RAM and the CPU.  Tracked on
    # `main` on purpose: it is the bleeding-edge tree, and services2's CUDA engine
    # (machines/services2/strata-package.nix) builds from it.  Upstream's own
    # flake only provides a devShell, so the engine is still packaged by hand;
    # this input is the source.  aibox's older HIP package still pins its own
    # fetchFromGitHub, so `nix flake update strata` moves only the CUDA engine.
    # `inputs.nixpkgs.follows` keeps the eval on one nixpkgs.
    strata = {
      url = "github:Niko1221/Strata";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    nixpkgs,
    nixos-utils,
    llama-cpp,
    nix-software-center,
    ...
  } @ flakeInputs: let
    system = "x86_64-linux";

    # Make "nix-shell" use the flake version.  A plain module, not part of
    # extraArgs: specialArgs is what modules read as arguments, so the module
    # list refers to it by name here rather than repeating it per machine.
    nix-shell-registry = {nix.registry.nixpkgs.flake = nixpkgs;};

    # Helper sets handed to every module through specialArgs (modules read them
    # as `INFRA`), so shared helpers are not re-imported in each file.
    extraArgs = {
      INFRA = import ./common/systemd.nix {lib = nixpkgs.lib;};
    };

    # The package set things that are not a machine's own `pkgs` are built with:
    # `packages.*` below, and the one package an overlay puts into aibox's set.
    pkgs = import nixpkgs {
      inherit system;
      config = {
        allowUnfree = true;
      };
    };

    # crates/whisper-ws, from its own recipe: it is Rust, so it lives in crates/,
    # but it is not a workspace member and not built by common/crane.nix — the
    # header of crates/whisper-ws/package.nix says why.  aibox's overlay below is
    # what puts it in `pkgs.whisper-ws` for whisper-ws.service's ExecStart.
    whisperWsPkg = pkgs.callPackage ./crates/whisper-ws/package.nix {};

    # The Pi Room computers' image: a machine, but one nobody switches.  It is a
    # nixosConfiguration like any other in here, and it is *built by* aibox rather
    # than deployed — machines/aibox/netboot.nix serves its kernel, initrd,
    # kernel params and squashfs store over iPXE + NFS, and reads this value out
    # of specialArgs to do it.  So it is a `let` binding and not an entry of
    # `nixosConfigurations` directly: aibox's config needs it, and an attrset
    # cannot refer to its own sibling.
    #
    # Deliberately absent: `nixos-utils.nixosModules.rollback`.  The root is a
    # tmpfs over a squashfs mounted off aibox and there is no profile to mark
    # good, so auto-rollback would be a timer that reboots a client into a
    # generation that does not exist.
    #
    # `nfsServer` is the address the *image* mounts its store from and it is an IP
    # for the reason written next to it in the file that came with this tree ("DNS
    # is too slow"); the iPXE menu that boots the image uses aibox's hostname, and
    # that string lives in machines/aibox/netboot.nix.
    piRoomSys = nixpkgs.lib.nixosSystem {
      inherit system;
      pkgs = import nixpkgs {
        inherit system;
        config = {
          allowUnfree = true;
        };
      };
      specialArgs = flakeInputs // extraArgs;
      modules = [
        ./machines/pi-room-sys/configuration.nix
        ((import ./machines/pi-room-sys/netboot.nix) {nfsServer = "10.3.1.32";})
        {
          environment.systemPackages = [nix-software-center.packages.${system}.nix-software-center];
        }
      ];
    };
  in {
    nixosConfigurations = {
      services1 = nixpkgs.lib.nixosSystem {
        inherit system;
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
          };
        };
        specialArgs = flakeInputs // extraArgs;
        modules = [
          nix-shell-registry

          nixos-utils.nixosModules.rollback
          nixos-utils.nixosModules.containers

          ./common/sops.nix
          ./common/containers.nix
          ./common/crane-args.nix
          ./common/build-farm.nix
          ./common/nas.nix
          ./common/tools.nix
          ./common/users.nix

          ((import ./machines/services1) flakeInputs)
        ];
      };

      # The second services box: a clone of services1 on AMD/Radeon hardware.  It
      # runs its own status dashboard and (since the RTX 3060 arrived) the Strata
      # CUDA engine — and nothing else, because its 30 GB of RAM is strata's: the
      # observability stack that used to live here (gatus, Prometheus, Grafana)
      # moved to services1 on 2026-10-17.  It is still this network's build
      # machine, and it is still measured — by services1's Prometheus and by gatus
      # from here.  See machines/services2/README.md for what runs and what is
      # left to measure it, and machines/services2/STRATA.md for the engine.
      services2 = nixpkgs.lib.nixosSystem {
        inherit system;
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
            # The RTX 3060 runs Strata's CUDA engine; this makes the splayed
            # cudaPackages_13_0 the toolkit the box builds against (and pulls
            # the CUDA stdenv in without a per-package override everywhere).
            cudaSupport = true;
          };
        };
        specialArgs = flakeInputs // extraArgs;
        modules = [
          nix-shell-registry

          nixos-utils.nixosModules.rollback

          ./common/sops.nix
          ./common/containers.nix
          ./common/crane-args.nix
          ./common/build-farm.nix
          ./common/nas.nix
          ./common/tools.nix
          ./common/users.nix

          ((import ./machines/services2) flakeInputs)
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
        # `piRoomSys` because this box serves that image: machines/aibox/netboot.nix
        # reads its build tree (kernel, initrd, kernel params, squashfs store) and
        # its boot logo.  It is passed in rather than imported because it is a
        # nixosSystem, not a module.
        specialArgs =
          flakeInputs
          // extraArgs
          // {
            piRoomSys = piRoomSys;
          };
        modules = [
          nix-shell-registry

          nixos-utils.nixosModules.rollback
          nixos-utils.nixosModules.containers

          ./common/sops.nix
          ./common/containers.nix
          ./common/crane-args.nix
          ./common/build-farm.nix
          ./common/nas.nix
          ./common/tools.nix
          ./common/users.nix

          (
            {
              config,
              pkgs,
              options,
              ...
            }: {
              nixpkgs.overlays = [
                (final: prev: {
                  # llama-cpp-leigh-rocm = llama-cpp.packages.${pkgs.stdenv.hostPlatform.system}.rocm;
                  llama-cpp-leigh-vulkan = llama-cpp.packages.${pkgs.stdenv.hostPlatform.system}.vulkan;
                  # llama-cpp-cpu = llama-cpp.packages.${pkgs.stdenv.hostPlatform.system}.default;
                  whisper-ws = whisperWsPkg;
                })
              ];
            }
          )

          ((import ./machines/aibox) flakeInputs)
        ];
      };

      # The Pi Room netboot image.  Here so it is evaluated and built by this
      # flake like every other machine — `nixos-rebuild --flake .#pi-room-sys`
      # would not do anything to a host, because there is no host: aibox boots
      # these machines over iPXE and they mount their store off aibox at runtime.
      # The why of the whole thing is in machines/pi-room-sys/README.md.
      pi-room-sys = piRoomSys;

      # The router.  The box exists (machines/gw/README.md: a Supermicro
      # X9SCL/X9SCM at 10.3.1.254) and this entry is deployed to it — but in
      # *co-existence* mode, beside the router it is meant to replace rather than
      # in its place, which is machines/gw/coexist.nix's whole job.  Two
      # consequences of how it is built, both deliberate:
      #
      #   * No ./common/nas.nix and no ./common/tools.nix: the router is the thing
      #     the network's DNS depends on, so it must not grow a dependency on a NAS
      #     that is still importing after a power cut.  tools.nix's wait-for-network
      #     unit reads `config.infra.nas.host`, which only exists with nas.nix, so
      #     the two are added and removed together — which is the check that keeps
      #     them out.
      #   * No ./common/build-farm.nix and no ./common/containers.nix: a router runs
      #     dnsmasq, pppd, openvpn, tailscale and nft, and nothing else.  The
      #     build-user invariant it does get, from common/base.nix's own import of
      #     common/nixbld.nix.
      gw = nixpkgs.lib.nixosSystem {
        inherit system;
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
          };
        };
        specialArgs = flakeInputs // extraArgs;
        modules = [
          nix-shell-registry

          nixos-utils.nixosModules.rollback

          ./common/sops.nix
          ./common/users.nix

          ((import ./machines/gw) flakeInputs)
        ];
      };
    };

    # `nix build .#strata` - the Strata engine (HIP/gfx1030) plus its Python
    # serve layer and the one-time model-prep tools: the same derivation
    # machines/aibox/strata.nix installs as a service. That file documents the
    # hand-run model download/pack/MTP steps this package deliberately keeps
    # out of the store.
    #
    # `nix build .#strata-cuda` - the same engine built for services2's RTX
    # 3060 (CUDA 13.0 / sm_86), from the bleeding-edge `strata` flake input.
    #
    # `nix build .#whisper-ws` - the aibox WebSocket gateway (crates/whisper-ws,
    # built by its own recipe).  It is a package here as well as an overlay entry
    # so it can be built on its own without the whole aibox system.
    packages.${system} = let
      pkgsCuda = import nixpkgs {
        inherit system;
        config = {
          allowUnfree = true;
          cudaSupport = true;
        };
      };
    in {
      whisper-ws = whisperWsPkg;
      strata = pkgs.callPackage ./machines/aibox/strata-package.nix {};
      strata-cuda = pkgsCuda.callPackage ./machines/services2/strata-package.nix {
        src = flakeInputs.strata.outPath;
        stdenv = pkgsCuda.cudaPackages_13_0.backendStdenv;
        cudaPackages = pkgsCuda.cudaPackages_13_0;
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
    # `cd crates/frigate-monitor/frontend && nix develop --command bash build.sh`
    # to iterate, or just `just build-frontend` (below).
    devShells.${system}.default = let
      pkgs = import nixpkgs {inherit system;};
      rust = pkgs.rust.packages.stable;
      CRANE = import ./common/crane.nix {
        inherit pkgs;
        crane = flakeInputs.crane;
      };
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
          # Nix formatter for the whole tree (`just fmt`); the repo standard is
          # alejandra — see .zed/settings.json and the justfile.
          pkgs.alejandra
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
