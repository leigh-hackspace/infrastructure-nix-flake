# Building Rust code here — crane, the workspace, and the wasm SPAs

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules (Rust for new
programs; shared code goes in `crates/common-rs/`; `just test` / `just clippy` /
`just build-frontend` are the entry points; never "fix" the wasm-bindgen pin by
moving either side). This file is how the build actually works and why it is
shaped like this — read it when you change `common/crane.nix`, a `Cargo.toml`, a
`build.rs`, or a flake output that builds a crate.

## Why crane, not `buildRustPackage`

The crates are packaged through **crane** (`github:ipetkov/crane`, shared helper
in `common/crane.nix`) rather than `pkgs.rustPlatform.buildRustPackage`, so the
dependency tree is compiled once into a cached derivation and a source-only edit
recompiles just the crate: measured on filestore, **~4 min cold vs ~50 s** for a
one-line change. `crane` reaches modules through the flake's `specialArgs` as
`CRANE` — modules never re-import it.

Call style is `CRANE.crate { pname; version; member; sharedCrates; }` — one
workspace member, built with `cargo build -p <pname>`. `crane` takes `cargoLock`
as a **path**, not nixpkgs' `cargoLock.lockFile` attrset, and vendors the crates
itself, so there is no `cargoDeps` hash to maintain.

One crate is not built this way: `crates/whisper-ws/`, which is not a workspace
member and is packaged by its own `package.nix`. See
[the fifth non-member](#the-fifth-non-member-crateswhisper-ws) at the end.

## One workspace, one dependency derivation

Because `crates/` is one cargo workspace (`crates/Cargo.toml`) there is **one
dependency derivation for the whole fleet** (`CRANE.workspaceArtifacts`). That is
the main win: before the workspace, filestore, gocardless-dashboard,
frigate-monitor and switch-gui each compiled their own copy of reqwest, because
each had its own lock file and so its own vendor dir.

Two things keep the *per-crate* build times anyway:

- **The source tree is the workspace with stub sources everywhere except the
  member being built** (`memberSrc`, built from `craneLib.mkDummySrc`). Cargo
  cannot build one member without loading every member's manifest — and it refuses
  a manifest whose declared targets are missing — so the tree has to carry the
  whole workspace. Stubs are what stop that meaning every `.rs` edit rebuilds all
  the programs: cargo sees the other members' manifests and no code.
- **The lock file reaches crane through `CRANE.lockFile`** (a `writeText` keyed on
  its **contents**), so the vendor derivation is keyed on dependencies rather than
  on the source tree. Changing a dependency anywhere invalidates the shared dep
  build for everyone — the price of a workspace, and the same price cargo charges
  locally.

`sharedCrates = ["<name>"]` names the `crates/common-rs/<name>` crates the member
reads: they are already in the tree as stubs (enough for cargo to *resolve* them)
and this swaps in the real code. **A crate that reads none must pass none** —
listing one it does not depend on makes its source an input of the build, so
editing it triggers a rebuild that changes nothing. `dto`-style crates that live
inside the app's own tree need no entry.

A `preBuild` that points `build.rs` at a wasm bundle is *not* an input to the
shared dependency derivation (the hooks are never passed to `buildDepsOnly`), so a
SPA-only change still does not rebuild a binary's deps. Pass `cargoArtifacts` only
to override the shared derivation deliberately.

**Known open hardening item:** `common/crane.nix` filters the four SPA directories
out of `workspaceSrc` and nothing else, and `lib.cleanSourceFilter` does not read
`.gitignore` — so an evaluation that reaches `crates/` as a *path literal* copies
`crates/target/` (3.1 GB) into the store. See the path-literal rule in
`docs/nix-dev-workflow.md` and the item in `docs/follow-ups.md`.

### Never splice a subpath of a source root: `${workspaceRoot}/x` drags the whole tree in

`cp -a \${workspaceRoot}/common-rs/json` reads like a copy of one directory, and it is one —
but the *splice* makes the **whole `crates/` tree an input** of the derivation, because that is
how Nix resolves `<source path>/subdir`. So every crate's `-src` derivation depended on every
other crate's code, and the per-member stub tree that `memberSrc` exists to build was
inert. Measured on services1, 2026-11-16, one **comment line** added to
`crates/slack-agent/src/agent.rs`:

| | before | after |
| --- | --- | --- |
| derivations built by `nix build` of services1's toplevel | **40** | **8** |
| native crates recompiled | all 9 | 1 (`slack-agent`) |
| wasm bundles rebuilt | 2 | 0 |
| wall clock | 102 s | 41 s |

The fix is `subSource` in `common/crane.nix`: filter to the subtree first, then splice *that*,
so the input moves only when that crate moves. `lib.cleanSourceWith` keeps the tree rooted at
`crates/`, so callers reach through it (`\${subSource "common-rs/json"}/common-rs/json`), and
`withSharedCrates`/`wasmSpa` grew a `srcRoot` for the same reason.

The same day, `mkDummySrc` was found to be fed `workspaceSrc` (everything) rather than a
manifest-only tree — so the stub tree moved on any `.rs` edit too, which is precisely what this
file's own comment claimed was impossible. `workspaceManifests` is the fix, and it is the
second instance of a pattern worth naming: **a comment describing an invalidation boundary is
not an invalidation boundary.** `nix flake check` cannot see any of this; the only test is
`grep -c "^building " <build log>` on a change that touches one file. Do that once after
touching `common/crane.nix` — the numbers above are reproducible in two minutes, and a fix that
silently regrows the rebuild cost is invisible until someone adds the tenth crate.

(Re-verify with: add a comment line to any one crate, `nix build --no-link
.#nixosConfigurations.services1.config.system.build.toplevel`, and count `^building ` lines and
`Compiling` lines per crate. Expect ~8 derivations and exactly one crate compiling.)

### `sharedCrates` is transitive, and only the Nix build checks it

`CRANE.crate` builds one member from a tree where every `common-rs/` crate the member does not
name in `sharedCrates` is present as crane's **manifest-only stub** — that is what keeps a `.rs`
edit from rebuilding every program (§above). So the list is not documentation, it is a
correctness input, and it has to include the crates your crate reaches *through* another shared
crate:

- 2026-11-16, `slack-agent`: it depends on `common-mcp`, and `common-mcp`'s client calls
  `common_http::post_json`. `sharedCrates = [ json mcp service ]` — no `http`.
- `cargo build` and `just test`: **green**, because in the real workspace every path dependency
  resolves to real code.
- `nix build .#nixosConfigurations.services1.config.system.build.toplevel`:
  `error[E0425]: cannot find function post_json in crate common_http`, with the error attributed
  to `common-rs/mcp/src/lib.rs` — a file in a crate that was *correctly* listed.

So **`just check-build` is the gate for adding a shared crate**, not `just test`, and the comment
in `machines/services1/services/slack-agent.nix` now says so where the list is. Deriving the list
from the members' `[dependencies]` `path = "../common-rs/X"` tables (transitively, with
`builtins.fromTOML`) is open in `docs/follow-ups.md`; until it is done, read the list as "what my
crate's transitive Rust closure touches".

