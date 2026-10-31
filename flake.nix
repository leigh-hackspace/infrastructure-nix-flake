{
  inputs = {
    # Every input here is a network-wide source and every one follows `nixpkgs`; the
    # one that did not is written up at `door-entry-management-system` below.  There are
    # deliberately no commented-out alternative URLs in this block — they read as pins
    # and do nothing.  To try one of these trees from a local clone for a single
    # command, say so on the command line instead:
    #
    #   nix flake check --no-build --override-input nixos-utils path:/home/leigh-admin/Projects/nixos-utils
    #
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    # Rust build helpers (cached dependency derivations); see common/crane.nix.
    crane.url = "github:ipetkov/crane";

    nixos-utils = {
      url = "github:cjdell/nixos-utils";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # The door entry system's management server (deployed on services1: its
    # backend + frontend units).  It is an input like every other source here, which
    # means it follows this flake's nixpkgs: without the follows line it drags in its
    # *own* nixpkgs node — one measured 435 MB fetched at *evaluation* time, fourteen
    # months behind the network's, unmovable by `just update-pkgs`.  With it the whole
    # flake evaluates on one stdenv and one lock.
    door-entry-management-system = {
      url = "github:leigh-hackspace/door-entry-system?dir=management-system";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    llama-cpp = {
      url = "github:ifm-ai/llama.cpp/model/K2Horizon";
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

    # Strata (https://github.com/Niko1221/Strata), the ggml-based engine that serves
    # Qwen3.8-Flash-Next across GPU VRAM, RAM and the CPU.  This input is the source
    # of **both** engines: services2's CUDA one (machines/services2/strata-package.nix)
    # and, since 2026-10-28, aibox's HIP/gfx1030 one (machines/aibox/strata-package.nix)
    # build from its outPath.  Upstream's own flake ships only a devShell, so both are
    # packaged by hand here, and the input is tracked on `main` on purpose — the
    # engine is the bleeding edge and this network wants the RDNA2/CUDA fixes as they
    # land.  (`aibox's package used to pin its own fetchFromGitHub, which is why
    # `nix flake update strata` once moved one engine and not the other; the nine-
    # release gap that produced is what the 2026-10-28 un-parking closed.)  The
    # engine itself, the pin and what a bump involves: machines/services2/STRATA.md
    # and machines/aibox/STRATA.md.  The `follows` line is the rule in the door-entry
    # note above, and it is the rule for every input here.
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

    # Make `nix registry query nixpkgs` answer with this flake's nixpkgs rather than
    # with whatever channel the box happens to have.  (It is named after the module it
    # feeds, not after `nix-shell`, which nobody here runs.) A plain module, and not
    # part of extraArgs: specialArgs is what modules read as arguments, so mkSystem
    # refers to it by name here rather than repeating it per machine.
    useFlakeNixpkgsAsRegistry = {nix.registry.nixpkgs.flake = nixpkgs;};

    # Helper sets handed to every module through specialArgs (modules read them as
    # `INFRA`), so shared helpers are not re-imported in each file — plus
    # `flakeInputs` itself, which is what lets `machines/<name>/default.nix` be an
    # ordinary module.
    #
    # It used to be curried: each machine directory was `flakeInputs: {…}`, applied at
    # its call site below with `((import ./machines/services1) flakeInputs)`.  The only
    # thing the four call sites needed it for was
    # `flakeInputs.sops-nix.nixosModules.sops` in their `imports` — the dashed
    # attribute name is not a legal function-argument pattern, so it could not come in
    # as an ordinary module argument — and the signature was not even consistent:
    # `machines/aibox/default.nix` was `flakeInputs: {config, pkgs, ...}: {…}`, a module
    # that was a function returning a module, which is what a newcomer copying that
    # file would have reproduced.  As a value in specialArgs it arrives by name.
    extraArgs = {
      INFRA = import ./common/systemd.nix {lib = nixpkgs.lib;};
      flakeInputs = flakeInputs;
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

    /*
    The five machines, as one function and five short calls (§2.1 of
    docs/flake-simplification-audit-2026-10-22.md).

    Before this there were five copies of `nixpkgs.lib.nixosSystem`, nine
    `import nixpkgs`, six `config.allowUnfree = true` and three verbatim copies of
    the shared module list.  The cost was not length, it was that the one fact worth
    reading in that whole section — **what the router deliberately does not get** —
    was a 30-line comment next to a list you had to diff by eye against another list
    to confirm.  It is now the `modules` of that machine's call, and the shared half
    has a name.

    `commonModules` is what every box that hosts services gets; `routerModules` is
    the router's, spelled out rather than filtered out of the other, because its
    differences are the interesting part and a filter would hide them:

      * no `./common/nas.nix` and no `./common/tools.nix`: the router is the thing
        the network's DNS depends on, so it must not grow a dependency on a NAS that
        is still importing after a power cut.  tools.nix's wait-for-network unit
        reads `config.infra.nas.host`, which only exists with nas.nix, so the two are
        added and removed together — which is the check that keeps them out.
      * no `./common/containers.nix` and no `nixos-utils`' containers module: a
        router runs dnsmasq, pppd, openvpn, tailscale, nft and its own small
        dashboard (machines/gw/network-status.nix), and nothing else.
      * `./common/build-farm.nix` **is** imported, with `gc = false`: gw delegates
        its builds to aibox (machines/gw/default.nix) so that an accidental
        `nix build`/`nix develop` cannot compile on the gateway, and the garbage
        collection half stays off because a router that starts walking and rewriting
        its own store on a timer is a second opinion about the rollback profile
        nobody asked for.
      * `./common/crane-args.nix` is here because that dashboard is a Rust crate; the
        crate is built on aibox either way — by `just deploy gw`, or by a `just switch`
        on the box, which is the same thing.

    Module *order* is merge order, so these lists are behaviour and not formatting:
    each machine's list is in the order the five copies used to spell it out, and the
    check that they still are is that every machine's `system.build.toplevel` drvPath
    is byte-identical across the change (docs/nix-dev-workflow.md, "proving a change
    is drv-neutral").
    */
    commonModules = [
      ./common/sops.nix
      ./common/containers.nix
      ./common/crane-args.nix
      ./common/build-farm.nix
      ./common/nas.nix
      ./common/tools.nix
      ./common/users.nix
    ];

    routerModules = [
      ./common/sops.nix
      ./common/users.nix
      ./common/crane-args.nix
      ./common/build-farm.nix
    ];

    mkSystem = {
      modules,
      # e.g. `{cudaSupport = true;}` for the box with the RTX 3060 — which used to be
      # the whole reason services2 was a duplicate block.
      pkgsConfig ? {},
      # A system's own argument to its modules, for the one case a module cannot read
      # any other way: aibox needs the netboot image's config, and an attrset cannot
      # refer to its own sibling in `nixosConfigurations`.
      extraArgs' ? {},
      # The netboot image wants neither.  Its root is a tmpfs over a squashfs mounted
      # off aibox and there is no profile to mark good, so `nixos-utils`' rollback
      # module would be a timer that reboots a client into a generation that does not
      # exist — and nothing runs `nix registry` on a PXE boot either.
      registry ? true,
      rollback ? true,
    }:
      nixpkgs.lib.nixosSystem {
        inherit system;
        pkgs = import nixpkgs {
          inherit system;
          config =
            {
              allowUnfree = true;
            }
            // pkgsConfig;
        };
        specialArgs = flakeInputs // extraArgs // extraArgs';
        modules =
          nixpkgs.lib.optionals registry [useFlakeNixpkgsAsRegistry]
          ++ nixpkgs.lib.optionals rollback [nixos-utils.nixosModules.rollback]
          ++ modules;
      };

    # The Pi Room computers' image: a machine, but one nobody switches.  It is a
    # nixosConfiguration like any other in here, and it is *built by* aibox rather
    # than deployed — machines/aibox/netboot.nix serves its kernel, initrd,
    # kernel params and squashfs store over iPXE + NFS, and reads this value out
    # of specialArgs to do it.  So it is a `let` binding and not an entry of
    # `nixosConfigurations` directly: aibox's config needs it, and an attrset
    # cannot refer to its own sibling.
    #
    # `nfsServer` is the address the *image* mounts its store from and it is an IP
    # for the reason written next to it in the file that came with this tree ("DNS
    # is too slow"); the iPXE menu that boots the image uses aibox's hostname, and
    # that string lives in machines/aibox/netboot.nix.
    piRoomSys = mkSystem {
      registry = false;
      rollback = false;
      modules = [
        ./machines/pi-room-sys/configuration.nix
        ((import ./machines/pi-room-sys/netboot.nix) {nfsServer = "10.3.1.32";})
        {
          environment.systemPackages = [nix-software-center.packages.${system}.nix-software-center];
        }
      ];
    };
  in {
    # `nix fmt` / `nix fmt -- --check` — the formatter is a flake output, and it comes
    # from this flake's pinned nixpkgs and from the binary cache.  Keeping it a flake
    # output rather than a devShell package is what lets `just fmt` run anywhere: the
    # wasm toolchain in `devShells.wasm` is *built* with cargo, and a formatter must
    # never be the reason a box — especially the router — compiles something.
    formatter.${system} = pkgs.alejandra;

    nixosConfigurations = {
      services1 = mkSystem {
        modules = [nixos-utils.nixosModules.containers] ++ commonModules ++ [./machines/services1];
      };

      # The second services box: a clone of services1 on AMD/Radeon hardware.  It
      # runs its own status dashboard and (since the RTX 3060 arrived) the Strata
      # CUDA engine — and nothing else, because its 30 GB of RAM is strata's: the
      # observability stack that used to live here (gatus, Prometheus, Grafana)
      # moved to services1 on 2026-10-17.  It was this network's build machine
      # until 2026-10-17; that is aibox now (common/build-farm.nix), so this box
      # keeps its RAM for the engine.  It is still measured — by services1's
      # Prometheus and by gatus from here.  See machines/services2/README.md for
      # what runs and what is left to measure it, and machines/services2/STRATA.md
      # for the engine.
      #
      # `pkgsConfig` is the only reason this machine used to be a duplicate block:
      # the RTX 3060 runs Strata's CUDA engine, and `cudaSupport` makes the splayed
      # cudaPackages_13_0 the toolkit the box builds against (and pulls the CUDA
      # stdenv in without a per-package override everywhere).  It gets no
      # `nixos-utils.nixosModules.containers` — nothing here runs a container the
      # other boxes do not.
      services2 = mkSystem {
        pkgsConfig = {cudaSupport = true;};
        modules = commonModules ++ [./machines/services2];
      };

      aibox = mkSystem {
        # `piRoomSys` because this box serves that image: machines/aibox/netboot.nix
        # reads its build tree (kernel, initrd, kernel params, squashfs store) and
        # its boot logo.  It is passed in rather than imported because it is a
        # nixosSystem, not a module.
        extraArgs' = {inherit piRoomSys;};
        modules =
          [nixos-utils.nixosModules.containers]
          ++ commonModules
          ++ [
            # whisper-ws and the llama.cpp builds enter this box's package set here;
            # `packages.whisper-ws` below is the same derivation, so it can also be
            # built on its own without the whole system.
            {
              nixpkgs.overlays = [
                (final: prev: {
                  # llama-cpp-leigh-rocm = llama-cpp.packages.${system}.rocm;
                  llama-cpp-leigh-vulkan = llama-cpp.packages.${system}.vulkan;
                  # llama-cpp-cpu = llama-cpp.packages.${system}.default;
                  whisper-ws = whisperWsPkg;
                })
              ];
            }
            ./machines/aibox
          ];
      };

      # The Pi Room netboot image.  Here so it is evaluated and built by this
      # flake like every other machine — `nixos-rebuild --flake .#pi-room-sys`
      # would not do anything to a host, because there is no host: aibox boots
      # these machines over iPXE and they mount their store off aibox at runtime.
      # The why of the whole thing is in machines/pi-room-sys/README.md, the
      # `registry`/`rollback` flags of which are in mkSystem above.
      pi-room-sys = piRoomSys;

      # The router: the Supermicro X9SCL/X9SCM at 10.3.1.1 (`common/hosts.nix`'s
      # `router`), cut over from the OPNsense appliance on 2026-10-21 and described
      # in machines/gw/README.md, with dnsmasq generated from common/vlans.nix +
      # common/dns-records.nix + common/int-dns.nix and nftables from
      # common/config.nix.  What it does not get, and the two reasons, are in the
      # `routerModules` comment above: no NAS dependency on the box DNS waits for,
      # and no container engine on the box that runs dnsmasq.
      gw = mkSystem {
        modules = routerModules ++ [./machines/gw];
      };
    };

    # `nix build .#strata` - the Strata engine (HIP/gfx1030) plus its Python
    # serve layer and the one-time model-prep tools: what machines/aibox/strata.nix
    # installs as a service.  That file and ./machines/aibox/STRATA.md document the
    # hand-run model download/pack/MTP steps this package deliberately keeps out of
    # the store, and what a bump has to re-check (the ggml pin, `data/`, the serve
    # layer's import list, the gfx1030 gate).
    #
    # `nix build .#strata-cuda` - the same engine built for services2's RTX
    # 3060 (CUDA 13.0 / sm_86), from the bleeding-edge `strata` flake input.
    #
    # `nix build .#rocm10-runtime` - the ROCm 10.0.0 (TheRock) user runtime aibox's
    # engine can be told to load instead of nixpkgs' ROCm 7.2: an 8 GB image layer
    # from Docker Hub, filtered to `lib/`.  Read the header of
    # ./machines/aibox/rocm10-runtime.nix — including its size — before building it
    # anywhere that is not this network's builder.
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
      strata = pkgs.callPackage ./machines/aibox/strata-package.nix {
        src = flakeInputs.strata.outPath;
      };
      strata-cuda = pkgsCuda.callPackage ./machines/services2/strata-package.nix {
        src = flakeInputs.strata.outPath;
        stdenv = pkgsCuda.cudaPackages_13_0.backendStdenv;
        cudaPackages = pkgsCuda.cudaPackages_13_0;
      };
      rocm10-runtime = pkgs.callPackage ./machines/aibox/rocm10-runtime.nix {};
    };

    /*
    The four hand-written assertion files, as flake checks (§2.6).

    They are plain functions rather than modules — `nix eval --impure --raw -f
    <file>` runs them, which is what `just check` does because that form *prints*
    which record is wrong — and until now nothing but that recipe ran them, so CI or
    a stranger typing `nix flake check` got a green light on a tree whose network
    tables disagreed.

    Each one returns the string `"ok\n"` or throws with the finding, and the check
    below relies on that: forcing the value at *evaluation* is the test, so the check
    runs under `nix flake check --no-build` (no derivation is built and none needs
    to be — the derivation is one line of text, and on the router that difference is
    the whole rule about not compiling here).  The four entry points are then:
    `nix flake check` (this), `just check` (the same calls, with the message), and —
    for the network tables only — the module assertion in
    `machines/gw/lib/assert-network-tables.nix`, which is what stops a router being
    *built* from tables that disagree.  `"ok\n"` is the seam between them: if a
    checker ever returns the failures as a list instead of throwing, all of the
    comparisons change together.
    */
    checks.${system} = let
      # Force the checker and wrap its verdict in something `nix flake check` accepts.
      asCheck = name: report:
        assert (report == "ok\n");
          pkgs.runCommand "check-${name}" {__evalCheck = true;} ''
            printf '%s\n' '${name}: assertions hold' > $out
          '';
    in {
      # common/vlans.nix <-> common/dns-records.nix <-> the ACL in common/config.nix:
      # a VLAN id that is not in the first table or one address claimed twice is a
      # client that gets the wrong address rather than a build failure.
      network-tables = asCheck "network-tables" (import ./machines/gw/lib/check-network-tables.nix);

      # The klippy/print state tables read out of crates/moonraker-exporter still match
      # the Grafana value mappings and the Prometheus alert numbers generated from them.
      printer-states = asCheck "printer-states" (import ./common/check-printer-states.nix);

      # The Nix -> GITLAB_OMNIBUS_CONFIG serializer.  It covers a **parked** module
      # (gitlab.nix is not imported), so this is the one check in here that is green
      # about something the network does not run — see `just check`'s own note and
      # docs/flake-simplification-audit-2026-10-22.md §6.
      config-to-gitlab = asCheck "config-to-gitlab" (import ./machines/services1/lib/check-config-to-gitlab.nix);

      # The instructions themselves: every `just <name>`, repo path and docs link in
      # AGENTS.md/README.md/justfile resolves to something that exists, and AGENTS.md
      # never describes the router as a shadow.  The one check here that can go red with
      # no code touched at all — which is exactly how `just dns-sync-sync` survived in
      # rule 14 for months after the recipe was deleted.
      doc-pointers = asCheck "doc-pointers" (import ./common/check-doc-pointers.nix);
    };

    # `nix develop`
    #
    /*
    Two shells, split by the one thing that has to be *compiled* (§2.4).

    `default` builds nothing: every package in it is substituted from the binary
    cache, including the whole Rust toolchain (the stable `rust-bin` set is prebuilt,
    and `lld` is nixpkgs').  `wasm` adds `CRANE.wasmBindgenCli`, which is the pinned
    wasm-bindgen 0.2.128 the four Dioxus bundles need and — because it is a cargo
    build of a crates.io tree — the only package in this file that compiles.  On a box
    that has never built a SPA, `nix develop` used to mean "compile Rust": fine on
    aibox, and on 2026-10-21 it put four `rustc` processes on gw, started by
    `just check` → `fmt-check` → `nix develop`, on the box that answers DNS for the
    whole network.

    gw delegating its builds and running `nix.settings.max-jobs = 0` closed that
    particular accident; the shell stays split because the point was never where the
    compile happens, it is that "give me a shell" should not be a build at all.  So:
    `nix develop` for cargo/rustc/clippy/just/alejandra/node, and
    `nix develop .#wasm` — which the SPA recipes in the justfile ask for by name —
    when you are building a bundle.  Note this does not make
    `nix flake check` cheap: it still builds `packages.strata`, `strata-cuda`,
    `whisper-ws` and any devShell it is given, which is why the gate in this repo is
    `just check` (`--no-build`) and not the bare flake idiom.
    */
    devShells.${system} = let
      rust = pkgs.rust.packages.stable;
      CRANE = import ./common/crane.nix {
        inherit pkgs;
        crane = flakeInputs.crane;
      };
      # In-repo path dependencies (common-rs/, frontend/dto) are git-ignored symlinks;
      # cargo needs them, the Nix build does not.  Run the working-tree copy, not the
      # one copied into the shell's store path.
      linkHook = ''
        if [ -f ./common/shared-rs-links.sh ]; then
            bash ./common/shared-rs-links.sh >/dev/null
        fi
      '';
      basePackages = [
        rust.rustc
        rust.cargo
        rust.rustfmt
        rust.clippy
        pkgs.lld
        pkgs.just
        pkgs.git
        pkgs.jq
        # Nix formatter for the whole tree (`just fmt`); the repo standard is
        # alejandra — see .zed/settings.json and the justfile.  `nix fmt` takes it from
        # the flake's `formatter` instead, which is why `just fmt` and `just check`
        # work with no shell at all.
        pkgs.alejandra
        # node is needed by the headless-browser test suites (Playwright is driven by node;
        # its browser binaries come from nixpkgs, not from Playwright's own download).
        pkgs.nodejs
        # handy for debugging (zip/struct inspection of generated archives, etc.)
        pkgs.python3
      ];
    in {
      default = pkgs.mkShell {
        packages = basePackages;
        shellHook = linkHook;
      };

      # The SPA build toolchain: everything in `default` plus the *pinned* wasm
      # bindgen.  Not `pkgs.wasm-bindgen-cli`: the SPAs are pinned to wasm-bindgen
      # 0.2.128 in their Cargo.locks and the bindgen *schema* must match the CLI
      # version exactly, so nixpkgs' 0.2.121 would refuse to process the output.  This
      # is the same derivation common/crane.nix uses to build the bundles the flake
      # embeds, which is also why the local `dist/` and the deployed one agree — and
      # why entering this shell on a cold box compiles for a few minutes.
      #
      # The stable toolchain already ships the wasm32-unknown-unknown std library, so
      # `cargo build --target wasm32-unknown-unknown` needs no rustup and no network;
      # `lld` above is the wasm-ld.  `dist/` is git-ignored and rebuilt by the flake:
      # `just build-frontend [crate]`, or
      # `cd crates/frigate-monitor/frontend && nix develop .#wasm --command bash build.sh`.
      wasm = pkgs.mkShell {
        packages = basePackages ++ [CRANE.wasmBindgenCli];
        shellHook = linkHook;
      };
    };
  };
}
