# Shared crane helpers for the Rust crates in this flake.
#
# `pkgs.rustPlatform.buildRustPackage` recompiles the whole dependency tree on
# every source change, which is the slow part of iterating on these crates.
# Crane splits a build into a cached dependency derivation (keyed only on the
# Cargo.toml/Cargo.lock/.cargo config — the sources are replaced with stubs)
# plus the crate itself, so editing one .rs file only recompiles that crate
# against the cached artifacts.
#
# `crane` and the helpers in this file reach modules through the flake's
# `specialArgs` as `CRANE` (imported once in flake.nix) — modules do not
# re-import it themselves:
#
#   CRANE.crate {
#     pname = "dns-sync";
#     version = "0.1.0";
#     member = "dns-sync";
#   };
#
# ## Everything native is one cargo workspace
#
# `crates/` is a single cargo workspace (crates/Cargo.toml), so there is one lock
# file and one dependency graph for all eight programs.  That has two consequences
# here, and the helpers exist to keep the second one from costing anything:
#
# 1. there is one dependency derivation for the whole fleet (`workspaceArtifacts`):
#    before the workspace, filestore, gocardless-dashboard, frigate-monitor and
#    switch-gui each compiled their own copy of reqwest, because each had its own
#    lock file and therefore its own vendor dir.  Now it is compiled once;
# 2. cargo loads *every* member's manifest to build any of them (and refuses a
#    manifest whose target files are missing), so a member cannot be built from its
#    own directory — the source tree has to carry the whole workspace.  Naively
#    that would mean every .rs edit rebuilds all eight programs.  `memberSrc`
#    avoids it: the tree carries the other members as crane's *stubs* (manifests
#    only, which is all cargo needs from them) and the real code of just the member
#    being built, so a crate's derivation still depends on that crate's source and
#    nothing else.
#
# The wasm SPAs under `crates/<app>/frontend` are deliberately *not* part of the
# workspace (they are built for another target against a pinned wasm-bindgen; the
# reason is written out in crates/Cargo.toml), so they keep their own lock files,
# their own dependency derivation, and the `sharedCrates` copy trick — see
# `wasmSpa`.
#
# Two differences from the pre-crane call style worth remembering:
#
# - crane takes `cargoLock` as a *path*, not nixpkgs' `cargoLock.lockFile` attrset;
# - crane vendors the crates itself (fixed-output, cached) instead of using
#   `cargoDeps`, so no `fetchCargoVendor` hash needs maintaining.
{
  pkgs,
  crane,
}: let
  lib = pkgs.lib;
  craneLib = crane.mkLib pkgs;

  # Read a Cargo.lock at evaluation time and re-emit it as a content-keyed store
  # file. Handing crane a lock file that lives inside the source directory makes
  # the vendor and dummy-source derivations depend on the whole source tree, so
  # *any* edit (even a comment in a .rs file) invalidates the dependency cache —
  # exactly the rebuild we are trying to avoid. Keyed on the lock contents, only a
  # dependency change forces a rebuild.
  lockFile = p: pkgs.writeText "Cargo.lock" (builtins.readFile p);

  # --- the native cargo workspace ---------------------------------------------
  workspaceRoot = ../crates;

  # The SPAs, relative to the workspace root.  They are excluded from the native
  # source tree: not workspace members, built for wasm32 with their own lock file,
  # and a change to a SPA manifest must not invalidate the native dependency cache.
  spaMembers = [
    "filestore/frontend"
    "frigate-monitor/frontend"
    "gocardless-dashboard/frontend"
    "switch-gui/frontend"
  ];

  # Non-members that are not SPAs, same reason for the filter here: `whisper-ws`
  # is a native crate but not a workspace member (its generated src/assets.rs
  # means it cannot compile as one — see the header of its package.nix), and it is
  # its own cargo workspace, so `cargo build` inside it writes a `target/` here
  # that `cleanSourceFilter` would otherwise copy into every crate's source tree
  # (it does not read .gitignore — the same trap as crates/target/, and the reason
  # this list is a filter rather than a comment).
  nonMemberPrograms = ["whisper-ws"];

  relativeToWorkspace = path: lib.removePrefix (toString workspaceRoot + "/") (toString path);

  workspaceSrc = lib.cleanSourceWith {
    src = workspaceRoot;
    filter = path: _type: let
      rel = relativeToWorkspace path;
    in
      !lib.any (d: rel == d || lib.hasPrefix (d + "/") rel) (spaMembers ++ nonMemberPrograms);
  };

  workspaceLock = lockFile (workspaceRoot + "/Cargo.lock");

  # The one dependency derivation every native crate builds against.
  #
  # `buildDepsOnly` replaces every member's sources with stubs, so this is keyed on
  # the workspace's manifests and lock file alone: no .rs edit reaches it.  Because
  # the lock file is shared there is exactly one store path here for the whole
  # fleet, which is the point — it is what stops each program recompiling its own
  # copy of the dependency tree.
  #
  # The flip side of one lock file: changing a dependency *anywhere* invalidates it
  # for everyone. That is the price of a workspace and it is the same price cargo
  # charges locally.
  # `pname`/`version` are stated rather than discovered.  The workspace root is a
  # *virtual* manifest — `[workspace]` with members, no `[package]`, so no name and
  # no version to read — and crane, asked for neither, reads `src/Cargo.toml`, finds
  # nothing, and warns on every single evaluation that it is using a placeholder
  # value.  They are only labels for the dependency derivation, which is the same
  # stubbed-deps tree for every member regardless of what any of them version
  # themselves (the members are 0.1.0 except frigate-monitor at 0.2.0, and neither
  # number reaches it).
  workspaceArtifacts = craneLib.buildDepsOnly {
    pname = "crates-workspace";
    version = "0.1.0";
    src = workspaceSrc;
    cargoLock = workspaceLock;
    doCheck = false;
  };

  # The source tree one member is built from: the workspace with stub sources
  # everywhere except `member` (and the shared crates it reads).
  #
  # The stubs are not a shortcut around cargo's rules, they *are* the rules — the
  # workspace manifest has to list every member and each member's manifest has to
  # parse, but nothing requires the members that are not being compiled to have any
  # code.  Keeping them as stubs is what preserves per-crate build caching now that
  # the crates share a lock file.
  memberSrc = {
    pname,
    member,
    sharedCrates ? [],
  }: let
    copies =
      lib.concatMapStringsSep "\n" (c: ''
        rm -rf $out/common-rs/${c}
        mkdir -p $out/common-rs
        cp -a ${workspaceRoot}/common-rs/${c} $out/common-rs/${c}
      '')
      sharedCrates;
  in
    pkgs.runCommand "${pname}-src" {} ''
      mkdir -p $out
      cp -a ${craneLib.mkDummySrc {src = workspaceSrc;}}/. $out/
      # Store paths are read-only and cp -a preserves the mode, so make the copy
      # writable before replacing anything in it.
      chmod -R u+w $out
      rm -rf $out/${member}
      cp -a ${workspaceRoot}/${member} $out/${member}
      ${copies}
    '';

  # A workspace member built against the shared cached deps.
  #
  # `member` is the path under `crates/`; `pname` is the cargo package name passed
  # to `cargo build -p`, which selects this program out of the workspace.  They are
  # the same string for every crate here, but saying both keeps the two jobs
  # separate if that ever stops being true.
  #
  # `sharedCrates` names the `common-rs/` crates the member reads by path: they are
  # already in the tree as stubs (enough for cargo to resolve them), and this is
  # what swaps in the real code.  A member that reads none needs nothing.
  #
  # Pass `cargoArtifacts` only to override the shared dependency derivation. It is
  # worth knowing about: a build hook that references another derivation (e.g. a
  # preBuild pointing build.rs at a sibling SPA) is *not* an input to the
  # dependencies here, so a SPA-only change never rebuilds a binary's deps.
  #
  # `doCheck = false`: the unit tests run in `just test` / `just clippy` (the
  # whole workspace in one pass), not inside the Nix build, which keeps a deploy
  # from failing on a test and the dependency cache independent of the test tree.
  crate = {
    pname,
    version,
    member,
    sharedCrates ? [],
    cargoArtifacts ? workspaceArtifacts,
    ...
  } @ args:
    craneLib.buildPackage (removeAttrs args [
        "member"
        "sharedCrates"
        "cargoArtifacts"
      ]
      // {
        inherit pname version cargoArtifacts;
        src = memberSrc {
          inherit pname member sharedCrates;
        };
        cargoLock = workspaceLock;
        cargoBuildCommand = "cargoWithProfile build -p ${pname}";
        doCheck = false;
      });

  # --- the wasm SPAs ------------------------------------------------------------
  #
  # A SPA is its own cargo workspace with its own lock file, so it gets its own
  # dependency derivation, and its shared path dependencies have to be copied into
  # its source tree: the git-ignored symlink a developer has locally
  # (`frontend/dto -> ../dto`, see common/shared-rs-links.sh) cannot travel in a
  # flake source, and a symlink outside the tree could not be copied into the store
  # either way.
  #
  # `sharedCrates` here names crates under `common-rs/`; a crate that lives inside
  # the app's own tree (a `dto`) is copied by the module that builds it, as before.
  withSharedCrates = {
    pname,
    src,
    names,
  }: let
    copies =
      lib.concatMapStringsSep "\n" (n: ''
        mkdir -p $out/common-rs
        cp -a ${workspaceRoot}/common-rs/${n} $out/common-rs/${n}
      '')
      names;
  in
    pkgs.runCommand "${pname}-src" {} ''
      mkdir -p $out
      cp -a ${src}/. $out/
      # Store paths are read-only and cp -a preserves the mode, so make the
      # copy writable before adding anything to it.
      chmod -R u+w $out
      rm -rf $out/common-rs
      ${copies}
    '';

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
    sharedCrates ? [],
  }: let
    names = sharedCrates;
    resolved =
      if names == []
      then src
      else
        withSharedCrates {
          inherit pname src names;
        };
  in
    craneLib.buildPackage {
      inherit pname version cargoLock;
      src = resolved;
      doCheck = false;
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
  inherit
    craneLib
    crate
    lockFile
    spaMembers
    wasmBindgenCli
    wasmSpa
    workspaceArtifacts
    workspaceRoot
    workspaceSrc
    ;
}