## Local Rust workflow

```bash
nix develop                    # cargo/rustc/rustfmt/clippy + lld + just + git + jq + alejandra + node (all substituted)
nix develop .#wasm             # ... plus the pinned wasm-bindgen-cli, which is COMPILED (a few minutes, cold)
just test                      # cargo test --workspace, from crates/
just clippy                    # cargo clippy --workspace
just shared-rs                 # recreate the SPAs' git-ignored path-dependency symlinks
```

Both shells create the SPA symlinks for you on entry and need no network.  `default`
ships no `wasm-bindgen` at all, deliberately: the CLI is pinned to the SPAs' lock
version, its derivation is built with cargo, and putting it in `default` is what made
`nix develop` — and once, `just check` — a compile on whatever box you typed it on, the
router included (`docs/nix-dev-workflow.md`, "Never build on the router").  Anything
that builds a bundle asks for `.#wasm` by name: `just build-frontend`,
`just filestore-test`, `just switch-gui-test`.

The pinned `wasm-bindgen-cli 0.2.128` is a derivation (`CRANE.wasmBindgenCli`, the same
one the flake uses — no `cargo install`, no network on entry), and the stable toolchain
already bundles the `wasm32-unknown-unknown` std, so there is **no rustup and no network
needed for the target**. `lld` is included because the wasm target links with `wasm-ld`
from it (the flake sets `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld` and
passes `pkgs.lld`).

### The binary you test is not the binary you just changed

Measured twice on 2026-11-16 while bringing up `crates/voice-assistant`, both times costing
the same 15-minute detour:

* **`cargo clippy` and `cargo test` do not leave you an updated `target/debug/<bin>`.**
  Clippy builds its own check artefacts and `cargo test` links a test harness; neither
  necessarily re-links the plain binary. So `cargo clippy && cargo test && ./target/debug/
  voice-assistant probe …` runs **yesterday's binary**, whose output then contradicts the
  code you are reading. `cargo build -p <crate>` is the step that produces the thing; check
  `ls -l --time-style=+%H:%M:%S target/debug/<bin>` before believing a measurement, because
  "I just ran it and it still prints the old string" is a false statement about the code.
* **A pipeline that swallows the build's exit status turns a compile error into a stale
  measurement.** `cargo build -q … | tail -2` exits 0 with `tail`'s status, so the build can
  have died and the next command runs the old binary and reports what the old binary does.
  Same failure shape as the `… | tail -6 && sudo nixos-confirm` trap the GOLDEN RULE documents:
  *it is not the tool lying, it is the pipe* (`AGENTS.md`, "Deploying"). Read the exit status
  (`; rc=$?`) or don't pipe the build at all.

And the one thing neither command covers: **`cargo test --workspace` does not build
`crates/whisper-ws/`** — it is one of the five opted-out non-members (its `src/assets.rs` is
generated by an npm build), so a Rust edit there is compiled for the first time by
`nix build` of the machine that ships it. That is how a `--assistant-url` line inserted
*outside* a `//!` doc comment — an unclosed `[` in the usage text, i.e. a syntax error —
survived `just test` and was caught only by `nix build`, 898 derivations later.

