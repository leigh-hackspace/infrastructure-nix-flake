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
  craneLib = crane.mkLib pkgs;

  # Read a Cargo.lock at evaluation time and re-emit it as a content-keyed store
  # file. Handing crane a lock file that lives inside the crate's own source
  # directory makes the vendor and dummy-source derivations depend on the whole
  # source tree, so *any* edit (even a comment in a .rs file) invalidates the
  # dependency cache — exactly the rebuild we are trying to avoid. Keyed on the
  # lock contents, only a dependency change forces a rebuild.
  lockFile = p: pkgs.writeText "Cargo.lock" (builtins.readFile p);

  # The cached dependency derivation on its own. Only Cargo.toml, Cargo.lock and
  # any .cargo/config.toml feed it (the sources are stubbed), so it survives
  # every edit to the crate itself.
  deps = args: craneLib.buildDepsOnly (args // {doCheck = false;});

  # A crate built against cached deps. Pass `cargoArtifacts` explicitly when the
  # crate's own build has hooks that reference another derivation (e.g. a preBuild
  # pointing build.rs at a sibling SPA): those would otherwise become inputs of
  # the dependency build and invalidate the cache whenever that derivation
  # changes, even though cargo never sees them as dependencies.
  cached = args: craneLib.buildPackage (args // {doCheck = false;});

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
      preBuild = "source ${../frigate-monitor/cargo-vendor-ua-patch.sh}";
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
