# `voice-wake` as a derivation: a workspace of one, built the `whisper-ws` way.
#
# Why this file exists at all (and why the crate is not a member of `crates/`) is
# written at the top of `crates/Cargo.toml`, in one sentence worth repeating here
# because it is the entire reason for the shape of this file: the workspace has one
# `Cargo.lock` and crane reads its **contents at evaluation**, so a member gaining a
# dependency dirties the vendored tree — and therefore the build — of every service on
# every box.  An ONNX runtime has no business in `switch-gui`'s dependency derivation.
#
# So: its own `Cargo.lock`, its own vendored tree, `rustPlatform.buildRustPackage`
# rather than the shared crane helpers (the precedent is `crates/whisper-ws/package.nix`,
# which opts out for a different reason and builds the same way), and — because this one
# *has* external dependencies, unlike `whisper-ws` — a `cargoHash` that comes from
# cargo's own resolution rather than from `crane`'s `vendorCargoDeps`.
#
# Two traps this file has to step around, both learned elsewhere in this repo:
#
# * **`target/` must not be an input.** `cleanSourceFilter` does not read `.gitignore`,
#   so `src = ./.` here would stream 660 MB of cargo artifacts into the store for every
#   build of this one binary.  The filter below names `target` out loud; the `.gitignore`
#   entry beside it is for humans, not for nix.
# * **`cargoHash` is content-keyed on the lock file**, so a dependency bump re-runs the
#   fixed-output fetch and nothing else; and it is a *fixed output*, which is what
#   licenses the network access that the build phase must not need.
#
# The ONNX models are deliberately **not** in this derivation. They are runtime data the
# service is pointed at, fetched by URL and hashed in the module that starts it
# (`machines/services1/services/voice-assistant.nix` when the wake tier lands), for the
# same reason the camera firmware is pinned rather than embedded: a model is not code, and
# swapping one should not rebuild a binary — or, given the paragraph above, the fleet.
{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "voice-wake";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = ./.;
    filter = path: _type: !lib.hasPrefix "${toString ./target}/" (toString path);
  };

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  # ## What actually pins these dependencies — measured, because the obvious answer here
  # is wrong.
  #
  # `cargoHash`, the field every other Rust package in nixpkgs carries, is **inert in this
  # call style**. Demonstrated 2026-11-17 by evaluating `cargoDeps` twice — once with
  # `lib.fakeHash`, once with a wrong-but-real-shaped SRI: the derivation was
  # `1g014cfgl5y1d549h7h9cf6vhw7wi01v-cargo-vendor-dir` **both times**, and the build with
  # the bogus hash succeeded silently. So a `cargoHash` line here would be theatre, and
  # "regenerate it with fakeHash" is not the advice it used to be. Writing one in anyway is
  # how a pin becomes a comment: something to look at, not something that acts.
  #
  # The real pin is `Cargo.lock`, in two parts, and both were measured here rather than
  # assumed:
  #
  # * the vendoring derivation is keyed on the lock's **contents** — appending a single
  #   newline moved it to `07d4f69dmlqyx3nr74c67g0kgbmfs13h-cargo-vendor-dir`; and
  # * every one of the **147 registry-sourced packages** in this lock carries its own
  #   `checksum = "<hex>"`, which cargo verifies per tarball inside the fetch.
  #
  # The second point needs the warning, because lock format **v4 writes the digest in hex,
  # not `sha256-<base64>`**: `grep -c '^checksum = "sha256-'` prints 0 and looks exactly
  # like an unpinned tree. It isn't; `grep -c '^checksum = '` prints 147, equal to the
  # 147 `source = ` lines, and that equality — not any hash in this file — is the property
  # worth checking if a lock is ever regenerated, vendored from a git source, or edited by
  # hand. Re-verify with:
  #
  # ```sh
  #   n=$(grep -c '^source = ' crates/voice-wake/Cargo.lock)
  #   c=$(grep -c '^checksum = ' crates/voice-wake/Cargo.lock)
  #   test "$n" = "$c" || echo "$n registry packages, $c checksums — the pin is not complete"
  # ```
  #
  # The engine resolves to the pure-Rust `tract` backend, so there is no ONNX Runtime blob
  # to link and no `LD_LIBRARY_PATH` to arrange: `ldd` on the result is libc/libm/libgcc
  # and nothing else.  If that ever changes (an `ort`-backed build for the 2.3× faster
  # inference upstream publishes), this needs `ORT_LIB_LOCATION`/`load-dynamic` pointed at
  # `pkgs.onnxruntime` and the note in `crates/voice-wake/README.md` updated in the same
  # commit, because the blob-free build is the reason the crate is buildable here.
  CARGO_BUILD_RELEASE = "1";

  # No tests in this crate, and `cargo test` would compile the whole dependency tree a
  # second time to discover that; the measurement the crate exists to make is a runtime
  # thing over real audio, not a unit test.
  doCheck = false;

  meta = {
    description = "Streaming wake-word detection harness for the voice tier";
    license = lib.licenses.asl20;
    # Deliberately not `mainProgram`-less: it is one binary, and `getExe` is how the
    # service module will name it.
    mainProgram = "voice-wake";
  };
}