## The five Dioxus SPAs

`crates/frigate-monitor/frontend/`, `crates/filestore/frontend/`,
`crates/gocardless-dashboard/frontend/`, `crates/network-status/frontend/`,
`crates/switch-gui/frontend/`. Each is
**its own cargo workspace**, not a member of the native one (the reason is written
out at the top of `crates/Cargo.toml`: a workspace resolves one version graph for
host and wasm, so a host-only feature unification would move `wasm-bindgen` out
from under the pinned CLI). Each has its own `Cargo.lock`, is built to wasm and is
embedded into its service through `CRANE.spaApp` (see
`machines/aibox/frigate-monitor.nix`,
`machines/services1/services/filestore.nix`,
`machines/services1/services/gocardless-dashboard.nix`,
`machines/services1/services/switch-gui.nix`,
`machines/gw/network-status.nix`).

**Their `dist/` directories are git-ignored and never committed** — the flake
rebuilds them at deploy time. All five are Dioxus since 2026-10-30; `crates/network-status`
was the exception for a year (its SolidJS bundle was committed because npm could not be
resolved offline) and now follows the same rule as the rest. See `docs/services.md`.

A SPA shares code with its server (the `dto` crates, `common-rs/preview`, the vendored
component library `common-rs/ui`) through a path dependency, and cargo insists a package
belongs to exactly one workspace, so
those are *copied* into the SPA's directory: locally by a git-ignored symlink
(`frontend/dto -> ../dto`, filestore's `frontend/common-rs`) that `just shared-rs`
or entering `nix develop` creates, and in Nix by a real materialisation via
`sharedCrates = ["oidc"]` etc. — **a symlink cannot travel in a flake source.**
Same file, two materialisations, so the SPA and the API cannot disagree.

Iterate locally:

```bash
nix develop
just build-frontend                    # all four SPAs
just build-frontend filestore          # just one
# equivalently, per frontend:
cd crates/frigate-monitor/frontend && nix develop .#wasm --command bash build.sh
```

All four call the same recipe, `common/frontend-build-spa.sh`.

### The wasm-bindgen pin (do not "fix" this)

The wasm-bindgen **CLI must match the crate version exactly**. The SPAs compile
against `wasm-bindgen 0.2.128` (in each `frontend/Cargo.lock`); the JS glue it
generates is schema-versioned, so `wasm-bindgen-cli 0.2.121` (what nixpkgs ships)
**hard-fails with a "schema version" error, not a warning**. That is why the
devshell pins 0.2.128 and the flake pins its own copy (`buildWasmBindgenCli` +
`fetchurl` from `static.crates.io` — the `crates.io` **API** endpoint is blocked,
use `static`). `common/frontend-build-spa.sh` reads the wanted version out of the
frontend's own `Cargo.lock` and **fails** on a mismatch, because the build would
die anyway.

**Do not "fix" a mismatch by bumping the crate or downgrading the CLI to nixpkgs'
version.** Keep the crate pinned at 0.2.128 and match the CLI to it, in all three
places at once (frontend `Cargo.lock`, `CRANE.wasmBindgenCli`, the devshell).

## The fifth non-member: `crates/whisper-ws/`

It is Rust, so it lives in `crates/`. It is **not** a workspace member, and not
for the wasm reason above:

* its status page is a Solid.js + Vite app in `web/app/`, and `src/assets.rs`
  (git-ignored) is **generated** from the built bundle by `web/app/gen_assets.sh`;
  `main.rs` will not compile without it, and cargo loads *every* member's manifest
  before it builds any of them and refuses a manifest whose target files are
  missing — so as a member it would put an npm build on the critical path of every
  other crate's dependency derivation;
* it depends on **zero external crates** (house style: stdlib-only), so the shared
  `Cargo.lock` and the one vendored dependency tree buy it nothing. It has its own
  `Cargo.lock`, which is empty but for itself.

So: it opts out of the workspace with an empty `[workspace]` table (the same
mechanism the SPAs use, and mirrored in `crates/Cargo.toml`'s `exclude`), it is
filtered out of `workspaceSrc` next to them (`common/crane.nix`'s
`nonMemberPrograms`, which also keeps its own `target/` out of everyone else's
source tree), and it is built by `crates/whisper-ws/package.nix` —
`buildNpmPackage` for the app, then plain `rustPlatform.buildRustPackage`, with
`preBuild` copying the generated `assets.rs` in. `flake.nix` binds that derivation
once (`whisperWsPkg`) and uses it twice: `packages.<system>.whisper-ws` and the
aibox overlay that provides `pkgs.whisper-ws` for
`whisper-ws.service`'s `ExecStart`.

```bash
nix build .#whisper-ws                       # from the repo root
web/app/build.sh && cargo build              # from crates/whisper-ws, no nix for the web step
```

It came from `~/Projects/whisper-ws` (a `git+file://` flake input with its own
`flake.nix`, whose `nixpkgs` pin was already overridden by a `follows` in
`flake.lock`); the flake went away in the 2026-10-15 move and `package.nix` is
that recipe, unchanged.
