# HELPER — function of {pkgs, crane} returning the crane build helpers; handed to modules as CRANE by common/crane-args.nix.
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
#     pname = "filestore";
#     version = "0.1.0";
#     member = "filestore";
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

  # A `Cargo.lock` handed to crane the way crane wants one: the file **and** its
  # contents, always as a pair.  Crane reads `cargoLock` during *evaluation*
  # (`vendorCargoDeps` does a `pathExists`/`readFile` on it), and `lockFile` returns a
  # derivation, so the read is a build — the failure this repo keeps paying for, and
  # the reason `just check` died on the router on 2026-10-22 with
  # `Cannot build '…-Cargo.lock.drv'`, on the one box that is right to refuse
  # (`nix.settings.max-jobs = 0`).  `cargoLockContents` is that same value pre-read, so
  # nothing has to realise anything.  Passing both is drv-neutral: `mkCargoDerivation`
  # prefers `cargoLock` when it is present, and `writeText "Cargo.lock" contents` is
  # precisely the path `lockFile` already made.
  #
  # Every site that gives crane a lock file goes through here, because a lock handed
  # over on its own is a build hiding in an evaluation, and only one of the three sites
  # was ever observed to fail.
  lockArgs = p: {
    cargoLock = lockFile p;
    cargoLockContents = builtins.readFile p;
  };

  # A vendored-dependency tree for a lock file that lives outside its source tree.
  #
  # Crane reads the lock file *at evaluation time* (`builtins.readFile cargoLock`)
  # and walks `src` for `.cargo/config.toml` files (`findCargoFiles src`).  Handed
  # plain paths both are free.  Handed a *derivation* — which is what `lockFile`
  # returns, and what `memberSrc`/`withSharedCrates` return — each of those reads
  # **realises the derivation**, so merely evaluating a machine's configuration
  # compiles `Cargo.lock` and every crate's `-src` stub instead of evaluating them.
  # Measured here 2026-10-18: a cold `nix flake check --no-build` died on
  # `path '…-frigate-monitor-src.drv' is not valid`, and evaluating aibox's config
  # took 56 s against 17 s for services1's because it built on the remote builder.
  #
  # Computing the vendor dir from the *plain* source directory and the lock's
  # contents short-circuits both reads.  The value is the one crane would have
  # computed anyway (no crate keeps a `.cargo/config.toml`, so `cargoConfigs` is
  # empty either way), which is why this is drv-neutral: `frigate-monitor`,
  # `switch-gui`, `filestore` and the `services1`/`services2`/`gw` toplevels keep
  # byte-identical store paths across it, so it costs no rebuild. (`aibox` and
  # `pi-room-sys` move on *any* tracked edit, which is a separate pre-existing
  # problem: `docs/follow-ups.md`.)
  vendorDirFor = {
    src,
    lockPath,
  }:
    craneLib.vendorCargoDeps {
      inherit src;
      cargoLock = lockFile lockPath;
      cargoLockContents = builtins.readFile lockPath;
    };

  # --- the native cargo workspace ---------------------------------------------
  workspaceRoot = ../crates;
  workspaceLockPath = workspaceRoot + "/Cargo.lock";

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
  workspaceVendorDir = vendorDirFor {
    src = workspaceSrc;
    lockPath = workspaceLockPath;
  };

  # The one dependency derivation every native crate builds against.
  #
  # `buildDepsOnly` replaces every member's sources with stubs, so this is keyed on the
  # workspace's manifests and lock file alone: no .rs edit reaches it.  Because the lock
  # file is shared there is exactly one store path here for the whole fleet, which is the
  # point — it is what stops each program recompiling its own copy of the dependency tree.
  #
  # `cargoVendorDir` is stated rather than left to crane for the reason given on
  # `lockArgs`: asked for neither a vendor dir nor the lock's contents, the dependency
  # build finds its own vendor tree by *reading* `cargoLock`, and this lock file is a
  # derivation.  `workspaceVendorDir` is that same tree, computed from the plain source
  # directory and the lock's contents (`vendorDirFor`), which is why stating it is free.
  #
  # The flip side of one lock file: changing a dependency *anywhere* invalidates it for
  # everyone. That is the price of a workspace and it is the same price cargo charges
  # locally.
  # `pname`/`version` are stated rather than discovered.  The workspace root is a
  # *virtual* manifest — `[workspace]` with members, no `[package]`, so no name and no
  # version to read — and crane, asked for neither, reads `src/Cargo.toml`, finds
  # nothing, and warns on every single evaluation that it is using a placeholder value.
  # They are only labels for the dependency derivation, which is the same stubbed-deps
  # tree for every member regardless of what any of them version themselves (the members
  # are 0.1.0 except frigate-monitor at 0.2.0, and neither number reaches it).
  workspaceArtifacts = craneLib.buildDepsOnly (lockArgs workspaceLockPath
    // {
      pname = "crates-workspace";
      version = "0.1.0";
      src = workspaceSrc;
      cargoVendorDir = workspaceVendorDir;
      doCheck = false;
    });

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
        cargoVendorDir = workspaceVendorDir;
        cargoBuildCommand = "cargoWithProfile build -p ${pname}";
        doCheck = false;
      }
      // lockArgs workspaceLockPath);

  # The source tree one SPA is built from: its own `frontend/` directory, plus the
  # crates its manifest reads as path dependencies — the `common-rs/` ones named in
  # `names`, and whatever `extra` maps by destination (a `dto` living inside the app's own
  # directory, which a developer reaches through a git-ignored symlink and a flake source
  # cannot carry at all — see common/shared-rs-links.sh).
  #
  # A derivation, and that is a cost rather than a choice, so it is written down. Crane
  # reads a crate's `src` while *evaluating* — `mkDummySrc` walks it with
  # `builtins.readDir` to find the manifests to stub, and its own comment calls this
  # "laundering the data at evaluation time" — so a derivation-shaped source is **built**
  # by anyone who merely evaluates a machine that installs the binary. Measured 2026-10-25
  # on a fresh flake source with nothing able to build (`--option builders '' --option
  # max-jobs 0`, which is the state `gw` keeps itself in), where the trace runs from a
  # unit's `ExecStart` straight into a build:
  #
  #     error: path '…-filestore-web-src.drv' is not valid
  #       … while evaluating attribute 'preBuild' of derivation 'filestore-0.1.0'
  #       … while evaluating attribute 'cargoArtifacts' of 'filestore-web-0.1.0'
  #       … while evaluating attribute 'src' of 'filestore-web-deps-0.1.0'
  #       … while adding path '…-filestore-web-src'
  #
  # and the same evaluation with `sharedCrates = []` — so the source is a plain directory
  # and there is nothing to add — passes. It is the third door into
  # docs/nix-dev-workflow.md's "Evaluation must not build", after the crate source stubs
  # and (now closed by `lockArgs`, which is why the error above names a `-src` and not a
  # `Cargo.lock`) the lock file — and, unlike those two, closing it is a decision rather
  # than a pairing: the path dependencies would have to stop being a copied tree, either
  # by moving them into the dependency derivation or by giving the SPA a source with no
  # workspace of its own. Both change what invalidates what, so neither belongs in a
  # simplification pass. What was tried and did not work: an **attrset** source is the
  # same bytes with no derivation behind them, but stdenv stringifies `src` — `cannot
  # coerce a set to a string: { "Cargo.lock" = «thunk»; … }` — so there is no third shape
  # to reach for. Measuring it is two flags on a box that has never seen the source
  # (`gw` keeps itself in that state); the recipe is in docs/nix-dev-workflow.md.
  withSharedCrates = {
    pname,
    src,
    names ? [],
    extra ? {},
  }: let
    copies =
      lib.concatMapStringsSep "\n" (n: ''
        mkdir -p $out/common-rs
        cp -a ${workspaceRoot}/common-rs/${n} $out/common-rs/${n}
      '')
      names;
    locals =
      lib.concatMapStringsSep "\n" (d: ''
        mkdir -p $out/${builtins.dirOf d.name}
        rm -rf $out/${d.name}
        cp -a ${d.value} $out/${d.name}
      '')
      (lib.attrsToList extra);
  in
    pkgs.runCommand "${pname}-src" {} ''
      mkdir -p $out
      cp -a ${src}/. $out/
      # Store paths are read-only and cp -a preserves the mode, so make the copy writable
      # before replacing anything in it.
      chmod -R u+w $out
      rm -rf $out/common-rs
      ${copies}
      ${locals}
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
  #
  # Nothing should call this directly: `spaApp` is the recipe, and it derives the
  # paths, the wasm package name and the DIST variable from an app's name.
  wasmSpa = {
    pname,
    version,
    src,
    # the lock file's path in the tree, NOT a store path: `lockArgs` turns it into the
    # content-keyed pair crane wants, including the contents so that nothing has to read
    # the derivation to get them (see `lockArgs`, `vendorDirFor`)
    cargoLockPath,
    # cargo's underscored package name, i.e. the .wasm file cargo emits
    wasmName,
    sharedCrates ? [],
    extraSources ? {},
  }: let
    resolved =
      if sharedCrates == [] && extraSources == {}
      then src
      else
        withSharedCrates {
          inherit pname src;
          extra = extraSources;
          names = sharedCrates;
        };
  in
    craneLib.buildPackage (lockArgs cargoLockPath
      // {
        inherit pname version;
        # computed from `src`, and not from `resolved`: the vendor tree is a function of
        # the lock file and the crate's own `.cargo` config, and must not be invalidated
        # by the path dependencies assembled around it
        cargoVendorDir = vendorDirFor {
          inherit src;
          lockPath = cargoLockPath;
        };
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
      });

  # --- one SPA and the binary that embeds it ---------------------------------------
  #
  # The shape every browser-served app in this fleet shares, so that adding one is a
  # name, a version and its crate lists.  `name` is the directory under `crates/`, the
  # cargo package name and the workspace member — the same string for all four — and the
  # SPA's directory, its lock file, its underscored wasm target and the environment
  # variable its `build.rs` reads are all derived from it.
  #
  # Two crate lists, because an SPA and its binary do not read the same shared crates:
  # `spaSharedCrates` are copied into the *frontend's* source tree (a manifest path
  # dependency), `sharedCrates` are the ones the *binary's* manifest reads (see `crate`).
  # `localCrates` are the crates that live inside the app's own directory — a `dto` of
  # its own — which a developer reaches through a git-ignored symlink and a flake source
  # cannot carry at all.
  #
  # Why the bundle is threaded through `preBuild` rather than being a cargo dependency:
  # `build.rs` embeds it, so the SPA is a *build-hook* input, and the dependency
  # derivation is the workspace-wide one, which never sees these hooks. That is what
  # keeps a SPA-only change from rebuilding everybody's dependency tree.
  spaApp = {
    name,
    version,
    sharedCrates ? [],
    spaSharedCrates ? [],
    localCrates ? [],
    distVar ? lib.toUpper (builtins.replaceStrings ["-"] ["_"] name) + "_DIST",
  }: let
    underscored = builtins.replaceStrings ["-"] ["_"] name;
    dist = wasmSpa {
      pname = "${name}-web";
      inherit version;
      src = workspaceRoot + "/${name}/frontend";
      cargoLockPath = workspaceRoot + "/${name}/frontend/Cargo.lock";
      wasmName = "${underscored}_web";
      sharedCrates = spaSharedCrates;
      extraSources = lib.genAttrs localCrates (c: workspaceRoot + "/${name}/${c}");
    };
    binary = crate {
      pname = name;
      inherit version;
      member = name;
      inherit sharedCrates;
      # Point build.rs at the nix-built bundle instead of the (uncommitted, and in a
      # Nix build nonexistent) frontend/dist in the source tree.
      preBuild = "export ${distVar}=${dist}";
    };
  in {
    inherit binary dist;
  };
in {
  inherit
    craneLib
    crate
    lockArgs
    lockFile
    spaApp
    spaMembers
    vendorDirFor
    wasmBindgenCli
    wasmSpa
    workspaceArtifacts
    workspaceRoot
    workspaceSrc
    ;
}
