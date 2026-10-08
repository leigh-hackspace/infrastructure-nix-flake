# Shared crane helpers for the Rust crates in this flake.
#
# `pkgs.rustPlatform.buildRustPackage` recompiles the whole dependency tree on
# every source change, which is the slow part of iterating on these crates.
# Crane splits each build into a cached dependency derivation (keyed only on the
# Cargo.toml/Cargo.lock/.cargo config — the sources are replaced with stubs)
# plus the crate itself, so editing one .rs file only recompiles that crate
# against the cached artifacts.
#
# `crane` reaches modules through the flake's `specialArgs`, and this file is a
# plain function, not a module:
#
#   let CRANE = import ../common/crane.nix { inherit pkgs crane; };
#   CRANE.cached {
#     pname = "dns-sync";
#     version = "0.1.0";
#     src = ../../dns-sync;
#     cargoLock = CRANE.lockFile ../../dns-sync/Cargo.lock;
#   };
#
# Two differences from the old call style worth remembering:
#
# - crane takes `cargoLock` as a *path*, not nixpkgs' `cargoLock.lockFile`
#   attrset;
# - crane vendors the crates itself (per crate, fixed-output, cached) instead of
#   using `cargoDeps`, so no `fetchCargoVendor` hash needs maintaining.
{
  pkgs,
  crane,
}: let
  lib = pkgs.lib;
  craneLib = crane.mkLib pkgs;

  # Read a Cargo.lock at evaluation time and re-emit it as a content-keyed store
  # file. Handing crane a lock file that lives inside the crate's own source
  # directory makes the vendor and dummy-source derivations depend on the whole
  # source tree, so *any* edit (even a comment in a .rs file) invalidates the
  # dependency cache — exactly the rebuild we are trying to avoid. Keyed on the
  # lock contents, only a dependency change forces a rebuild.
  lockFile = p: pkgs.writeText "Cargo.lock" (builtins.readFile p);

  # --- shared in-repo crates (common-rs/) ------------------------------------
  #
  # Some crates share code through cargo *path* dependencies (`dns-sync` and
  # `moonraker-exporter` use `common-json`, `filestore` and
  # `gocardless-dashboard` use `common-oidc`).  The dependency is written
  # `path = "./common-rs/<name>"` and the crate directory carries a git-ignored
  # `common-rs` symlink to the repo's `common-rs/`, so `cargo build` works in
  # the crate's own directory like any other workspace.  A Nix build is handed
  # one crate's source tree, which cannot contain anything above the tree, so
  # the source passed to crane gets the real shared crates copied inside it —
  # the same trick the gocardless-dashboard frontend uses for its `dto`
  # dependency.
  #
  # The dependency build is given stub sources instead of the real shared code:
  # the path dependencies only have to resolve there, and keying the stubs on
  # the shared crates' manifests (read at eval time) means a dependency change
  # in a shared crate invalidates the cache while editing shared code does not.
  # Passing the real shared sources through the dependency build would make a
  # one-line change in common-rs recompile every vendored dependency.
  sharedCrateDir = name: "${../common-rs}/${name}";
  sharedCrateManifest = name: ../common-rs + "/${name}/Cargo.toml";

  # `src` with the named shared crates copied in (real sources).
  withSharedCrates = {pname, src, names}: let
    copies = lib.concatMapStringsSep "\n" (n: ''
      mkdir -p $out/common-rs
      cp -a ${sharedCrateDir n} $out/common-rs/${n}
    '') names;
  in
    pkgs.runCommand "${pname}-src" {} ''
      mkdir -p $out
      cp -a ${src}/. $out/
      # Store paths are read-only and cp -a preserves the mode, so make the
      # copy writable before adding anything to it.
      chmod -R u+w $out
      ${copies}
    '';

  # `src` with the named shared crates copied in as stubs: real manifests, empty
  # sources, for the dependency build only.
  withStubbedSharedCrates = {pname, src, names}: let
    stubs = lib.concatMapStringsSep "\n" (n: ''
      mkdir -p $out/common-rs/${n}/src
      cp ${pkgs.writeText "${pname}-${n}-stub-Cargo.toml" (builtins.readFile (sharedCrateManifest n))} \
        $out/common-rs/${n}/Cargo.toml
      : > $out/common-rs/${n}/src/lib.rs
    '') names;
  in
    pkgs.runCommand "${pname}-deps-src" {} ''
      mkdir -p $out
      cp -a ${src}/. $out/
      chmod -R u+w $out
      ${stubs}
    '';

  # `args.sharedCrates = ["oidc"]` selects the right source tree for the call:
  # real shared code for the crate build, stubs for the dependency build.
  resolveShared = {args, stub}: let
    names = args.sharedCrates or [];
    wrapped =
      if names == [] then args
      else if stub then removeAttrs (args // {
        src = withStubbedSharedCrates {inherit (args) pname src; inherit names;};
      }) ["sharedCrates"]
      else removeAttrs (args // {
        src = withSharedCrates {inherit (args) pname src; inherit names;};
      }) ["sharedCrates"];
  in wrapped;

  # The cached dependency derivation on its own. Only Cargo.toml, Cargo.lock and
  # any .cargo/config.toml feed it (the sources are stubbed), so it survives
  # every edit to the crate itself.
  deps = args: craneLib.buildDepsOnly (resolveShared {inherit args; stub = true;} // {doCheck = false;});

  # A crate built against cached deps. Pass `cargoArtifacts` explicitly when the
  # crate's own build has hooks that reference another derivation (e.g. a preBuild
  # pointing build.rs at a sibling SPA): those would otherwise become inputs of
  # the dependency build and invalidate the cache whenever that derivation
  # changes, even though cargo never sees them as dependencies.
  cached = args: craneLib.buildPackage (resolveShared {inherit args; stub = false;} // {doCheck = false;});

  # wasm-bindgen-cli pinned to 0.2.128 — the exact wasm-bindgen version every SPA
  # is compiled against (see the frontend Cargo.locks). The CLI and the
  # wasm-bindgen runtime inside the wasm must match exactly or the generated JS
  # glue is incompatible, and nixpkgs only ships older versions.
  #
  # This one still goes through fetchCargoVendor (hence the user-agent patch:
  # crates.io's API endpoint is blocked from our network, and nixpkgs' fetcher
  # sends a python-requests UA which crates.io refuses).
  wasmBindgenCliSrc = pkgs.fetchurl {
    name = "wasm-bindgen-cli-0.2.128.tar.gz";
    url = "https://static.crates.io/crates/wasm-bindgen-cli/wasm-bindgen-cli-0.2.128.crate";
    hash = "sha256-LikUDAToGDKQK3Dl03uc4b+oEcj+RWO+oI9234OIzyA=";
  };
  wasmBindgenCli = pkgs.buildWasmBindgenCli {
    version = "0.2.128";
    src = wasmBindgenCliSrc;
    cargoDeps = pkgs.rustPlatform.fetchCargoVendor {
      pname = "wasm-bindgen-cli";
      version = "0.2.128";
      src = wasmBindgenCliSrc;
      hash = "sha256-R1Tas33Ursy8kqsxguAkG0ZhNed2n5uFTAhw1l2qlLY=";
      preBuild = "source ${../common/cargo-vendor-ua-patch.sh}";
    };
  };

  # A Dioxus SPA compiled to wasm. cargo must target wasm32 (nixpkgs' rustc ships
  # no wasm linker, so wasm-ld comes from pkgs.lld) and the output is
  # post-processed by wasm-bindgen. Both the cached deps and the crate are built
  # for the wasm target, otherwise the cache would be for the wrong target.
  wasmSpa = {
    pname,
    version,
    src,
    cargoLock,
    # cargo's underscored package name, i.e. the .wasm file cargo emits
    wasmName,
  }:
    cached {
      inherit pname version src cargoLock;
      nativeBuildInputs = [wasmBindgenCli pkgs.lld];
      cargoBuildCommand = "cargoWithProfile build --target wasm32-unknown-unknown";
      preBuild = "export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld";
      installPhaseCommand = ''
        wasm-bindgen --target web --out-dir $out --no-typescript \
          target/wasm32-unknown-unknown/release/${wasmName}.wasm
        cp index.html $out/index.html
      '';
    };
in {
  inherit craneLib cached deps lockFile wasmBindgenCli wasmSpa;
}
