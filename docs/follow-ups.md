# Follow-ups — the live backlog

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps a short list of the ones
that an agent can trip over or fix in passing; this file keeps each item with the
context that makes it actionable. **Add an item here (with a date and what blocks
it) rather than growing `AGENTS.md`** — that is what keeps the rules file a rules
file.

Narrative of work already finished: the current month's `docs/work-log-2026-11.md`; a month
that is closed gets absorbed into the live docs and deleted, and git history holds it.

---

## Nix / build

- **`nix flake check --no-build` on services1 dies on `path '…-sops-secret-pg_pass.drv' is
  not valid`** (measured 2026-11-25 while landing services3's VLAN move). Rule 9 makes a
  baseline a fact about a **lock**, so this was re-measured rather than inherited: it
  reproduces **byte-identically on a clean-HEAD worktree** (`git worktree add /tmp/…
  HEAD`), i.e. it is store state on this box at this lock and not the change that was
  being landed. Mechanism (docs/nix-dev-workflow.md, "Evaluation must not build anything"):
  one name in `infra.buildTimeSecrets` is read with `builtins.readFile` at eval time
  (`machines/services1/services/outline.nix` builds its postgres URL through
  `config.sopsSecretText "pg_pass"`), so the evaluation *needs* that derivation realisation,
  and `--no-build` declines to provide it while this box's store has since GC'd it — and the
  `.drv` cannot be rebuilt from its own store path (`nix-store --realise` answers
  `don't know how to build these paths`). **Blocks:** nothing that is running. It blocks
  `just check`'s flake step on this box and any cold evaluation of services1's config. **The
  way out is a build**, not an edit: `nixos-rebuild build|switch --flake .#services1` or
  `just check-build` instantiates and builds the drv, so the next services1 deploy restores
  the step and re-measures whether it is warning-free. Do not "fix" it by taking the name out
  of `infra.buildTimeSecrets` or out of `secrets/secrets.yaml` — that list is an assertion
  about a deliberate store-path cost (rule 13), and the pg password is genuinely read at
  build time by an app module.

- **`gw`'s evaluation prints a deprecation warning that is ours to fix: `getExe: Package
  "ups-watchdog-0.1.0" does not have the meta.mainProgram attribute`** (seen 2026-11-19 in
  `/var/tmp/deploy-gw-2026-11-19.log` while switching gw for the ACL row, three times, once
  per attribute path that reaches it). It is `machines/gw/ups.nix`'s package for the
  workspace member, and nix states the assumption will become an error. Rule 9 makes a
  warning a finding; the fix is one `meta.mainProgram = "ups-watchdog";`, which dirties
  every unit that embeds the binary — so it rides along with the next gw change rather than
  earning a switch of its own. Re-verify with:
  `sudo nixos-rebuild dry-activate --flake .#gw 2>&1 | grep -c getExe`.

- **A registry hiccup while fetching `whisper-ws`'s npm tree fails `just check-build` for
  the whole fleet — and the reason it had to fetch at all is that no box's store is shared
  with any other, not the
  commit** (hit 2026-11-12, on the aibox toplevel: `whisper-ws-web-0.1.0-npm-deps` failed
  with `couldn't fetch node_modules/@rolldown/binding-win32-arm64-msvc at
  https://registry.npmjs.org/… : [92] Stream error in the HTTP/2 framing layer`, which
  cascaded `unit-whisper-ws.service.drv → system-units → etc → nixos-system-aibox`).
  **The diagnosis written here on 2026-11-12 was wrong and is corrected as of 2026-11-15.**
  It claimed `fetchNpmDeps` takes `inherit src`, so any commit anywhere dirties that
  fixed-output and re-streams the whole npm tree. It does not, measured: touching a
  `.md` and asking the same question before and after gives an identical
  `whisper-ws-0.1.0.drv`, an identical `src`
  (`/nix/store/rv3kba0jcc44xwqpkyypn6zyrmvh8sjw-whisper-ws`) and an identical
  `whisper-ws-web-0.1.0.drv` — and because a drv path hashes its entire input spec, an
  unchanged web-app drv *proves* its npm-deps input is unchanged too. The crate's source
  is already content-keyed to its own directory (`crates/whisper-ws` is added to the store
  on its own; it is not a subpath of the flake's copied tree), which is exactly the
  narrowing this item proposed to add.

  What does make that fetch re-run: **the output was not in the store of the box doing the
  work**. Measured 2026-11-15 on `aibox`, then the network's builder: 21 150 / 39 848 store
  paths, 55 GiB and 85 GiB, including
  crate outputs other boxes had just been built against — and every one of them was then
  rebuilt or re-fetched on the next deploy, because outputs built *for another machine* are
  unreachable there and its GC collects them. Since 2026-11-23 no box builds for another at
  all, so that waste is not gone, it is per-box: five stores, five fetches of the same npm
  tree, five compilations of the same crate set. A fixed-output fetch that has to happen
  again is a network flake with the fleet's build behind it, and there are now five of those
  behinds.

  **Blocks:** nothing permanently (retry works), but it makes `just check-build` flaky in a
  way that reads like the deployed change being broken. **To do:** make one store's useful
  part reachable by the others rather than trying to narrow sources that are already
  narrow — the shape is a binary cache both sides can read and one box posts to (attic is
  already cloned, unused, at `~/Projects/attic` on aibox), or a rooted shared store.
  **Do not go looking for `machines/aibox/deployed-closure-cache.nix`, which an earlier
  version of this item named as existing: it never has** (`git log --all --` that path is
  empty), and the reason nobody noticed is the new item at the end of this file — check 2 of
  `common/check-doc-pointers.nix` reads AGENTS.md, README.md and the justfiles, and not
  `docs/*.md`. Finish the item by widening whatever root you land on to what the fleet
  demonstrably rebuilds. Re-verify the claim above (the command
  this item used to suggest, `nix derivation show -R` on the package, returns **nothing**
  here — it exits 1 — so use this instead):

  ```bash
  nix derivation show ".#packages.x86_64-linux.whisper-ws" | grep -oE '[a-z0-9]{32}-whisper-ws[^"]*\.drv' | sort -u
  # run it, edit any .md, run it again: the paths must be identical
  ```

- **No box's store holds what another box already built; the fleet recompiles its own
  dependency graph per deploy, and the fix is still blocked on a declared **root** ssh
  identity, not on a missing idea** (measured end to end 2026-11-15 while
  fixing it — the fix was designed, tested, and **not shipped** because step 1 is missing).
  Then `aibox` compiled for every other box and the outputs it produced for them were
  unreachable *on `aibox`*: nothing in its profiles referred to them, so its GC collected the
  fleet's crate set, engines and npm trees as litter, and the next deploy bought them back
  with compile minutes. That day: three GC passes deleted 21 150 / 2 339 / 39 848 store
  paths (55 / 30 / 85 GiB), after which `services1` rebuilt ~700 derivations it had already
  built hours earlier, `services2` rebuilt the shared cargo workspace and its CUDA engine
  from source, and a few hundred npm tarballs came back from the registry.

  The shape that would fix it is one symlink per machine on `aibox` —
  `/nix/var/nix/gcroots/deployed/<machine> -> <that box's current toplevel>` — bounded by
  construction to five roots, because `ln -sfn` replaces the link, so a box's previous
  generation becomes collectable the moment the box moves. Four things were measured about
  it, and the last one is why it is not in tree:

  1. **A symlink in a gcroots directory is honoured; a text file of store paths is not.**
     Both were tested against `nix store delete` on the same otherwise-unreachable path:
     the symlink made the delete refuse (`still alive`), the text root did not — the path
     was deleted anyway. So the mechanism has to be symlinks, one per path.
  2. **A root only protects a path the machine has.** `aibox` currently holds **zero** of
     the services3 generation that is deployed and running on services3 (`find /nix/store
     -maxdepth 1 -name "<toplevel>*" | wc -l` → 0): the client kept its own toplevel, and
     the GC took the rest. A root pointing at it would be a lie that protects nothing.
  3. **`nix copy` from a client to the builder fails on the documented identity trap** —
     `Too many authentication failures`, the same one the justfile already works around for
     `git fetch` with `IdentitiesOnly=yes`. **The operator-account half of this was answered
     on 2026-11-15**: `common/build-farm.nix` (since split; that block is now in
     `common/ssh-hop.nix`) grew `programs.ssh.hosts` — `IdentityFile ~/.ssh/agent-hop-key`,
     `IdentitiesOnly yes`, `ConnectTimeout 5` for the network's own addresses, rendered into
     `/etc/ssh/ssh_config` (`docs/nix-dev-workflow.md`, "The identity half of that hop is not
     the host half, and it was missing everywhere") — and it is the *system* file because this
     pinned nixpkgs has **no home-file option in `users.users.<name>` at all** (`just check`
     refuses `users.users.leigh-admin.extraFiles`). It does **not** help `nix copy`, though:
     the stanza spells the path `~`, which for the daemon expands to `/root/.ssh/agent-hop-key`,
     where no key is, and `nix copy --to ssh://` cannot pass `-i` at all. So the client→builder
     copy that would make `aibox` a real cache still cannot start. Closing it needs the same
     block with an **absolute** path — already this fleet's practice for root (`mkRemoteBuilder`'s
     `sshKey` is gone with the builder, but the `sudo env NIX_SSHOPTS='-i /home/leigh-admin/.ssh/agent-hop-key …'` hatch
     below) — and was not done quietly, because an absolute path in the *system* file also puts
     `IdentitiesOnly yes` on every user of those addresses at once, the NAS's other accounts
     included, and those have never been checked. That is item 5 of the root-ssh-config list
     above.
  4. Derivations survive where outputs do not: 26 `nixos-system*.drv` and 1 578
     `cargo-package-*` paths are still in `aibox`'s store, all from the **previous** lock.
     So `keep-outputs = true` plus rooting drvs is a second candidate shape, but `aibox`
     does not hold the *other* boxes' toplevel drvs either (only its own and `pxeclient`),
     so it has the same missing precondition.

  **Blocks:** nothing; it is a cost, not a fault — roughly a full engine rebuild per
  post-GC deploy, and since 2026-11-23 per *box* rather than per fleet, because the builder
  that used to amortise it is gone (`common/nix-gc.nix`'s header). **To do, in this order:**
  (1) declare the ssh identity for machine-to-machine
  `nix` traffic **as root** — the operator-account half landed on 2026-11-22
  (`common/ssh-hop.nix`: `IdentitiesOnly` plus `IdentityFile` for `10.3.1.*`, rule 32), so
  what is left is the absolute-path/root half, in the same file or
  `common/ssh-known-hosts.nix`'s neighbourhood,
  and deleting the hand-written `/root/.ssh/config` at the same time; (2) have `just deploy
  <m>` `nix copy --to ssh://<m>` the new toplevel from the box that built it — which is now
  the *only* way a box gets someone else's output, since nothing delegates any more, and it
  is cheap because `nix copy` sends only the delta; (3) root it with the one symlink.
  Verify each step rather than assuming it: `nix copy` from a client must succeed; the
  rooted toplevel must survive `sudo nix-collect-garbage` on `aibox`; and the proof the
  whole thing worked is that the *next* deploy of that box reports fewer built derivations
  than the one before it.

- **`services3`'s llama.cpp is nixpkgs' build recipe over the upstream flake's source, and a
  nixpkgs bump moves that recipe under it** (opened 2026-11-11, `machines/services3/llama.nix`).
  The engine is `pkgs.llama-cpp.override { vulkanSupport = true; }.overrideAttrs (… src =
  flakeInputs.llama-cpp-upstream.outPath; version = "1.0.0"; patches = []; npmDepsHash =
  <this tree's>; makeCacheWritable = true; cmakeFlags += [ LLAMA_BUILD_UI=ON and
  LLAMA_USE_PREBUILT_UI=OFF ]; doCheck = false)` because upstream's own flake cannot build here
  — its `tools/ui` web UI is a *derivation input* whatever `useWebUi` says, and
  `webui-0.0.0.drv` fails on `EACCES: copyfile … /build/source/static/favicon-dark.svg`. Every
  one of those lines is pinned to a fact about **this** nixpkgs pin, and **the condition this
  item was waiting on has now arrived**: at the 2026-11-12 nixpkgs lock `pkgs.llama-cpp` is
  **0.9.0** (`src` = tag `v0.9.0`) and that tag ships `src/models/lfm3.cpp` *and*
  `src/models/qwen35.cpp` — the two arches this box actually serves.  So "the moment nixpkgs'
  `llama-cpp` is ≥ the release that has `lfm3`" is satisfied, and taking the package straight
  is now a real option; **it is not a simplification any more, it is an engine downgrade**
  (1.0.0 → 0.9.0) whose KV ladder, flash-attention refusals and `--mmproj` measurements were
  all taken at the pinned tree, which is why it has not been taken here.
  `machines/services3/llama.nix`'s `npmDepsHash` and `version` are **not** derived from the
  input — they are hand-written copies of a fact about it, and the 2026-11-15 repin proved the
  drift is real: upstream changed one UI dependency's `resolved` URL from a `codeload` tarball
  to a `git+ssh` ref, the lock hash moved, and the failure surfaced only as `ERROR: npmDepsHash
  is out of date` part-way through a *toplevel* build (the same class as rule 31: a fact about
  a moving thing, hand-copied).  **Blocks:** nothing today; the pins are corrected at the
  2026-11-15 lock and the engine moved to 1.0.0 with the input.
  **To do (a decision, not a chore):** decide whether to take nixpkgs' package straight — at
  0.9.0 that is a downgrade of a live endpoint, so it needs the per-card measurements re-run
  first, and if it stays, then say so in `llama.nix` and stop describing the input as temporary.
  Re-verify before either half — these three ask the tree and cost no build:

  ```bash
  V=$(python3 -c "import json;print(json.load(open('flake.lock'))['nodes']['llama-cpp-upstream']['locked']['rev'])")
  RAW=https://raw.githubusercontent.com/ggml-org/llama.cpp/$V
  curl -s $RAW/CMakeLists.txt | grep -E 'LLAMA_VERSION_(MAJOR|MINOR|PATCH)'   # the tree's own version
  curl -s $RAW/src/models/lfm3.cpp   -o /dev/null -w '%{http_code} lfm3\n'     # 200 = arch still there
  curl -s $RAW/src/models/qwen35.cpp -o /dev/null -w '%{http_code} qwen35\n'
  nix eval --raw .#nixosConfigurations.services3.pkgs.llama-cpp.version        # what nixpkgs now ships
  ```

  And to see whether upstream's flake has fixed its own UI build:
  `nix build --no-link 'github:ggml-org/llama.cpp#packages.x86_64-linux.vulkan'`.
  The UI half has two failure modes of its own, both written up in
  `machines/services3/README.md`: `LLAMA_USE_PREBUILT_UI` defaults **ON** and *downloads* the UI
  from a Hugging Face bucket, which under a sandbox means an empty embedded asset table and a
  `404` at `/` with a perfectly healthy-looking build (this flake pins it OFF so it cannot be
  silent again), and npm 11 tries to `mkdir _cacache/tmp` inside the read-only `npmDeps` store
  path unless `makeCacheWritable` is set.

- **Evaluation still reads a derivation: finish the crane half of "Evaluation must not
  build"** (found 2026-10-22, on gw, by accident: `just check` began failing with
  `Cannot build '/nix/store/7ac7180r…-Cargo.lock.drv'` — `local builds are disabled
  (max-jobs = 0)`; **gw holds `max-jobs = 2` since 2026-11-23, so that exact refusal is
  gone and the realisation is now merely possible there — see the item at the end of this
  file**). **Blocks:** any `just check` on gw, so the router currently has no
  flake-level gate by choice (rule 22); nothing else is blocked, because the assertion evals and
  `just fmt-check` do not evaluate a machine. **It is not a regression** — the same call
  fails at `f923c6e`, the commit this pass branched from, from a clean `git worktree`. It
  was latent because `…-Cargo.lock` was sitting in gw's store, and it cannot go back: the
  path is an unsigned local build (`nix copy --from ssh://…aibox` → `lacks a signature by
  a trusted key`), and there is no cache for it.
  **Fixed later the same day, and it moved this item's failure point:** the hop itself was
  broken — every delegated build from gw died with `Host key verification failed` because
  the hop's ssh is the **daemon's** (uid 0, `HOME=/root`), not the operator account's, and
  gw's root had no host key for the builder. `common/ssh-known-hosts.nix` (DATA, the four
  flake machines pinned rsa+ed25519) handed to `programs.ssh.knownHosts` from
  `common/ssh-hop.nix` writes `/etc/ssh/ssh_known_hosts`, and a trivial delegated
  drv built on aibox and copied back. So the `Cargo.lock` read now **resolves** and
  `just check` on gw dies at the **filestore** read — `path
  'xc2p67qsy…-filestore-web-src.drv' is not valid` — the "second read" this item already
  predicted, seen from a clean pass. **The item is therefore one step shorter than written:
  the remaining work is the SPA-source reader**, not the `cargoLock`/`cargoLockContents`
  pairing (still worth doing, and still measured to uncover a read). Mechanism of the
  hop, the `-o KnownHostsFile=` dead end, and why the pin is declarative: `docs/nix-dev-workflow.md`,
  "The hop itself was broken, separately".
  **What to do:** `lockFile`'s result is handed to crane as `cargoLock` at three sites —
  `workspaceArtifacts`, the native crate arguments, `wasmSpa`'s `buildPackage` — without
  the `cargoLockContents` pairing `vendorDirFor` already uses, and reading a `writeText`
  result builds it. Doing that is measured to clear the `Cargo.lock` error and uncover a
  **second** read, `path 'xc2p67qsy…-filestore-web-src.drv' is not valid` (the SPA's
  `withSharedCrates` src); `crateName = pname` at both `buildPackage` sites does not
  remove it, so identify the reader first. Do it on aibox and prove the three toplevel
  `drvPath`s (`56m97iwl…`, `hs90mhsc…`, `9ip1w63…`) unchanged — an unverified edit to the
  shared dependency wiring costs every crate on the fleet a rebuild. Full mechanism, the
  A/B that proves the read, and the trace that ends at
  `machines/services1/services/filestore.nix:82`: `docs/nix-dev-workflow.md`, "Reopened
  2026-10-22".

  **Closed 2026-10-25, half of it — and the other half is wider than this item says.**
  `lockArgs` pairs `cargoLock` with `cargoLockContents` at all three sites that hand crane a
  lock (`3b1ec44`), and the call moved to the predicted next read, `…-filestore-web-src.drv`.
  What the move exposed: that read is keyed to the **flake source**, not to the box, so on any
  source hash nothing has evaluated `nix flake check --no-build` fails *everywhere* — measured
  on aibox in a fresh worktree with the same hash gw reported (`jfv7x3pz…`), and `--no-build`
  refuses it there even though aibox is one trivial `runCommand` away from having it. One full
  evaluation writes those `.drv` files and the check passes again (`nix eval
  .#nixosConfigurations.services1.config.system.build.toplevel.drvPath`, then
  `nix flake check --no-build` → `all checks passed!`, aibox 2026-10-25). So "run it on aibox"
  is not a workaround, it is a warm store, and this item's sibling — "`just check` is
  machine-dependent" — is the same read seen from the other side rather than a second door.
  The read itself needs a decision about where a SPA's path dependencies live, not a pairing.

  **Decided 2026-10-25: name the gate instead of buying the wiring.** `just check` split into
  `check-eval` (the assertion evals + `fmt-check`, no machine evaluated — safe on `gw` by
  construction, and the router's real gate) and `check`, which depends on it and adds
  `nix flake check --no-build`; on `gw` that step is **skipped with the reason printed** rather
  than dying halfway through the gate. So the router now has a gate it can actually run, and the
  bug is written down where it runs instead of being papered over with a per-box recipe. The two
  shapes that would remove the read are recorded in `docs/nix-dev-workflow.md` ("The decision
  taken 2026-10-25"): a build-phase copy, which moves the SPAs' path dependencies into their
  dependency derivations and needs a cache-cost measurement to justify, and per-app tracked
  copies, which rule 2 forbids and cannot express `filestore`'s `preview` at all. They were
  declined because the read behind this one — `builtins.readFile (config.sopsSecretText …)`, the
  item at the end of this section — means fixing the SPA half buys no gate at all: whoever closes
  the class closes both doors in one change, verified with the detector, or neither.

  **Its sibling is the item at the end of this section** — "`just check` is
  machine-dependent", where the read is `builtins.readFile (config.sopsSecretText …)` and
  what gets compiled is a *decrypted secret*. Same class, two doors, and they fail in a fixed
  order: gw dies on `Cargo.lock.drv` before the secret one is reached, and aibox passes
  only because those `sops-secret-*` outputs are already in its store — so that item's
  "the documented fallback has one leg" now reads "has one leg, and it is held up by store
  state". Whoever fixes either should fix the class: no `builtins.readFile` on a
  derivation in a module, which is the same sentence `common/crane.nix` was written to
  obey.

- **The door entry package asks the build for the network, for three reasons, and its
  build is not reproducible** (measured 2026-10-22 and 2026-10-25, and re-measured end to
  end 2026-11-23 under the `deno` that is deployed now, 2.9.8).
  `door-entry-management-system` sets **`__noChroot`**, and nix refuses such a derivation
  outright on a daemon whose `sandbox` is `true`:
  `derivation '…-door-entry-management-system-1.1.9.drv' has '__noChroot' set, but
  that's not allowed when 'sandbox' is 'true'`.  Reproduce it with
  `nix build --no-link --rebuild --option sandbox true ".#nixosConfigurations.services1.pkgs.door-entry-management-system"`
  (no system build needed); the same command with `--option sandbox relaxed` builds it.
  **Taken 2026-11-23, and verified on the box that was refusing:** aibox now sets
  `nix.settings.sandbox = "relaxed"` (`machines/aibox/configuration.nix`), which is what
  `just check-build` run on aibox — and any rebuild of services1's closure that has to
  compile rather than substitute — needed: the 2026-11-22 upgrade run lost the whole
  compile gate to this refusal and had to move the gate to services1 ("the gate refused to
  run where the runbook says to run it", docs/work-log-2026-11.md). Verified on the box
  after the switch, with nothing forced: the item's command — `building '…yhf7283…
  door-entry-management-system-1.1.9.drv'` — the *same* drv services1 holds — compiles
  in 2 minutes and answers `/nix/store/zrmk5wv6…`: the same output path the door units run
  today, so those 490 M of app binaries are now a copy over the hop instead of a compile per
  box. `/etc/nix/nix.conf` was read back after the switch (rule 10): line 13 `sandbox =
  relaxed`, `sandbox-fallback = false` unchanged under it. What "same output path" does **not**
  buy is the half still open below — with a build this network cannot reproduce, two boxes
  answering one store path are not guaranteed the same bytes, so the receipt names paths and
  claims nothing about the app. Leaving aibox strict instead costs every gate that asks
  it for services1's closure, which is what that 2026-11-22 run spent a whole gate on.

  **Three fetches, not one, and that is why vendoring does not fit the deno we run.** The
  whole `installPhase` reaches the network in three places, measured 2026-11-23 in a tree
  whose `node_modules` were pre-installed and then re-run under `sudo unshare -n` (no network):
  (1) `deno i` resolves the npm/jsr closure from the registry — this is what the package
  asks for and what it says it asks for; (2) `deno compile` downloads **its own runtime**,
  `https://dl.deno.land/release/v2.9.8/denort-x86_64-unknown-linux-gnu.zip`, even with
  `--cached-only`; (3) the sass/esbuild half of `deno task build` pulls
  `baseline-browser-mapping` data.  Offline, with only a cold deno cache and vendored
  `node_modules`, every phase that touches jsr fails first with
  `JSR package version manifest for '@hectorm/otpauth@9.5.0' failed to load: Specifier not
  found in cache: "https://jsr.io/@hectorm/otpauth/9.5.0_meta.json", --cached-only is
  specified`, so jsr deps need the **cache**, not just `node_modules`, and `deno install
  --frozen` — the flag 2026-10-25's plan named — no longer exists in 2.9.8.  A
  vendored closure would therefore have to pin a deno-version-specific cache (the denort zip
  and the toolchain data move with deno's version), i.e. a hand re-pin at every nixpkgs bump,
  inside the monthly upgrade runbook, for a package that cannot be built reproducibly in the
  first place (below).  That is a change to the app's own flake (`github:leigh-hackspace/door-entry-system`,
  `management-system/`); when it lands there, `__noChroot` drops out of the closure by itself
  and `just noChroot` is the thing that says so.

  **What is still open: the package is not reproducible**, and the relaxation does not fix
  that half.  `--rebuild` on the already-valid drv prints
  `may not be deterministic: output … differs` (reproduced 2026-11-23), so the output
  path moves on every build, "identical drvPath / identical output" is not a proof that
  includes it, and every rebuild of services1 re-compiles 490 M of two self-contained
  `deno compile` binaries (405 M backend, 106 M frontend, read out of the live store path
  2026-11-23) and restarts both door units.  A pinned fixed-output derivation is the other
  shape that would make the path stable, and it is the same problem: the binaries move with
  deno's version, so the pin would be re-pinned on every bump.

  **The consequence to keep in mind:** `nix.settings.sandbox = "relaxed"` is this
  derivation's, not a box's, and `just noChroot` names it per box (re-read 2026-11-23: one in
  services1's 8,886 derivations — the door entry one; one in aibox's own 18,547,
  `llama-cpp-vulkan-0.0.0.drv`, which a strict daemon has only ever answered there by
  substitution; none in services2's 8,027 or gw's 5,540; services3 is Wi-Fi-only and was
  unreachable that day).  The older 2026-10-25 read was one in 7,547 and none in 7,840, which
  is why services2 went back to the default `true` (its own closure asks for nothing; the
  recipe is in `machines/services1/configuration.nix`, and services1 still needs the setting
  because nothing else builds this derivation — measured the same day, the drv is absent
  from every other reachable box).

- **`just check` on gw could now run its flake step; it still does not, and that is a
  decision someone should take rather than inherit** (opened 2026-11-23, by the change that
  withdrew the network's builder). The old reason for the skip was hard: gw had
  `max-jobs = 0`, so `nix flake check --no-build` — which needs one trivial source
  realisation, because evaluating the machines reads a derivation (`common/crane.nix`, and
  the two items above) — could only fail there. Now gw builds for itself at 2 jobs inside a
  3/4.5 GiB cgroup (`machines/gw/default.nix`), so the step *can* run. **For:** the router
  would then have a flake-level gate of its own, and `just check` would mean the same thing
  on every box. **Against:** it is one realisation the router did not previously pay, on the
  box that answers DNS for the network, and `just check-eval` already covers every assertion
  and the formatter. **Re-verify with:** `sudo nix-collect-garbage --max-age 1d` is *not*
  the way to test this on the router; use a fresh `git worktree` of the flake source on gw
  and run `nix flake check --no-build`, and read `nix config show | grep max-jobs` first.
  The deleted item this one replaces (added 2026-10-21, deployed the same evening,
  superseded 2026-11-23) is in git history:
  `git log -L :1:docs/follow-ups.md --grep='gw now delegates'` or
  `git show 83b5ba3:docs/follow-ups.md`. Its two open questions — prove the delegation and
  prove the failure, and whether `just switch` should be allowed on gw — were answered the
  same way: the first by deletion, the second yes, bounded, and measured by reading
  `systemctl show nix-daemon -p MemoryMax --value` after the switch.

- **`just check` is store-warm, not tree-warm: on a box that has never built
  filestore's SPA source it dies at HEAD** (measured 2026-10-21 on `gw` at
  `3a8d231`, where the pristine tree fails identically; the same command on
  services1, same commit, prints `all checks passed!`). The error is the signature
  this file already documents —
  `error: path 'fmwkw5y9566w42c8zciwb5a15gb3vzmf-filestore-web-src.drv' is not
  valid`, raised while forcing `systemd.services.filestore.serviceConfig` — so
  something reads a *derivation* at eval time and `--no-build` declines to build it
  rather than failing to. The read is `CRANE.wasmSpa`'s (`machines/services1/services/
  filestore.nix`, whose `cargoLockPath` is exactly the workaround applied to the
  `Cargo.lock` half of the same problem). Two consequences worth naming: AGENTS'
  "a clone of this repo evaluates every machine and `just check` runs anywhere" is
  true of a *warm* clone only, and the box the docs single out as safe for
  `just check` — gw, because it evaluates and formats without compiling — is now a
  box where that command fails for a reason that has nothing to do with the change
  being checked. Workaround, until the src read goes: run the gate on services1 or
  aibox, and locally use the standalone assertion evals
  (`machines/gw/lib/check-network-tables.nix`, `common/check-printer-states.nix`,
  `common/check-doc-pointers.nix`) plus `nix eval` of the option
  you changed. The durable fix is the one the crane header already prescribes — hand
  the SPA builder the source by path and never let it read a derivation.

- **`nix flake check` is not warning-free: two findings, both reachable now**
  (noticed 2026-10-14 while checking the `gw` machine; re-read on 2026-10-15 during
  the network upgrade). Checking `nixosConfigurations.services2` emits
  `evaluation warning: Grafana passwords will be stored as plaintext in the Nix
  store! Use file provider or an env-var instead.`, which by rule 9 is a finding.
  **Re-read at the current lock, 2026-11-18, and neither finding reproduces:** `just check`
  on services1 evaluates all six configurations (`nix flake check --no-build`) and prints
  no evaluation warning at all — the only `warning:` line in its output is `Git tree … is
  dirty`, which is the uncommitted tree, not a finding. Close this item on the next
  clean-tree run rather than inheriting either half of it (rule 9: a baseline is a fact
  about a lock).
  Original finding, for the record:
  either the Grafana provisioner moves to the file provider /
  `GF_SECURITY_ADMIN_PASSWORD__FILE`-style env, or the warning is the documented
  exception. **The blocker that used to hide it is gone at the 2026-10-14 lock:**
  `nix flake check` no longer dies on
  `error: path '…-sops-secret-<name>.drv' is not valid` — it reaches all five
  configurations (`services1`, `services2`, `aibox`, `pi-room-sys`, `gw`) and exits
  0, so both warnings are now visible from one command and both are findings.
  (Note `nix flake check` is not a cheap eval gate: it *builds* `packages.strata`,
  `packages.strata-cuda`, `packages.whisper-ws` and the devShell — ~10 minutes on a
  lock bump.)

- **Any tracked edit anywhere re-hashes pi-room-sys's and aibox's system
  derivations** (measured 2026-10-18 while proving a `common/crane.nix` change was
  drv-neutral). `services1`, `services2` and `gw`'s toplevel `drvPath`s are stable
  across an unrelated edit; `pi-room-sys`'s is not — one HTML comment appended to
  `docs/follow-ups.md` produced a third distinct drvPath, and the moving parts are
  `append-initrd-secrets` and `boot.json`. aibox inherits it through
  `machines/aibox/netboot.nix`'s `netboot.ipxe`, which names the pi-room-sys
  closure, so a doc edit re-instantiates aibox's `/etc` too. Nothing rebuilds
  *expensively* (the kernel and the squashfs store keep their paths), but it makes
  "identical drvPath" unusable as a proof for those two machines and it dirties
  every deploy's diff. Suspect a path literal that drags a whole-directory store
  path into the initrd (`./leigh-logo.png`, `./ssh_host_ed25519_key` in
  `machines/pi-room-sys/configuration.nix` are the candidates; a single-file literal
  should be content-keyed, so the real culprit is probably how one of them is
  consumed). Blocked on: reading the generated `append-initrd-secrets` drv, which is
  not on disk — instantiate it with `nix build --no-link --keep-going` on the
  pi-room-sys toplevel and inspect, rather than guessing from the module.

  Re-measured 2026-10-22 while proving the `flake.nix` restructure (five copies →
  `mkSystem`) was drv-neutral, which is exactly the case this item blocks: on a tree
  whose only changes were `flake.nix` and the four `machines/<m>/default.nix`, the
  `services1`/`services2`/`gw` toplevel `drvPath`s came out byte-identical and the
  other two did not. **What aibox's `/etc` actually gains from the churn, in full:**
  one line of `fstab` — the squashfs store path of the `/exports/netboot-squashfs`
  bind mount (`diff -r` of `config.system.build.etc` before and after finds nothing
  else). So the workaround for a proof like that is stated rather than guessed: compare
  the three stable machines by `drvPath` and aibox by its rendered `/etc`, and expect
  that one line. The squashfs itself keeps its contents; only its path moves.

- **~17 GB of fat `-source` copies that only `nix store gc` can reclaim**
  (2026-10-18). Five 2.9 GB copies of the pre-`crates/` working tree, from the
  `builtins.getFlake "/abs/path"` evals of 2026-10-11/12, are each named by 2–6 old
  `.drv` files, so `nix store delete` refuses them; the three 7.7 GB copies from
  2026-10-18 were unreferenced and went away with 22.6 GB freed. Two things to know
  before repeating it: the unreferenced ones still refused as `still alive` until
  `--ignore-liveness` was used (no roots, no referrers, no process naming them —
  unexplained, so check `nix store verify --all` if it recurs), and a client whose
  environ names a path roots it, so assemble path strings inside the script
  (`docs/nix-dev-workflow.md`). What is left is a `nix store gc` decision on this
  box, which is a maintenance call rather than a fix: 113 GB used, 93 GB free.

- **aibox's root, and the model data that lives outside the store** (opened 2026-10-17
  with the move of the builder role off services2; the builder half of this item is
  **void as of 2026-11-23** — nothing delegates, `nix.distributedBuilds` is nowhere in the
  tree, and `just build-remote` is deleted, so there is no hop left to prove. What that
  era settled: the hop's last proof was 2026-10-12, a delegated build and a `--max-jobs 0`
  build against services2 with no SIGKILL sweep, and the mechanism behind it is
  `common/nixbld.nix`. The disk half is more live than it was, because every box now grows
  its own store).

  1. **aibox's root and the model data.** aibox measured 82 % full on
     2026-10-15, while services2 had 211 GB free; the builder is the box that now
     grows `/nix/store`. Strata's data is deliberately outside the store and is
     **not** removed by disabling the service: the IQ2_XS GGUFs under
     `/home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF` (~68 GB) and the
     pack/MTP under `/home/leigh-admin/Strata`. Deleting them is a manual decision
     — do not automate it from the flake — but until then this box's headroom is
     smaller than a build wants. `common/nix-gc.nix`'s policy (`--delete-older-than
     30d`, plus `store-hygiene.nix`'s weekly walk) is what keeps it from being a
     surprise, and since 2026-11-23 `nix-daemon` is capped at 12/18 GiB
     (`machines/aibox/configuration.nix`) so a full store cannot take the engine.

- **`packages.strata-cuda` emits four upstream deprecation warnings (open, one
  line).** `checking derivation packages.x86_64-linux.strata-cuda` prints
  `stdenv.isLinux` / `isAarch64` / `isDarwin` "is deprecated, use
  stdenv.hostPlatform.…". They are not ours — nothing in this tree says `stdenv.is*`
  (`grep -rn 'stdenv\.is' --include=*.nix` is empty) — they come from the CUDA-enabled
  package set `pkgsCuda` (`nixpkgs` with `cudaSupport = true`, `flake.nix:295`) and
  the `cudaPackages_13_0` overlays. Rule 9 says a warning is a finding, so either
  they get an exception here with a pointer to the upstream file that still uses the
  old attribute, or they go when nixpkgs' CUDA glue does. `packages.strata` (the
  HIP one on aibox) is clean.

- **`crates/target/` is not excluded from `workspaceSrc` (open, one line).**
  `common/crane.nix` filters the four SPA directories and `whisper-ws` out of
  `workspaceSrc` and nothing else, and `lib.cleanSourceFilter` does not read
  `.gitignore`, so an evaluation that reaches `crates/` as a **path literal**
  copies 3.1 GB of cargo artifacts into the store while holding the nix big lock —
  which is what wedged the box on 2026-10-14 (mechanics:
  `docs/nix-dev-workflow.md`). Harmless through the flake, since `self` arrives
  git-filtered, so this is hardening rather than a fault: adding `target` to that
  filter makes the mistake impossible to repeat. (`crates/whisper-ws/` is its own
  cargo workspace, so it writes a second `target/` one level down; that one is
  already excluded, as a side effect of `whisper-ws` being a non-member.)

- **The `deploy` account is dead weight and should go.** `useradd`ed on 2026-10-11
  as a workaround for the build-user bug (build as a uid that owns nothing), with
  its own copy of the repo and the private flake inputs under `/home/deploy`. The
  bug is fixed (`common/nixbld.nix`, deployed 2026-10-12), so nothing needs it, and
  it is not declarative — a switch will not recreate it. Remove it
  (`userdel -r deploy`) unless something turns out to depend on it. Do **not**
  deploy through it: builds as `leigh-admin` are safe and that path is the one
  `just switch` takes. (Since 2026-10-15 the "private flake inputs" half of why it
  existed is gone entirely: the flake has no `~/Projects` inputs to copy.)

- **services2 has no swap partition** (services1 has 18 GB). Fine at the current
  load, worth matching before it runs anything heavier than builds. **Update
  2026-10-15:** it has 15 GB of *swap file* (`/var/swap/services2` — the
  `DEVICE=/var/swap/services2` in its units is swapon), so the partition is absent
  but the swap is not; what is true is that a full three-machine build pushed the
  box to 0 MiB free and ~6 GiB swap while strata held 24-26 GiB resident. It survived;
  the margin is not there twice.

- **Declare root's `safe.directory` for the repo, or stop depending on it** (found
  2026-10-15, first run of `docs/network-upgrade-runbook.md`). `sudo nixos-rebuild
  switch --flake .` opens the checkout as **root**, and root's HOME is `/root`
  (`sudo`'s `secure_path`/`env_reset`), so git's ownership check fires: `repository
  path '…infrastructure-nix-flake' is not owned by current user (libgit2 error code
  = 7)`. services1 and services2 have `/root/.gitconfig` with
  `safe.directory = /home/leigh-admin/Projects/infrastructure-nix-flake`; **aibox
  did not**, and its first switch of the run died on it in the first second. That is
  imperative, undeclared, per-host state under the single most-used command in the
  repo — one `nix.settings`-adjacent line (or `programs.git.safeDirectories` for
  root, or `sudo` keeping `HOME`) makes three boxes agree. Until then Phase 0 checks
  it, because the failure looks like a permissions bug in the deploy rather than a
  missing config file on one box.

- **aibox's netboot export makes every switch of that box exit 4** (found
  2026-10-15). `/exports/netboot-squashfs` is a `fileSystems` **bind mount of a
  store path** (`machines/aibox/netboot.nix`), so the source changes on every
  nixpkgs bump; the mount unit is boot-generated from `/etc/fstab` (nothing in the
  toplevel's `etc/systemd/system`), so activation's `systemctl restart` of it fails
  *after* `/run/current-system` has flipped and after nginx/sshd/immich/strata have
  restarted. The workaround is the `just switch-netboot` dance (`umount -f -l` +
  `mount -a`, then switch again) and it is now written into the runbook; the fix is
  to stop the export path from moving — export a directory that holds a **symlink**
  to the current image (or `linkAs` the squashfs into a stable name), so the bind
  mount is created once and never restarted. Until then: aibox's switch is expected
  to fail once, and the second failure is the stop condition.

  It no longer panics clients when it does fail: the menu boots
  `init=/nix/store/.netboot-closure/init` and each image carries that symlink to
  its own closure (2026-10-21), so a client that mounts the *previous* image boots
  the previous generation instead of dying in `initrd-find-nixos-closure`. A
  refresh that leaves the export *unmounted* still stops clients from booting at
  all, so the dance stays in the runbook.

- **`just check` is machine-dependent: an eval-time secret read makes it pass or fail on
  store state** (found 2026-10-22; the failure was first reproduced against clean
  `f136d8a` in a throwaway worktree, then the *same* command passed on the same tree once
  the path existed, which is the actual bug). The failing form is:

  ```
  error: path 'zicnr1hgclx6vmnrhh17w30sd2h4f8dz-sops-secret-pg_pass.drv' is not valid
  ```

  with a trace ending at `services.outline.databaseUrl`:
  `machines/services1/services/outline.nix` does
  `builtins.readFile (config.sopsSecretText "pg_pass")`, and reading a **derivation** at
  eval time realises it. `--no-build` declines, so `nix flake check` dies — on any box
  that has not already got that path — while a build or a deploy silently *compiles the
  secret* to satisfy the same read (`building '…-sops-secret-pg_pass.drv' on
  'ssh://leigh-admin@10.3.1.32'…`, seen live; the output is now in services1's and aibox's
  stores, which is why the check passes today and will not pass on a fresh box). **It
  stopped passing on aibox on 2026-10-24** — same command, same
  `zicnr1hgclx6vmnrhh17w30sd2h4f8dz-sops-secret-pg_pass.drv`, same trace, because aibox's
  GC had collected the realised path since the item was written and "the store has it"
  was never a durable precondition, only a temporary one. That is the item's own
  conclusion restated as a receipt: the check is store-state-dependent, so treat a
  `sops-secret-*.drv` death as this and not as a regression — and the fix is not
  `--no-build` gymnastics, it is realising the path by evaluating any one machine once
  (`nix eval --raw ".#nixosConfigurations.services1.config.system.build.toplevel.drvPath"`
  builds the five secret decrypts in seconds and no compiled packages), which is exactly
  what was done here before `just check` was believed again. Two things are wrong. One is the eval-time realisation itself — the class
  `docs/nix-dev-workflow.md` says evaluation must never be in, and the class
  `common/crane.nix` deliberately designed around with `cargoLockContents`. The other is
  that `services.outline.databaseUrl` is a string, so the postgres password ends up in a
  world-readable store path either way; fixing the first properly means not putting the
  secret in that option at all (some `EnvironmentFile`-shaped plumbing), or a
  `databaseUrlFile`-style change upstream), which is a bigger call than a ride-along.
  Until then: if `just check` dies on `sops-secret-*.drv`, that is this item, not your
  change — say so, and say what you verified against a clean HEAD. **Reproduced again
  2026-11-11, on both names and both sides of that rule:** `just check` on services1 died
  on `…-sops-secret-pg_pass.drv`, and `nix flake check --no-build` from a clean
  `git worktree` at `5787f23` (so: at HEAD, with nothing of the author's in it) died on
  `…-sops-secret-grafana_secret_key.drv` — which is the exact string AGENTS.md rule 9
  records as *not* reproducing at the 2026-10-14 lock, and this flake is still pinned to
  `26.11.20261014.319e4ce`, so the doc's sentence is a receipt about a store, not about a
  lock. The documented unblock worked: `nix eval --raw
  ".#nixosConfigurations.services3.config.system.build.toplevel.drvPath"` realised the
  decrypts in seconds and the trace then reached further before dying the same way.

  **Again 2026-11-16, on services1, with two things this item does not yet say.** The
  unblock is **per-machine, not global**: evaluating `services1`'s toplevel realised
  `pg_pass` and the check then died on `grafana_secret_key`, then on `strata_api_key`, so
  it took all four `commonModules` machines' `toplevel.drvPath` (`services1`, `services2`,
  `services3`, `aibox` — each box realises the secrets its own modules read) before
  `just check` printed `all checks passed!` — the first green flake step measured on this
  box at the current lock, `26.11.20261112.2074cc6`. And `nix build /nix/store/<hash>-….drv`
  is **not** a way in: it answers `error: path '…' is required, but there is no substituter
  that can build it`, because a GC'd *drv* path is not in any substituter — only
  re-evaluating the expression that produces it writes it back.

- **Nothing runs the three Playwright suites, so a red one is only found by whoever happens to
  need it** (found 2026-11-09 while restyling network-info: `5ebe2d5` added the Uplink tab at
  17:10 and five of `crates/network-status/tests/suite.mjs`'s ten checks went red silently — a
  click on the wrong tab still clicks something — and nothing would have noticed at all had the
  restyle not needed the suite two hours later; the last receipt in that suite's own README,
  "ten checks, all pass" dated 2026-10-31, was still standing as the record of a failing
  suite). The cause is structural, not carelessness:
  `just network-status-test`, `just switch-gui-test` and `just filestore-test` are build-shaped
  (`_build-host-guard`), so they cannot run on the router, and they are not in `just check`,
  `just check-build` or `just test` anywhere else either. They run when a human remembers.
  **Blocks:** nothing — this is an open question, not a broken thing. **Options, cheapest
  first:** (a) run the affected suite whenever a `crates/*/frontend/**` or `machines/*/*.nix`
  unit that serves it changes, which is a documentation rule and is what `AGENTS.md` rule 3
  gestures at but does not say; (b) add the three to a new `just check-ui` and run it on
  aibox before a deploy that touches a frontend; (c) a nightly `systemd.timer` on aibox that
  runs all three and leaves the journal as the alarm — cheapest to keep honest, and it also
  catches the browser/crate drift none of these suites can see today. What blocks (c) is
  that `switch-gui-test` needs the local Postgres cluster and all three need
  `nixpkgs#playwright.browsers-chromium` present, so it wants the same shape as the other
  nightly units in this file (a `--no-block` `systemd-run` or a oneshot + timer, and a
  failure that is visible without reading a log). Re-verify the gap with:
  `just network-status-test` on services1 or aibox, and
  `grep -n "network-status-test\|switch-gui-test\|filestore-test" just/tests.just`.


## Strata (services2, and aibox's engine since 2026-10-28)

- **An md5 of the completion is a weak A/B instrument on Strata, and it is worth knowing
  before you trust one** (measured 2026-10-28 on aibox's engine: same config, same prompt,
  `temperature: 0`, two runs, **different output digests** — `edf775d3…` then `e83db119…`
  over a fixed 200-token reasoning sample; and the same for two 16-token replies in one
  A/B arm). Greedy decoding on this engine is not byte-reproducible run to run — most
  plausibly kernel atomics / batch-shape in the grouped experts, none of which is a bug to
  chase here. `docs/follow-ups.md` and `machines/services2/STRATA.md` both tell the next
  reader to "compare the md5 of the completion", and that advice is right only with this
  added: **run the arm twice first.** If the digest moves within an arm it cannot measure a
  difference *between* arms, and the honest alternatives are (a) a many-sample agreement
  rate (how often two runs share a digest), (b) upstream's own first-token KL figures, or
  (c) reading both outputs. Timing stays a fine instrument, and the within-arm spread of
  this box's own cold reads (349.7 s vs 257.9 s for the same-size prompt, ±26%) is the
  noise floor any speed claim has to clear.

- **aibox's prefill wall has an upstream lever that its rocBLAS build refuses: measure a
  gfx1030 solution table** (found 2026-10-28 while bumping aibox's engine from 0.1.43.1 to
  the flake input's 0.1.52). aibox's real limit is prefill — ~45 tok/s whatever the config,
  so a 20 000-token prompt is a 7-minute prompt (`machines/aibox/STRATA.md`, "The real limit
  for coding use is prefill" and "The plateau"). 0.1.4x added exactly that problem for this
  card: `src/prefill/rocblas_tuning.hpp` — "rocBLAS picks a kernel per GEMM shape from its
  own table, and on gfx1030 that pick is up to 6x slower than the best kernel the library
  holds for the same shape (N=10240 K=2560 below T=1152: `tools/hip/tune_rocblas.cpp`)" —
  read through `STRATA_ROCBLAS_TUNING` (a table file, or a directory holding
  `<arch>-rocblas-<version>.txt`). Upstream ships `tools/hip/gfx1030-rocblas-5.6.0.8d1ae90e.txt`,
  and the loader refuses any table whose header's rocBLAS version string is not the linked
  library's: nixpkgs reports **5.2.70203** (`librocblas.so.5.2.70203` in
  `rocmPackages` 7.2.4), so today it is **refused and its absence is invisible** (one line
  on stderr: `prefill gemm: rocBLAS … version mismatch …; rocBLAS's default kernels run`).
  **To do:** build `tune_rocblas.cpp` against this rocBLAS (it is in the source tree the
  engine builds from; `machines/aibox/strata-package.nix` already builds the HIP test
  targets whenever HIP is on, which is how `strata-alias-check` got installed), name the
  result `gfx1030-rocblas-<version>.txt`, point `STRATA_ROCBLAS_TUNING` at the directory in
  `$out/share/strata/tools/hip`, and A/B a long prompt — answers may move (a different GEMM
  algorithm is a different summation), so compare `md5` of a completion, per the house rule
  about measuring the answer and not the banner. **Blocks:** nothing; env-only, no engine
  rebuild once a table exists. Note the interaction with ROCm 10: with
  `services.strata.rocm10Runtime = true` the linked rocBLAS *is* the runtime's (10.0.0
  reports its own version string and carries 144 gfx1030 Tensile files), so a table has to
  be measured for whichever runtime the unit actually loads — `strata-device --selftest`
  prints `driver / runtime` and is the one-line check of which that is.

  **Addendum, 2026-11-04 (engine 0.1.56):** the loader now *names* the versions it holds
  community tables for — the deployed unit prints, on every start,
  `community rocBLAS tables for gfx1030 are loaded when the running rocBLAS is exactly
  5.4.0.ad84daa81c or 5.2.0.dabb6df2b98-dirty and STRATA_ROCBLAS_TUNING points at them`,
  and nixpkgs' rocBLAS is still **5.2.70203**, so both are still refused and the work above
  still stands; what the same new block turned **on** for this card at 0.1.56 is its other
  half — the **simt2 QSA selector is now the default** (#3073, "community-tested only",
  `STRATA_SELECT_SIMT=0` puts the old selector back, no rebuild, answers move). Measured
  receipts for the HIP side of the bump — the control bench against the plateau band, and
  the fact that a plain `just switch` on aibox fails on the busy netboot bind mount and
  auto-rolls the box back — are in `machines/aibox/STRATA.md`, "Update: engine 0.1.52 →
  **0.1.56** (the flake input), deployed — 2026-11-04".

  **And try the one-line version first.** The engine prints, unprompted, on every start of
  the deployed unit (seen in `/var/lib/strata/strata.log` on 2026-10-28):

  > `strata prefill: HIP device 0 gfx1030 - tip: STRATA_HIP_PROMPT_F16=1 reads long prompts
  > about 2x faster here (FP16 prompt GEMMs, rocBLAS is tuned only for FP16 on gfx103x; the
  > numbers differ slightly, see docs/AMD_HIP.md)`

  That is upstream naming the *same* rocBLAS-tuning fact as an answer available today: FP16
  prompt GEMMs are the tuned path on gfx103x, so the FP32-out shapes the tuning table above
  exists to fix are partly a consequence of the default. `STRATA_HIP_PROMPT_F16=1` is one
  `extraEnv`/`env` line in `machines/aibox/strata.nix` — no rebuild — against a claimed 2x on
  the axis that costs 7 minutes per agent turn, for "the numbers differ slightly".
  **Measured 2026-10-28: it is ON, and the 2× is real here** — 349.7 s → 158.4 s on the same
  13,069-token cold read (37.4 → 82.5 tok/s), corroborated by a second fresh prompt at 81.7
  tok/s, decode unchanged; the measurement, the 26% noise floor it had to clear and why an
  md5 could not bound the "not bit-identical" are in `machines/aibox/STRATA.md`, "Prefill
  2.2×". What the table above still buys, if anyone measures it, is the remaining gap on the
  FP32-out shapes — and probably better than "slightly" on answers, since it changes the
  algorithm chosen rather than the precision it runs at.

- **Unexplained, and it bounds what aibox's conversation-cache numbers mean** (found
  2026-10-28 while measuring that cache). This file's own older logs, on engine 0.1.43.1 with
  **no** parking available at all, show `prompt 97819 tokens = 97781 reused + 38 read in
  2834 ms … 6 checkpoints` — a *grown* prompt reusing 99.96% of the previous one. The probes
  run today against the served endpoint (`/v1/chat/completions`, one `user` message, same
  13K-token prefix, +2 and +30 tokens) report `reused 0` in **both** arms, parking on and
  off, while an *identical* prompt restores in 37 ms. So prefix reuse exists on some path and
  not on the one clients use, and nobody has read the code far enough to name the
  discriminator (`--prompt-cache` checkpoints? the multi-message `messages` array? session
  identity from the connection? a header the front door does or does not forward?). **Why it
  matters:** the answer decides whether a long agent conversation on aibox costs 1.2 s or
  158 s per turn — usable for coding agents, or not. **To do:** read the reuse/matching path
  in `serve/server.py` and `src/core/conversation_*`, then reproduce with the built-in chat
  endpoint (where the 97,781 line came from) against a single-message request of the same
  tokens. Not an upstream bug until that comparison is done; the cache flags in
  `machines/aibox/strata.nix` say what they measured rather than what this would conclude.

- **The 2026-10-27 bump turned two switches on for this card class, and nobody has
  measured them on our pack** (found while bumping; the whole finding is in
  `machines/services2/STRATA.md`, "Update: engine 0.1.52 (app 0.2.7) — 2026-10-27").
  `STRATA_ROUTE_PRIOR=1.5` (gate: one discrete NVIDIA card of ≤13 GiB — `kSmallCardGiB = 13.0`,
  ours is the 12 GB RTX 3060) and `STRATA_PROMPT_ATTN_IMMA` (gate: CUDA `cc ≥ 80`, so sm_86).
  Upstream's speed numbers on *their* RTX 3060 were +15% story / **+37% code** (IQ3_XXS), and
  their own agent-loop sample said **fewer loops finish**: 4.5/10 at 1.5 over 4 runs against
  5.6/7 with it off. Nobody has run either number on the **IQ1_M, 256-of-512-expert** pack this
  box serves, which is the pack pi answers from (`PI_PROVIDER=strata-services2`). So the claim
  is *"faster answers, possibly fewer completed loops"* and neither half is ours.
  **To measure** (run it as a detached `systemd-run` unit writing to a log, then poll — AGENTS.md
  rule 11): the same prompt through the served endpoint with `extraEnv` flipped,
  `STRATA_ROUTE_PRIOR=0` / `STRATA_PROMPT_ATTN_IMMA=0` in `strata.nix`, no rebuild (both are
  env vars read at start, `sm120_series.hpp`); `STRATA_DEFAULTS=026` is the one-step rollback
  for the whole 0.2.7 default set. Do not judge the answer quality from a banner — for the
  house rule see `crates/switch-gui/README.md`'s "A UI that refuses an edit must say so" and
  keep the two runs' `md5` of the completion, which is how the 2026-10-19 findings compared
  outputs. The journal says what resolved: the engine prints its sm_86 table line at start
  (`strata: sm_86 (Ampere): … switches on by default`), and
  `journalctl -u strata -n 50` after the switch is the receipt for which defaults landed.

- **The conversation cache: can it be bought with the swap file?** No, and the answer is
  settled, not open (asked 2026-10-27, with the 0.1.52 engine running): the receipts and the
  one lever that would open RAM honestly (`--resident-budget-gib`) are in
  `machines/services2/STRATA.md`, "The conversation cache cannot be bought with the swap file".
  What the same start measured: 33 of 37 requests already reuse 2.74 M of 2.95 M prompt tokens
  on `--prompt-cache` alone, `engine/vram_free_mib = 408`, 2,229 MiB of the swap file already
  spent on the arena, and `available_host_bytes()` never counts swap on Linux. **Open** half:
  nobody has tried `--resident-budget-gib` here, so the file-read cost of opening headroom is
  unmeasured — the tier line reads `files 0 blobs 3939.5 MB read (the GGUF in place)` today.

- **The Vega 8's one plausible seat has never been tried: the picture reader, not an
  expert tier** (found 2026-10-24 while answering "can the iGPU be a co-processor to the
  3060" — the answer to *that* is in `machines/services2/STRATA.md`, "The Vega 8 (Cezanne,
  gfx90c) is not the expert tier", and it is closed upstream, #514 *not planned*: two backends
  in one process, and an iGPU reads the CPU's DDR5, so the expert tier gains nothing).
  What the same maintainer sentence leaves open is the compute-heavy path: `tools/image/qimage.py:74-96`
  lists `gfx902 gfx909 gfx90c` (Raven/Renoir/Cezanne) as APUs and gives them `backend: vulkan`
  for the denoiser, and an AMD engine folder upstream carries a GPU picture reader
  (upstream calls it `strata-vision-hip`: llama.cpp's HIP backend, built from
  `tools/vision/CMakeLists.txt` with `STRATA_VISION_HIP` / `STRATA_VISION_HIP_ARCHS`, and
  `strata_control/vision_reader.py:1,5` is the rule that picks one — an AMD engine folder gets
  `strata-vision-hip`, an Intel Arc gets `strata-vision-sycl`, everything else the CPU reader).
  **This box deploys none of them**: `machines/services2/strata-package.nix:100-106` builds
  `-DSTRATA_ENABLE_CUDA=ON -DSTRATA_ENABLE_HIP=OFF -DCMAKE_CUDA_ARCHITECTURES=86`, the package's
  `bin/` is `strata`, `strata-device`, `strata-prep`, `strata-server` (+ `libstrata_kq.so`), and
  the engine's config is `env = { CUDA_VISIBLE_DEVICES = "0" }`, `lib_dirs =
  [ "/run/opengl-driver/lib" ]` — one CUDA device and the CUDA runtime, nothing AMD.
  **Blocks:** an engine package change, so the bump ritual in `machines/services2/STRATA.md`
  ("Updating the engine") applies, `just check-build` before it; and because the rule above
  *picks* a reader by what the engine folder carries, adding a GPU reader changes what
  `strata-server` uses — test the fallback after, do not assume it.
  **What to try, in the order that costs least:**
  1. the arch define on the *reader's* build, not on the engine's: `STRATA_VISION_HIP=ON` and
     `STRATA_VISION_HIP_ARCHS=gfx90c` belong to llama.cpp's own HIP build (`tools/vision/`), so
     adding them does not touch `cmake/hip_backend.cmake`'s accepted-arch lists — those stay
     `gfx1100`/`gfx1201` validated, `gfx1101`/`gfx1200` community, opt-in `gfx1010…gfx1152`, and
     anything else is still a `FATAL_ERROR`.
  2. the shape to copy is aibox's, which is **not** a dead end: `machines/aibox/strata-package.nix:9-12`
     says its iGPU reports gfx1035, which is not in the accepted list, so the engine there is built
     for **gfx1030** — the nearest accepted arch — behind a deliberately restricted `rocmPackages`
     scope (lines 88-111, `cl`/`hipblas`/`rocblas`/`hipblaslt` only, because the default scope pulls
     every gfx target). services2's package has **no** `rocmPackages` at all (0 hits against aibox's
     11), and `find /run/current-system/sw /run/opengl-driver -maxdepth 4 -name 'libhsa*' -o
     -name 'libroc*'` prints nothing: the runtime is the missing half on this box, not the ISA.
  3. a Vulkan reader — the backend `qimage.py` gives an APU — needs the *loader*, which this box
     does not have: `/run/opengl-driver/lib/` carries the driver `libvulkan_radeon.so` (and its
     25.0.0-rc6 siblings), while `find /run/current-system/sw /run/opengl-driver -name 'libvulkan.so*'`
     prints nothing in either `lib` or `lib32`. The CUDA path works without a loader because the
     driver's `libcuda.so.1` is put on `LD_LIBRARY_PATH` by that `lib_dirs` line above.
  **What the nearest measured analogue is worth** (upstream's own bench, `docs/AMD_HIP.md:594`,
  read at the 0.1.48.1 tree): an AMD iGPU picture reader is a real win, not a marginal one — the
  same reader built for gfx1103 ran on a Radeon 780M at **0.40 s for 260 tokens against 2.42 s on
  its CPU**, and on the discrete R9700 57 ms / 0.40 s against the CPU reader's 3.13 s / 14.1 s,
  with embeddings agreeing at cosine mean 0.99985. What is *not* measured is Cezanne: this box's
  iGPU is 8 CUs of GCN5.1 (`simd_count 32`, `simd_per_cu 4`, `gfx_target_version 90012`) against
  aibox's 6 CUs of RDNA2 gfx1035 (`machines/aibox/STRATA.md:79`) and upstream's RDNA3 gfx1103, and
  no gfx90c compute kernel has ever run on this box — `amdgpu` is loaded and its only boot line
  about the card so far is `amdgpu: ATOM BIOS: 13-CEZANNE-019`, i.e. the display path, and the
  engine's device probe on this box answers `device 0: NVIDIA GeForce RTX 3060` (`11.6 GiB`) alone.

- **`gpu-unlock` (the NVIDIA driver's own half-precision tensor cap) is a prefill-only win on
  this card, and it costs a boot-critical kmod: nothing was tried, and the deciding measurement
  has never been run** (found 2026-10-25 answering "can `git@github.com:Zagitta/gpu-unlock.git`
  speed up strata?" — `github.com/Zagitta/gpu-unlock`, issues #13/#31/#75/#78/#21 read at its
  `main` that day). **Nothing deployed, nothing in tree:** no module, no flag, no blob; the
  finding is that the mechanism exists, works on our exact PCI ID, and does not pay where this
  box spends its time. Full receipt — the selector numbers (`SS0`/`SS1`, the 1 FMA2/clock/SM of
  FP2048 against FP64's 2 and DP4A's 4), the upstream measurements on this card family (26 ->
  48 TFLOPS GEMM but **1.20-1.32x** on real inference at the same 170 W default; issue #31,
  the one Strata report: "4x RTX 3060 12GB running Qwen3.8 Flash Next IQ4_XS on **Strata**.
  Prefill increased from 450 to 600, but decode performance barely changed"; issue #75's
  decode-0%-bandwidth-bound half; issue #78's NixOS + `595.99.02` **open kernel modules** report
  against our `595.104.02`) and the four things it would cost — **`machines/services2/STRATA.md`**, "The card's half-precision tensor ceiling is a *driver* limit".
  **What blocks it, in order:** an out-of-tree module built against our kernel and loaded *in
  the initrd* before `nvidia` binds (neither half is in our initrd today — grep the
  `/boot/EFI/nixos/c2zwr…-initrd-linux-6.18.56-initrd.efi`, an uncompressed newc cpio, for
  `gpu_unlock` and for `lib/firmware`: both 0 — and `hardware-configuration.nix:26` is `boot.initrd.kernelModules = []`; the ordering holds today only because `nvidia` is *also* absent, which is exactly what upstream's issues #11/#76 break); a kernel command line naming our bus id (`gpu_unlock.targets=0000:01:00.0`, the list at `hardware-configuration.nix:35`); and its blobs, which `make firmware` extracts from **CachyOS's proprietary `nvidia-x86_64-610.43.03` package** with `firmware/NOTICE.md` saying NVIDIA's redistribution is not approved — so it is *not* a flake input and cannot be one, and this flake's every-input-is-a-network-source property is what breaks first. A failed unlock that already holds the card's BAR leaves `nvidia.ko` failing `request_mem_region` — no driver, no GPU for `strata.service` — and a bad module in the initrd is an unbootable box.
  **Blocks:** nothing today; an experiment, if someone wants it, must budget as a real deploy — reboot with the module + flag, read back in the *same* shell invocation (`journalctl -k -b | grep gpu_unlock` must print `unlocked`, and `nvidia` must bind after it), keep an older generation to boot into, and note upstream's own GA106 LHR failure at an older commit (`-110`, two 3060 LHRs) versus the merged PR #13 that carries the two relaxed checks a 3060 needs.
  **What to do instead, cheapest first:** one dense BF16/FP16 cuBLAS GEMM on the card with the CUDA toolchain `packages.strata-cuda` already puts on this box — stock ~26 TFLOPS means the cap is live and the only remaining question is strata's GPU share of prefill (the engine's own `prefill plan inputs` line answers that); stock ~48 means the card is not paying it and there is nothing to win. Nobody has run either number here. **Do not reach for `nvidia-smi dmon -s c` to see the selectors** — upstream's own "check what you have" line: this box's `nvidia-smi` (595.104.02) prints only `mclk`/`pclk` for `-s c`.
  **The free half:** the cap covers *only* half-precision tensor math — FP32 FFMA and integer DP4A never pay it, which is why llama.cpp (upstream's own null there: pp512 1460.5 -> 1467.1) sees nothing and Strata does not. An upstream integer/DP4A prefill path would therefore buy the same thing with **zero boot risk**; nothing in `engineArgs` gives it today, and if it ever appears this item is the way back.

- ~~**`strata3.int` is declared and not deployed, and meanwhile `llama.int` answers with the
  wrong engine**~~ — **done 2026-11-22**, in the order set out below, each step read back.
  `gw` first (`just deploy gw`: exit 0, `/run/current-system` == `system-good`, zero failed
  units): `/etc/nginx/nginx.conf` carries the `strata3.int` server block and
  `grep -c llama.int` on it is **0**; `/etc/dnsmasq-int-vhosts.hosts` answers the name with
  all three door addresses (IPv4, public IPv6, ULA — the dual-stack rule holds); `getent
  hosts llama.int.leighhack.org` returns nothing, which was this item's own stated
  done-state. From the router, through the door: `/health` 200 and `/v1/models` **200 with no
  client key** — the only proof that `strataBearer` is actually injecting against this engine
  rather than merely configured — and `front-door-cert-check.service` still says Let's
  Encrypt, not minica. Then `services1`: gatus's own API on `:8999` now lists
  `Strata (services2, CUDA)`, `Strata2 (aibox, HIP)`, `Strata3 (services3, CUDA sm_61)` and
  **no Llama row**, so the row that was green about the wrong engine is gone. Text below kept
  as the record of the state that was closed.

  (measured 2026-11-22, merging services3's `strata3` work into `main`). The
  row, the DNS entry, the gatus check and pi's provider all moved in one commit, but that
  commit lives only in `main`: `gw` still serves the old door table and dnsmasq still answers
  the old name, so today `strata3.int.leighhack.org` does not resolve at all
  (`getent hosts` on gw: nothing) and the box is reached by IP. What is worse than a name
  that does not resolve is the name that does: **`llama.int.leighhack.org` still resolves (to
  the door) and still proxies to `10.3.2.213:8080`, and Strata took llama.cpp's port** — so
  that name now answers `{"service":"strata","model":"qwen3.8-flash-next-coder-iq1m","api_key":true}`
  with `/health` **200** and `/v1/*` **401**, because its row deliberately injects no key (the
  llama row's own note argued, correctly for llama.cpp, against pointing an unrelated engine
  at `strata_api_key`). So: the gatus `Inference` row for "Llama (services3, Vulkan)" is
  **green on an engine that is not llama.cpp**, and every llama.cpp client on the network is
  401-ing against a name whose documentation promises a different model. The merge's commit
  message says "`llama.int` is dead either way; that name is retired, not broken" — it is not
  dead, it is answering, and that is the state to end.
  **Blocks:** nothing that is running, but it is the reason the next `gw` switch should be
  this one rather than an arbitrary later one.
  **To do, in this order:** (1) build the engine at the new pin (`.#strata-cuda12`, on aibox —
  the item below), so services3 does not cross a name move and an unverified engine build in
  one switch; (2) push `main`, then `just deploy gw` and **read the rendered door**
  (`/etc/nginx/nginx.conf` for the `strata3.int` server block, `/etc/dnsmasq-int-vhosts.hosts`
  for the name — rule 21, never evaluate to find out what a change does) and run
  `sudo systemctl start front-door-cert-check.service` after it, which is the only check that
  tells a CA-issued cert from minica's fallback; (3) `just deploy services1`, whose gatus row
  moves with the table; (4) switch services3. Confirm the GOLDEN RULE way, in the same shell
  invocation, on each box — and remember on `gw` that a switch whose activation runs long is
  the one case the rollback timer becomes the failure (`docs/router-front-door-migration.md`).
  **Re-verify with:** `getent hosts strata3.int.leighhack.org` (one answer, the door),
  `curl -s -o /dev/null -w '%{http_code}\n' https://strata3.int.leighhack.org/health` (200),
  `getent hosts llama.int.leighhack.org` (**NXDOMAIN** — that is the done-state of this item),
  and `grep -c 'llama\.int' common/front-door.nix common/int-dns.nix common/monitoring-endpoints.nix`
  (0 on all three).

- ~~**services3's engine has never been compiled at the 0.1.63 pin, and merging that bump into
  the box's own work is what made it so**~~ — **done 2026-11-22: built, deployed, verified.**
  The box's own work is what made it so (opened 2026-11-22). `strata-unstable-sm61.drv` built
  on **aibox**, which is the box predicted below and was confirmed before starting (aibox
  holds `cuda12.8-cuda_nvcc-12.8.93` and the previous engine output; services2 holds both
  CUDA-13 halves and neither of these), producing
  `/nix/store/gm5jwjykj0l4x61vqn8ays9d848lagi1-strata-unstable-sm61` with all four binaries —
  `strata`, `strata-device`, `strata-prep`, `strata-server`, the last two being exactly what
  the `terminal/` copy list and the vendored gguf-py could have broken, so their presence is
  the receipt for the reconciliation. `services3` then switched: `strata` active, its log
  reading **engine 0.1.63** (from 0.1.59.1), `strata3.int` 200 with `loaded: true`, GPU guard
  agreeing at `580.196.07`, zero failed units — and since a native (IQ) pack will not start
  without `--spec`, "loaded" is itself the confirmation that flag survived the bump. **What
  was not re-measured:** the 18.2 tok/s decode and 464-539 tok/s prefill band in
  `machines/services3/README.md` belongs to 0.1.59.1. Treat it as a prior until it is
  re-measured on this engine — there is no `just` verb for it on this box; the harness is
  `machines/services2/strata-tune/` (`strata-bench.py`, `strata-answer-score.py`), written
  for the 3060 and needing its paths and model name pointed at `/srv/strata` and
  `strata3.int` before its numbers mean anything here. The pack was *not* re-prepared,
  because the 2026-11-21 bump proved the pack, expert profiles and `draft_vocab.bin`
  byte-identical across the pin move.

  The original text follows as the record (opened 2026-11-22, the day services3's
  `strata3.int` work was merged into `main`). The box was built and brought up on the input
  as it stood at its own commit (`13eedf552`, pre-0.2.16), while `main` had moved `strata` to
  `3142b66b9` and to `flake = false`. So the merge had to reconcile a third
  `strata-package.nix` with a layout move it had never seen — `cmakeDir = "../engine"`, the
  `terminal/` copy list, the vendored gguf-py, and the `-DSTRATA_GGML_DIR`/`fetchFromGitHub`
  pair deleted — and those edits are **reasoned from services2's working package and from
  reading `3142b66b9`'s tree, not from a build**: nothing on this network has compiled the
  CUDA 12.8/sm_61 arm since the pin moved. The two things that could still bite, both read
  off the pinned tree rather than guessed: `STRATA_EXPERIMENTAL_SM60` and the sub-sm_75
  refusal it lifts are still there (`engine/cmake/backends/cuda.cmake` at that rev, so the
  flags are right), and `strata-device` still builds (proven by services2's installPhase
  succeeding at this pin). **Blocks:** the next switch of services3, which rebuilds its
  engine. **To do:** build `.#strata-cuda12` on **aibox**, which is the box that already
  compiled it — measured 2026-11-22: aibox's store holds
  `/nix/store/29cm6lmacqp2nqdvl44b84fn240ny4sc-strata-unstable-sm61` (1.8 GB, the
  2026-11-21 bring-up's output, which services3 delegates its builds there for) and
  `cuda12.8-cuda_nvcc-12.8.93`, while services2 has neither path. (That is deliberately the
  opposite of the aibox note below and of the 2026-11-21 bump's "build the CUDA half on
  services2": that one is about `.#strata-cuda`'s CUDA **13.0** toolkit, which only services2
  holds. One CUDA major per warm box, and the two notes must not be collapsed.)
  Then deploy services3 and read
  back `strata3.int.leighhack.org/health` and a live `/v1` completion; `strata-device
  --selftest` is the card check. If the build dies, the fallback is one line each way: pin
  the input back for the merge, or hold services3 on its current generation until the CUDA-12
  arm is proven — do **not** move this box onto CUDA 13 to make the build pass, which is the
  whole thing `machines/services3/strata-package.nix`'s header exists to forbid.
  Re-verify with: `git log --oneline -1 -- flake.lock`, on services3
  `systemctl show strata -p ExecStart --value` (the engine's store path, hence its pin), and
  on each candidate builder
  `ls -d /nix/store/*-strata-unstable-sm6* /nix/store/*cuda12.8-cuda_nvcc*` — whichever box
  answers is the warm one, and both answers were the same on 2026-11-22.

- ~~**aibox is still on engine 0.1.59.1 while the flake input is at 0.1.63 — three
  releases behind, and the middle one is the one its chip was waiting for**~~ — **done
  2026-11-22.** `just deploy aibox` (exit 0, `current == system-good`, zero failed units,
  kernel `7.3.4` matching its generation), and `/var/lib/strata/strata.log` reads **engine
  0.1.63** with `strata.service` active and `strata2.int.leighhack.org/health` 200
  `loaded: true` afterwards. **Its stated blocker had already gone**: the 2026-11-21 note
  below says the switch was held off because the root filesystem was at 100 %, and reading
  the box before deploying showed **79 GiB free (91 % used)** — so the gate was somebody
  having reclaimed the space, not a change in this flake; re-read `df` rather than inheriting
  the blocker. The netboot squashfs re-export (the activation trap that makes aibox exit 4 on
  a moved bind-mount source) was handled inside `just deploy`'s lazy unmount, and the Pi Room
  clients were left alone — `findmnt` shows the export pointing at the new store path and
  they pick it up **on their own next boot**, which is a decision, not a side effect.
  All three engines are now on the one input, which is what the sentence below was written
  to achieve.

  (split opened 2026-11-18 at 0.1.61, retitled 2026-11-21 when the input moved to 0.1.63:
  `nix flake update strata` was taken and deployed to services2's CUDA engine and
  **not** to aibox's HIP one, and `one input, both engines` is what `flake.nix` says —
  **three** engines now — so leaving this open is what makes the two engines out of step).

  What is proven, so the next person starts from receipts rather than from the
  changelog: the HIP package **builds** at the new pin — built on aibox on 2026-11-21
  from a working copy of the bump (`nix build .#strata`, one derivation, ~5 min, out
  path `wd40i8bgr2dg0hwc2j5c1544r6cr1pws-strata-unstable`) — which means the 0.2.16
  layout rework in `machines/aibox/strata-package.nix` is real: `cmakeDir =
  "../engine"`, the `terminal/` copy list, the vendored ggml/gguf-py instead of a
  `fetchFromGitHub`, and the **deletion of `-DSTRATA_PREFILL_MMQ=ON`** (0.2.16 removed
  that option; `docs/AMD_HIP_PERFORMANCE.md` says so and points anyone who wants MMQ
  back at the `v0.2.15` tag). And aibox's running engine was read back as
  `session is up (engine 0.1.59.1)` on 2026-11-18, so nothing is broken — it is
  simply the older side of the same input.

  Why it should not be left, in the order the read will price them:
  1. `9783e09f9` (in 0.1.61, so already owed) makes **gfx1030 / 1031 / 1032 — aibox's
     device — read its prompt experts on Strata's own int8 kernels by default**, for
     expert groups of 256 entries and up (~512 tokens and more), measured at **1.10 to
     1.35 × MMQ** at 2,048 / 8,192 / 20,000 tokens for IQ2_S / IQ2_XXS gate-up with a
     Q2_0 down. That is exactly aibox's pack, and prefill is exactly aibox's wall
     (`machines/aibox/STRATA.md`: "the real limit for coding use is prefill", ~45
     tok/s whatever the config). Upstream's caveat is in the commit message and must
     be read as the reason to measure rather than as permission to assume: *"No
     end-to-end run on a gfx103x card with a model has been made (needs the lender's
     word)."*
  2. The MMQ path is now **gone from the build**, so that default is not a switch
     beside the old one — the old one is not compiled in. `STRATA_EXPERT_I8=0` is the
     one-line runtime rollback of the *int8* half; there is no rollback of MMQ's
     removal short of an older pin.
  3. `87c9821bb` "HIP Linux: keep the kernel's NUMA balancing off the engine's
     memory" and `8d92f0a38` "HIP: the arch choices of the prompt path and the
     drafters are made per card, not by the first card that asks" are both new lines
     in this box's start block, and aibox is a single-socket box whose "VRAM" is its
     own DDR5 — read the start lines rather than assuming they changed nothing.

  **How to take it**, in the order that doc already demands: `just deploy aibox` in a
  window when nothing is building and the bot/voice tier can lose its 262144 endpoint
  for a minute or two (aibox is the network's builder and runs 52 of 59 GiB with the
  arena loaded, so its engine restart is not free for anyone — and note the
  2026-11-14 receipt in `machines/aibox/STRATA.md`: the netboot bind mount must be
  umounted *before* the switch or the activation dies and auto-rolls the box back).
  Then read the start line for the sentence that says the card class took the new
  default, compare **cold-to-cold** against the plateau that doc names (225–231 ms per
  window) because the cold/warm boundary moved under it once already, and do not judge
  the first start after any bump (that one pays the cold-page cost, ~3 s → 2979/3374 ms
  recorded there).

  Re-verify with:

  ```bash
  sudo grep -haE "session is up|prompt experts|int8 kernels|tail skip" /var/lib/strata/strata.log | tail -6
  nix flake metadata --json | python3 -c "import json,sys; print(json.load(sys.stdin)['locks']['nodes']['strata']['locked']['rev'])"
  # ...and the same `rev` short sha must appear in aibox's running engine's store path:
  #   systemctl status strata --no-pager | grep -oE '[a-z0-9]{32}-strata-unstable'
  ```

- **services2 runs decode 13 % faster than upstream's 0.1.63 default, on purpose,
  and the setting is one this fleet now owns** (decided and deployed 2026-11-21,
  receipts in `machines/services2/STRATA.md`, "Update: engine 0.1.61 → 0.1.63").
  `env.STRATA_ROUTE_TAIL_SKIP = "7"` in `machines/services2/strata.nix` opts out of a
  default upstream flipped to off in 0.2.16/0.1.63 because *answers differ slightly*
  (upstream's KL against exact routing on an RTX 3060: 0.0058, perplexity x1.011,
  HumanEval not separated; ours: 9/9 on the answer key either way, prefill flat,
  decode 46.6 against 39.3 tok/s). Two things to watch, neither urgent: **upstream can
  retire the setting**, the way it retired `STRATA_PREFILL_MMQ` in the very release
  this bump crossed, and then the line is dead config that only the start line will
  expose (`sudo grep "ROUTE_TAIL_SKIP" /var/lib/strata/strata.log | tail -1` must keep
  printing the `is ON` sentence); and **`strata-answer-score.py` is too coarse to be
  the gate here** — nine greedy arithmetic/one-line items cannot see KL 0.006, so what
  actually protects this decision is pi's usage, and if completions ever read worse on
  this endpoint the revert is deleting one line and `just switch`.
- **services2 now runs its last ~1.5 GiB of RAM on the streaming K/V, so `LowMemory`
  fires there — fix the rule to know this box, or buy the DIMMs** (deployed 2026-11-18,
  the config and every number in `machines/services2/STRATA.md`, "256K at full precision,
  at this box's speed").

  `--max-context 262144 --kv int8 --kv-resident 32768` with
  `STRATA_RESIDENT_HEADROOM_GIB=2` reads an 8k prompt **13 % faster** and a 28k prompt the
  same as the old 131072/int8 config, at full int8 precision, with the expert cache *up*
  from 1781 slots to 2332 — and the bill is host RAM: 21.88 GiB of experts page-locked +
  3.09 GiB of pinned K/V + ~4 GiB of OS and monitoring against a 30.7 GiB box, so
  `free -m` reports **1.1–1.6 GiB available (~4 %)** where it used to report ~3.9 GiB
  (12.7 %).

  Consequence, live now: `LowMemory` is
  `node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes * 100 < 10` for 5m
  (`machines/services1/services/observability.nix:101`), i.e. 3.07 GiB on this machine,
  so it will sit firing on a box that is behaving exactly as designed.  Two ways out, and
  they are not the same decision:

  1. **Make the rule know what a strata box's `MemAvailable` means.** On this box most of
     "unavailable" is the page-locked expert arena, which is the point, not a leak — so a
     single fleet-wide percentage cannot describe it.  The minimal change is a per-instance
     threshold (e.g. `and on instance != "services2"` at 10 %, with a 3 % rule for it, or
     an offset that subtracts `node_memory_Shmem_bytes` held by `strata`).  **This is a
     decision about when someone gets paged, so it was left undecided deliberately** — the
     arithmetic is in the doc, the edit is one `expr` line.
  2. **Buy the RAM, which retires the whole question.** `dmidecode -t memory` on services2:
     **two DIMM slots, both filled with 16 GiB DDR4-3200 (M378A2K43DB1-CVF) running at
     2933, board maximum 128 GiB** — so it is a 2×32 GiB swap, not an added stick, and it
     matches upstream's own measured shape for this lever (12 GB card + 45 GiB RAM: 756 →
     1267 expert slots, 32K prompt 21–34 s → 12–17 s).  After it, put
     `STRATA_RESIDENT_HEADROOM_GIB` back to 4 (or delete it: 4 is `--resident-experts`'
     default) and re-check the gate line below.

  The one line that says whether any of this is healthy, on any start:

  ```bash
  sudo grep "lendable slots" /var/lib/strata/strata.log | tail -1
  # must read: 1533 of the prompt path's 1533 lendable slots keep their experts in RAM too
  # if it reads "N of M … (P %) will read their experts from the pack", the prompt path is
  # reading experts off the NVMe and the 8k read costs 562-917 tok/s instead of ~1210
  ```

  **Blocks:** nothing for the config (it is deployed and confirmed); the alert fix blocks on
  a choice about paging, and the RAM fix on a part.  Until one lands, decide which of
  "alert permanently red on this box" and "give 256K back" is the smaller harm — the
  second is a three-line revert (`"--kv"` to `q4_0`, delete the `--kv-resident` pair and
  the headroom line).

## Machines / access

- **aibox's switch fails on `exports-netboot-squashfs.mount`, reproducibly, and it is not
  the change being deployed** (2026-10-28, twice in one hour: both `nixos-rebuild switch`
  runs ended `Failed to restart exports-netboot\x2dsquashfs.mount` →
  `switch-to-configuration` exit 4 → the whole switch reports failure **after**
  `/run/current-system` flipped and after the new units started, with `auto-rollback.timer`
  armed and firing minutes later). The mount itself is fine — still `active`, still mounted,
  `systemctl list-units --failed` empty — and re-running
  `/run/current-system/bin/switch-to-configuration switch` exits 0 on the second attempt,
  which is the fix and also the proof that nothing else was wrong. It was read as a busy
  restart of the Pi Room netboot export (real machines NFS-reading it) racing the activation;
  **that reading is wrong, see the correction below.** **Blocks:** nothing, because the retry works; but every future aibox switch
  pays a manual second activation and the judgement call on a *failed* activation (never
  confirm one) with a 5-minute reboot timer running. **To do:** read
  `machines/aibox/netboot.nix` and give that mount what a busy restart needs — the usual
  answers are `umountOptions = "--lazy"`/`timeout` on the mount unit, or not marking it
  restartable when its contents are a store path clients hold open.
  **Corrected 2026-10-29 by the third and fourth occurrences (a deploy at 11:58 and my own
  retry two minutes later): the "real machines reading it" story is wrong, and so is the
  reproduction instruction.** `journalctl -u exports-netboot\x2dsquashfs.mount` shows the
  failing step is the **unmount**, not the mount — `umount[…]: /exports/netboot-squashfs:
  target is busy`, status 32 — on every attempt back to 2026-10-26, including today with
  **zero NFS clients attached** (`ss -tn '( sport = :2049 )'` empty, `lsof +D` empty, both Pi
  Room boards powered off for days). So it does not need clients to reproduce; it reproduces
  every time. The busy-to-a-normal-umount / free-to-a-lazy-one split points at a kernel-side
  reference rather than an open file, and the obvious holder is **nfsd itself**: `exportfs -vs`
  lists `/exports/netboot-squashfs` with `nohide`, and this is a bind mount **of a store path**
  — so the export pins the very mount the activation is trying to move. **The workaround that
  works first time, confirmed today:** `sudo umount -f -l /exports/netboot-squashfs` then
  `sudo systemctl start exports-netboot\x2dsquashfs.mount` — which is what `just
  switch-netboot` already does with its opening line, and is why that recipe exists.
  **The consequence that matters more than the failure:** between the failed restart and
  noticing, the box is completely healthy-looking — `active`, no failed units, nginx serving
  the new kernel/initrd and the new menu — while `/exports/netboot-squashfs` **still bind-mounts
  the previous generation's squashfs**. A client then boots a new kernel and initrd against the
  old store, which is the exact generation-skew `netboot.nix`'s constant `init=` path exists to
  tolerate, so nothing errors; it just silently tests the wrong image. After any aibox switch,
  check `findmnt -no SOURCE /exports/netboot-squashfs` against
  `nix eval .#nixosConfigurations.pi-room-sys.config.system.build.squashfsStore.outPath` before
  believing a netboot test. **To do, in order of confidence:** make the export a stable
  directory holding a symlink (or a copy step) instead of a bind of a store path, so a switch
  updates a symlink rather than remounting an nfsd-pinned mount; failing that, order the mount
  restart around nfsd (stop `nfs-server`, remount, start it) rather than racing it.
  **Receipt that the pre-emptive lazy umount is enough (2026-11-14, the strata bump):** doing
  `umount -f -l /exports/netboot-squashfs` **before** `nixos-rebuild switch` inside one detached unit
  (`systemd-run --unit=aibox-sw`, so the confirm stays chained) made the switch exit **0** first time:
  activation restarted the mount itself, `findmnt -no SOURCE` then names the **new** generation's squashfs,
  and `auto-rollback` never fired. That is the recipe in practice — `machines/aibox/STRATA.md`, "Deploy
  record, 2026-11-14" — and it does not close this item, because the next operator has to remember to do it
  by hand; the declarative fixes above still own the problem.

- **The boot path resolves a name, and it does not have to** (2026-10-26, the same
  investigation). Every URL in
  `machines/aibox/netboot.nix`'s iPXE menu — `bzImage`, `initrd`, `logo.png` — and the
  `dhcp-boot=` URL `machines/gw/lib/network-tables.nix` hands the client are built from
  `aibox.int.leighhack.org`, so a PXE client cannot start without the router's DNS being
  right about both address families. The store mount already uses the IPv4 literal for the
  documented reason ("DNS is too slow so use IP address", `machines/pi-room-sys/README.md`),
  and the address is already in `common/hosts.nix`, which both files could read: the whole
  change is `nfsServer` becoming that address and the boot URL interpolating the same
  `tftpServer` `netbootLines` already takes. **Blocks:** nothing today — the addresses are
  true as of this fix. **Needs:** a decision about defence-in-depth versus one more reader
  of the table. It is what stands between the next wrong AAAA and a dead room: the SeaBIOS-shaped
  test client — whose iPXE has no IPv6, so it never tried the dead address — booted from this
  same VLAN on this same afternoon while the UEFI boards could not. Do not do it by editing a
  literal in two files.

- **The boards never load the NBPs aibox serves — and the boot survives only because of
  the blank `autoexec.ipxe`** (found 2026-10-26 with a raw TFTP client; answered the same
  afternoon by `just pxe-client-uefi`, which is the thing that finally made it
  observable). The server half is right and should not be "fixed": `in.tftpd … /etc/tftp`
  accepts `RRQ "/etc/tftp/ipxe.efi"` and refuses `RRQ "ipxe.efi"` with `ERROR 2: Only
  absolute filenames allowed`, so the absolute path in
  `machines/gw/lib/network-tables.nix`'s `netbootVariants` is what this server requires and
  `machines/gw/README.md`'s "the path belongs to aibox" is correct.

  What one UEFI boot actually does, read off `journalctl -t in.tftpd` — **that** filter,
  because the lines carry the syslog identity `in.tftpd` and `journalctl -u tftpd` shows
  none of them, which is the trap in reading `in.tftpd -v` for the first time:

  ```
  RRQ from 10.3.14.208 filename /etc/tftp/ipxe.efi
  tftp: client does not accept options
  RRQ from 10.3.14.208 filename /etc/tftp/ipxe.efi
  RRQ from 10.3.14.208 filename /etc/tftp/autoexec.ipxe
  ```

  and, from the guest's own console:

  ```
  >>Start PXE over IPv4.    NBP filename is /etc/tftp/ipxe.efi
  iPXE initialising devices...
  autoexec.ipxe... ok
  iPXE 2.0.0 -- Open Source Network Boot Firmware
  Filename: http://aibox.int.leighhack.org/boot/netboot.ipxe
  ```

  So the firmware's PXE stack asks for the NBP, **its TFTP client refuses `in.tftpd`'s
  OACK**, it retries the same name, then takes the blank `autoexec.ipxe` — and the iPXE
  that goes on to run is the one **built into the firmware**, not the 309 KB `ipxe.efi`,
  which was never executed. Three consequences worth writing down. (1) `ipxe.efi`,
  `undionly.kpxe` and the vendor-matching in `common/ipxe.nix` are decorative, which is
  why the "we build for the wrong NIC vendor" problem never once mattered. (2) The blank
  `autoexec.ipxe` is what converts a failed NBP transfer into a working boot: deleting it
  breaks the netboot, and its comment is a fact about behaviour, not cosmetics. (3) The
  option negotiation is the actual defect, and its only observed cost is one wasted
  retry and a log line.
  **Blocks:** nothing that works. **Needs:** nothing urgent — and if it is ever chased,
  chase it with `just pxe-client-uefi` watching `journalctl -t in.tftpd`, not on a board,
  because there is no way to test it here that does not end with the boards as the test
  group. Changing `netbootVariants`, `in.tftpd`'s flags or either binary on the strength
  of this note alone is how a working netboot gets broken.

- **services2 cannot hop out.** `machines/services2/README.md` used to claim the
  shared machine-hop key was installed there; it is not. Only `authorized_keys` is
  (by `common/users.nix`), so you can hop *in* to 10.3.1.21 but it cannot ssh to
  services1, and it has no GitHub credential of its own. Fetch there with `ssh -A` (agent
  forwarding — `AGENTS.md`, "SSH between machines") rather than copying a private
  key around. Install `~/.ssh/agent-hop-key` deliberately if it should hop out;
  never make it a side effect of a deploy. **(corrected 2026-11-22:** the first half of this
  item is stale — `machines/services2/README.md` verified on 2026-10-15 that the hop key **is**
  installed there and hops out, and `just repo-status` confirms it today; what is still true is
  the credential half, i.e. the bullet below**)

- **Two boxes still have no credential GitHub accepts, and the shape that gives a box one is
  now written down** (done for services3 on 2026-11-22, the first box that ever pushed by
  itself). Measured that day off all five boxes: `gw`'s and `services2`'s `git ls-remote origin
  HEAD` die at GitHub's own refusal — `Please make sure you have the correct access rights`,
  `Permission denied (publickey)` — because their `~/.ssh` holds only the hop key, while
  services1, aibox and services3 answer with a rev. The two halves of that hop and the map of
  which box reaches what are in `docs/nix-dev-workflow.md` ("2026-11-22: GitHub joined that
  table, and the credential half became per box") and, per box, `machines/services3/README.md`
  ("Getting commits on and off this box"). **Blocks:** nothing — pushing through services1 works
  and is what those two boxes do today, and `machines/services2/README.md` owns the relay dance
  and its two traps. **To do,** on whichever of the two next wants its own: `ssh-keygen -t ed25519
  -N '' -f ~/.ssh/id_ed25519` **on that box** (the name is what makes it work with no ssh config,
  since that is one of the file names ssh tries unprompted), then register the public half with
  `gh api -X POST repos/leigh-hackspace/infrastructure-nix-flake/keys -f read_only=false` from
  services1 — **a repo-scoped deploy key, not a key on the account**: `ssh -T git@github.com` on
  services1 and aibox answers `Hi cjdell!`, so an account key bolted to a box can read every
  private repo that account can, including the thingino firmware repo whose history carries the
  cameras' Wi-Fi PMKs (rule 30), and a deploy key can push and pull this repository and nothing
  else. `read_only=false` is load-bearing: a read-only deploy key *authenticates* a `git push`
  and then refuses the write, which reads as a broken remote rather than as the wrong grant. Then
  land the flake half — `common/ssh-known-hosts.nix` grew github.com's three published keys on
  2026-11-22 and is deployed on nothing yet, so a box's github host keys are still whichever
  `~/.ssh/known_hosts` someone ssh'ed from by hand, which no deploy can reproduce and a reinstall
  loses. Re-verify per box: `git ls-remote origin HEAD` answers a rev, `git status -sb | head -1`
  reads `## main...origin/main` (not `...infra/main` — see the `origin/HEAD` trap in
  `docs/network-upgrade-runbook.md`), `grep -c github /etc/ssh/ssh_known_hosts` goes `0` → `3`,
  `ssh-keygen -l -E md5 -f ~/.ssh/id_ed25519` matches the fingerprint that box's README records,
  and `gh api repos/leigh-hackspace/infrastructure-nix-flake/keys --jq '.[]|{title,read_only}'`
  on services1 (the grant is off-box state this flake cannot see, and `gh` is configured only
  there).

- **`machines/pi-room-sys/ssh_host_ed25519_key` is a private host key in plain
  git** (moved in with the tree on 2026-10-15; it was committed in
  `leigh-hackspace/pi-room-sys` before that, so this is not a new leak — it is a
  new place to fix it). `netboot.nix` puts it in the netboot initrd as
  `boot.initrd.network.ssh.hostKeys`, so it is needed at *build* time: it should
  become a sops secret rendered through `config.sopsSecretText`
  (`common/sops.nix`, which already exists for exactly this) and then be
  **rotated**, because anyone who can PXE can read the initrd and the same file is
  in git history in two repositories. While it is a plain file it is also why
  `nix flake check` cannot run without a checkout of `machines/pi-room-sys/`:
  the path literal is copied at evaluation. Related, same file: `emergencyAccess =
  true` gives an unauthenticated root shell in the initrd, which is a lab
  convenience on the Pi Room VLAN and still worth knowing about
  (`machines/pi-room-sys/README.md`).

- **services3 owes four things, and three of them are the rack** (joined the flake
  2026-11-11, assimilated from another fleet (`cjdell/nixos-config`, where it was
  `asus-i7-6700-nixos`); the box's own file is `machines/services3/README.md` and this
  is only the network's half). **Blocks:** nothing that is running.
  1. ~~It answers on **VLAN 226** (`10.3.2.213`, reserved by its USB Wi-Fi dongle's MAC),
     so it is the one services box not in 10.3.1.x.~~ **Done 2026-11-25**: the box is racked,
     cabled into a VLAN-225 **trunk** port, and answers `10.3.1.22` with the wired MAC
     reserved in `common/dns-records.nix`; `./vlan225.nix` replaced
     `./networking.nix` (deleted) in the same commit that moved `common/hosts.nix`, and
     `gw` was deployed first so the reservation is the router's before the box asks for
     its lease. Two findings the plan got wrong, both written into
     `machines/services3/README.md`: the box needs a **boot**, not only a switch (a
     `.link` rename is applied by udev when the device is (re)created, so the VLAN child
     does not exist until the reboot), and the plan's re-verify named
     `services3.int.leighhack.org`, a name that never existed — the record's name is bare
     `services3`, the `*.int` name this box carries is `strata3.int`.
  2. It is **not in `machines/gw/ups.nix`'s `shutdownTargets`**, and that is a fact about
     nobody having checked what it is plugged into, not a decision to leave it out. It
     joins the list (in the order `docs/ups-and-fleet-shutdown.md` argues for) when it is
     racked on the same load as the others.
  3. Its Prometheus scrape rows are written but **not yet scraping**: they live in
     `machines/services1/services/observability.nix`, so they land on services1's next
     switch, and a `dry-activate` of exactly that change showed it would restart six of
     services1's apps (the crates' unit paths move with the flake source). Not a fee
     worth charging a new box on the same evening; it rides the next services1 deploy.
     Until then `10.3.1.22:9100/9558/9633` answer and nobody reads them.
  4. ~~Both `services3` and `gw` are activated from an uncommitted tree.~~ **Committed
     2026-11-11**, which is why both boxes are now `git log -1`-checkable (rule 20) and
     `just deploy` of either reproduces what runs instead of reverting it.
  5. **The ssh *identity* that makes a `nix copy` to a new box work is hand-written.**
     `sudo nix copy --to ssh://…` runs as root out of `HOME=/root`, and root's ssh only has
     an identity for hosts named in `/root/.ssh/config` — which had a stanza for aibox and
     nobody else, so the copy to services3 died with `Too many authentication failures`
     (after an earlier, separate `Host key verification failed` caused by a stale `.213` line
     in `/root/.ssh/known_hosts`, which ssh reads *before* the declarative
     `/etc/ssh/ssh_known_hosts`). Rule 25 made the *keys* data; the identity is still a file
     on one box, edited at 23:50 by whoever is unlucky next. **Blocks:** nothing — the
     workarounds are known (`Host` stanza, or `sudo env NIX_SSHOPTS='-i … -o
     IdentitiesOnly=yes' nix copy …`). **To do:** decide it in `common/ssh-hop.nix` (the
     operator-account block already lives there and is keyed to `common/hosts.nix`; the root
     half needs an **absolute** `IdentityFile`, which is why it has not been folded in
     quietly — see item 3 of the shared-store item above), or root
     gets the hop key declaratively; then delete the hand-written file. Re-verify: `sudo ssh
     -o BatchMode=yes leigh-admin@<a new box> true` on a box whose `/root/.ssh/config` you
     have not touched.
  6. **Disk: 9.7 GiB free** with the engine and driver installed. The recipe is in
     `machines/services3/README.md` and it destroys the rollback path to the folding@home
     configuration, so it is a decision. Nothing is blocked until a nixpkgs bump lands.
  7. ~~`llama.int.leighhack.org/health` is not in `common/monitoring-endpoints.nix` yet~~ —
     **done 2026-11-15**: `strata.int`, `strata2.int` and `llama.int` `/health` are now a
     gatus **Inference** group, because the two inference outages of the 2026-11-15 repin
     were both 502-ing with zero failed units and the board had no row that could have shown
     either. Probe **`/health`, not `/`**: llama.cpp's UI is gzipped into the binary and `/`
     answers `415` to a client that sends no `Accept-Encoding: gzip`
     (`machines/services3/README.md`). **2026-11-21:** that third row is
     `strata3.int.leighhack.org/health` now, with services3's llama.cpp gone (the row's `name`
     says "Strata3 (services3, CUDA sm_61)"). What is still open is the other half: see the
     next item.

- **`ai.int.leighhack.org` and `ai.leighhack.org` answer 502 as the designed steady state, and
  no check can say so** (found 2026-11-15; the endpoints were probed while diagnosing the
  repin's inference outages, `docs/work-log-2026-11.md`). Both door rows proxy to `aibox`'s
  llama.cpp fork, and `machines/aibox/strata.nix` clears that unit's `wantedBy` because
  llama-server's ~33 GB of GTT and strata's 35.5 GiB arena cannot be resident at once — so
  with strata on the box (the deployed state since 2026-10-28) the two names 502 forever,
  and their own comments already say they should. The gap is not the 502, it is that
  `common/monitoring-endpoints.nix` cannot express "502 is correct here": every `mkHttp` is
  `>= 200 && < 300`, so the choices are a permanently-red row (worse than none — it trains
  everyone to ignore the board) or silence, and today it is silence.
  **To do:** give `mkHttp` an optional per-endpoint expectation (`condition`, or a `status`
  argument already supported by gatus's own schema) and a row that asserts the *right* answer
  per the door's current owner of `aibox`'s GPU — which means the check needs to know which
  engine is wanted, so the honest form may be a row on `strata2.int` only plus a comment here
  saying `ai.*` is a known-502 alias. **Blocks:** nothing. **Re-verify with:** `curl -s -o
  /dev/null -w '%{http_code}\n' https://ai.int.leighhack.org/` (502 while strata owns aibox),
  `systemctl is-enabled llama-server.service` on `aibox` (currently `linked` = not wanted),
  and `grep -c 'ai\.int\|ai\.leighhack' common/monitoring-endpoints.nix` (0 today).

- **`machines/services3/README.md` describes a box that no longer exists: it says "running
  nothing yet" and "20 GiB free", and both were untrue when the box was read on
  2026-11-12** (`llama-server.service` active since the 2026-11-11 engine work; disk at
  **9.3 GB free of 109 GB (91 %)**, GC'd to 84 GB the same day by the operator). The header
  line and the "What it runs" section were written before the engine landed, and the disk row
  predates a store whose growth nobody was watching. **Blocks:** nothing, but it is the file
  an agent is told to read *before* deciding whether that box can host anything — "20 GiB
  free" is the difference between "yes, try it" and "no". **To do:** update the header,
  "What it runs" and the Disk row, and add the thing the investigation actually needed:
  that the free space on this box is the constraint on every experiment, and that `/tmp` is
  on `/`. Re-verify with: `df -h /`, `systemctl is-active llama-server`,
  `grep -n "nothing yet\|GiB free" machines/services3/README.md`.

## Nightly units that are red on services1 (found 2026-11-03 while reading `is-system-running`)

`systemctl is-system-running` on services1 says **degraded**, and it has been saying it for
long enough that it stopped being news — which is the actual finding, because a permanently
red box is where a real fault hides. Two units, neither to do with anything that moved on
2026-11-02/03 (both timestamps predate the DNS deploy by an hour or more):

- **`update-containers.service` has failed every night since at least 2026-10-24 — ten
  consecutive nights.** `Error: unable to copy from source docker://uosserver:c9603dec9010:
  reading manifest … in docker.io/library/uosserver: requested access to the resource is
  denied`, exit 125. The image name is the bug: Ubiquiti does not publish UniFi OS Server
  under `docker.io/library/`, so the nightly updater asks Docker Hub for a repository that
  will never answer, and every container after it in the list is never looked at (the run dies
  on the first failure). Read `docs/unifi-os-server.md` before touching the pin — that doc
  already records the appliance as a *downgrade* of the running controller, so "update the
  image" is not the fix; not pulling it may be.
- **`borgbackup-job-backup-srv.service` failed 2026-11-01, -02 and -03 in a row** (6 failures
  across the journal, so it is intermittent-but-current), always with
  `/srv/frigate/config/frigate.db-wal: file changed while we backed it up`, exit 1. Borg's 1
  is the *warning* class, so an archive is still created — but the thing inside it is a torn
  copy of a live SQLite WAL, which is a backup of `frigate.db` that nobody has promised can be
  restored. The honest fix is one of: stop backing the database up file-by-file (dump it first
  with `sqlite3 .backup`, which is what the Frigate docs want anyway), or exclude `*.db-wal`
  and take the whole directory as a consistent pair. Either is a backup-policy decision, so it
  is written down rather than taken at 01:40.

Both are nightly in the sense that matters for the rule this repo keeps about red things: they
make `just switch` output and any `is-system-running` check start out-looking-bad, so the first
thing to do with either is to date it (`journalctl -u … | grep "Failed with result" | awk
'{print $1,$2}' | uniq -c`) before deciding it is today's fault.

## DNS / monitoring
- **`pete-voron.leighhack.org` proxies to `10.3.2.110`, which is inside VLAN 226's *dynamic*
  pool** (opened 2026-11-23 with the row, `common/front-door.nix`). Everything else the door
  reaches on CS_VLAN is a `common/dns-records.nix` reservation (`voron` 10.3.2.50,
  `snapmaker` .56, `cnc-1` .61) precisely so the address cannot be handed to another device;
  this one was added as an address because that was the address, and dnsmasq may re-lease it.
  **Blocks:** the device's MAC (or client-id) — read it from the box or from
  `/var/lib/dnsmasq/dnsmasq.leases` on gw and add the row, the same one fact every other
  VLAN 226 device is. Re-verify with: `grep -n '10.3.2.110' common/dns-records.nix` (empty
  today) and `just check-eval` after adding it; do not move the address out of the pool by
  editing `common/vlans.nix`, which moves every device on the VLAN.

- **`voron.leighhack.org` (10.3.2.50) has no `websockets` on its row, and the sibling added
  beside it on 2026-11-23 proves what that costs** (`common/front-door.nix`). `pete-voron`
  shipped without it and a WebSocket to `/websocket` came back
  `400 Can "Upgrade" only to "WebSocket".` from the device's own aiohttp — a page that
  renders, a printer that never updates, and nothing in the door's log but a 400 — while
  `curl --http1.1` straight at the box answered `101` and Moonraker's notifications. Nothing
  has been measured against `voron` itself, so this is "untested, not absent": the two boxes
  are the same kind of appliance and one of them is a Mainsail. **Blocks:** a request to
  `voron` that opens a socket (Mainsail's browser console is enough); if it 400s, add
  `websockets = true` to that row, which is one line and a gw switch.

- **`just check-build` cannot pass at the current lock, and it is not this repo's build: the
  pinned `strata` input fails its own fetch (2026-11-03)**. `nixos-system-aibox` fails as
  `error: hash mismatch importing path '…-strata-unstable'; specified: sha256:04hc…,
  got: sha256:05rr…`, and `strata-unstable` is the upstream project's own path — nothing in
  this tree names it (`grep -rn strata-unstable --include=*.nix .` is empty), so the
  pinned rev (`5e8a4cca7d690ac60ca37eb09e7d0e010099ad6d`, `flake.lock`) no longer produces
  what its pin says. Measured on `nixos-system-aibox`; `services2` builds the same engine's
  CUDA flavour, so expect its toplevel the same way — **not re-measured here**, because
  building it is a 30 GB detour. What *is* measured matters more: `services1`'s and `gw`'s
  toplevel paths evaluated at the failing commit are byte-identical to the ones running, which
  is how to tell an input fault from a config fault in a hurry. Fixing it is a decision, not a command — `nix flake update strata` moves
  **both** engines (the `follows` rule at `flake.nix`'s `strata` input, `machines/aibox/STRATA.md`
  and `machines/services2/STRATA.md` for what a bump involves) — so it belongs with whoever is
  touching strata, not bolted onto an unrelated deploy. Until then `just check-build` is red for
  a reason no change here can fix, and `just check-eval` plus a
  targeted `nixos-rebuild dry-run --flake .#<machine>` are the gates that still mean something.

- **`nix flake check --no-build` on services1 fails at this lock over an eval-time derivation
  read (2026-11-02): `error: path 'w1qfxxfc0wwjzfqivfnqfqjf60dily0h-sops-secret-pg_pass.drv'
  is not valid`** — `services.outline.databaseUrl` embeds `config.sopsSecretText "pg_pass"`, so
  evaluating services1 needs that derivation realised, and `--no-build` declines to build it.
  Verified against clean HEAD (`git stash`) the same day: it reproduces identically with none of
  that day's edits present, so it is a baseline fact, not a finding of any change made that day —
  and the AGENTS.md note that the *grafana* `…not valid` error "did not reproduce at the
  2026-10-14 lock" is now stale in the same way the `docs/follow-ups.md` entry above says: at the
  current lock it is the `pg_pass` one that does. Same class as the crane stubs
  (`docs/nix-dev-workflow.md`, "Evaluation must not build anything"): the fix is the same — one
  build/deploy realises it and the flake step turns warning-free again — so treat
  `just check-eval` as the gate on services1 until then, and never read a red `just check` flake
  step there as "my change broke it" without re-running the same command against clean HEAD.

  **Turned green on services1 on 2026-11-03, exactly as predicted and for exactly the reason
  written above:** a `just check-build` run realised the `pg_pass` sops derivation into this
  box's store, and `nix flake check --no-build` now prints `all checks passed!` — so `just
  check` is green here end to end for the first time in a fortnight. **Nothing was fixed** to
  make that happen: the eval-time read is still there, the step is green because of *this
  store's contents*, and a box that has never built this configuration (or a store that has GC'd
  the stub) goes red again with the same message. So the rule stays and "treat `just check-eval`
  as the gate on services1" is now a statement about a *fresh* box rather than this one — read
  the step, do not inherit its state, which is the same lesson the 2026-10-14 grafana baseline
  note already carried.

- **The router's DNS is three generated files now, so `common/int-dns.nix`'s `legacy`
  set is a to-do list, not a design** (added 2026-10-22). `nginx.int`,
  `tailscale.int`, `gitlab.int` and `ldap.int` answer with the front door and are served
  by no enabled vhost; each carries its reason in the table. `gitlab.int` disappears by
  itself when the gitlab module is re-enabled (the guard moves it from `legacy` to
  `names` by refusing the other way). `mqtt.int` was checked on 2026-11-02 and **cannot** be
  deleted — three clients dial its socket — and it left `legacy` that same day into the
  table's new `answeredBy` group, deployed 2026-11-03, so it is no longer parked;
  `tailscale.int` still
  wants the same check. `nginx.int` stays — it is the CNAME
  target of every public `*.int` record in DigitalOcean. And `ldap.int` belongs with the
  authentik decision below, not with DNS.

- **`mqtt.int` answered with the front door while the broker's socket does not live at the
  front door — closed 2026-11-03** (found 2026-11-02; the three clients retargeted by hand
  that day; the table, the renderer, the guards and the deploy landed by 2026-11-03 01:09,
  the dials followed at 01:12–01:14). Home Assistant's MQTT config entry (`apps1`'s runtime
  `.storage/core.config_entries`), Frigate's publisher (`/srv/frigate/config/config.yaml`)
  and zigbee2mqtt's `server:` (`/srv/zigbee2mqtt/configuration.yaml`) all dialed
  `mqtt://mqtt.int.leighhack.org`, and the 2026-10-24 front-door move re-pointed that name at
  `gw`, where nothing listens on 1883 — so the MQTT link, and with it Home Assistant's Frigate
  *camera* entities (their `_available` flips only on a broker message), was dead for over a
  week while the HTTP sensors kept counting. All three were retargeted to `10.3.1.20` to stop
  the bleeding, which is the broker's address hardcoded three times on two boxes — exactly the
  drift `common/int-dns.nix` exists to prevent. The declarative option — a **new group** in
  that table answering the name with services1, because `legacy` answers with the door and
  `machines/gw/int-dns.nix` asserts `names + exempt` is exactly the door-served vhost names,
  so neither existing group fits — was **taken**, and the other one, moving the broker to `gw`,
  **rejected**: a podman container and its `/srv/mosquitto` state on the network's router,
  front door, TLS terminator, dnsmasq and UPS host, for clients that live on services1 and
  apps1. What landed: `answeredBy` in `common/int-dns.nix` (`mqtt.via = "services1"`), a third
  branch in `machines/gw/lib/network-tables.nix`'s `intHosts` rendering **one IPv4 line and no
  bare-name alias**, and three assertions in `machines/gw/lib/check-network-tables.nix` (an
  unknown `via`, a label in two groups or in `dns-records.nix`, a name that is not exactly one
  line carrying its box's address) plus that file's line total. Measured after the deploy:
  `10.3.1.20  mqtt.int.leighhack.org` in the router's generated file, `dig` answering
  `10.3.1.20` from services1 and apps1, a TCP probe through the name open, all three clients
  re-CONNECTED inside two minutes, and all eight `camera.*` entities `recording` in HA's state
  history. Receipts, the HA query, and the detour the deploy took through the router's
  diverged checkout: `docs/mqtt-int-legacy-answer-2026-11-02.md` ("Deployed") and
  `machines/gw/README.md`.

  **What is left in this class is one question, not a fault:** `tailscale.int` is still a
  `legacy` row answered at the door, and headscale's UI is a **WebSocket** on :8085 rather
  than a plain page. It works because headscale now runs *on* the door's box, so the socket
  and the answer are the same box — the same fault shape, not currently a fault. No guard
  would catch it moving: re-measure it the way this one was (the name's answer, a TCP probe
  through the name, and what actually dials it) if headscale ever moves again.


- **The `10.3.1.30:8123` forwarder is a crutch with a due date — its last client is gone, so the
  clock is now running (2026-11-16)**. Home Assistant moved to services1 and a `socat` unit on
  `apps1` kept the old address answering, so that whatever still dialled it kept working *and*
  logged itself (`just ha-legacy-watch`). What it actually found: `apps1`'s own containers and this
  box, **and no LAN device, phone, dashboard or script at all**. The one genuine client,
  `hackspace-api`, has itself been moved to services1 (`machines/services1/services/
  hackspace-api.nix`) the same afternoon, so since 16:30 the forwarder has reported no source and
  holds no established connection. **Blocks:** nothing — it is pure safety margin now. **To do:**
  on or after **2026-12-16**, if `just ha-legacy-watch 30` has read clean, remove it on
  `10.3.1.30`: `systemctl disable --now ha-legacy-proxy.service && rm /etc/systemd/system/ha-legacy-proxy.service`.
  **Re-verify with:** `just ha-legacy-watch 7` (expect no sources; the classes and what each means
  are in its own output) and `systemctl is-enabled ha-legacy-proxy.service` on `10.3.1.30`, which
  should answer `disabled` and eventually `Unit … could not be found`. Keep the shape of this,
  not just the outcome: the reason the client was **repointed rather than deleted** is that it is
  the hackspace's public SpaceAPI endpoint, and the door logged **7,939 external `/space.json`
  requests in 40 hours** from `spacer-fetch`, `MapsOfMaking-heartbeat`, `mapall.space`,
  `SpacePush`, `HackerspacesGlobe` and `HackerspaceNewsFeed`, plus 448 `/events.ics`
  subscriptions — measured in `docs/home-assistant-on-services1-2026-11-16.md`.

- **A plaintext admin password sits in `apps1`'s Prometheus scrape config, for an application that
  has been masked there for two weeks (2026-11-16)**. `/srv/prometheus/scrape_configs/uptimekuma.yml`
  defines the `uptime-kuma` job with `basic_auth` and a literal username and password, on a box
  where Uptime Kuma has been stopped, disabled and **masked** since 2026-11-01 and where
  `uptime-kuma.int.leighhack.org` no longer resolves (`docs/monster-proxmox.md`). So the
  credential authenticates against nothing that exists, is readable by any local user on that box,
  and is precisely the kind of string that outlives its application by being reused. Found while
  repointing the job beside it; not touched, because a credential rotation on an off-flake box is
  its own decision. **Blocks:** nothing. **To do:** confirm the name still does not resolve and
  the target is dead, then delete the file (or, if the password matters elsewhere on that box,
  rotate it and remove it from the file); the same sweep should read the other two files in that
  directory for `basic_auth`. **Re-verify with:** `grep -rl basic_auth
  /srv/prometheus/scrape_configs/` on `10.3.1.30` printing nothing, and
  `curl -s -m 5 https://uptime-kuma.int.leighhack.org/` failing on DNS rather than on auth.

- **`space_status` says `ago` twice on the printer line (2026-11-16)**. `just slack-agent-call
  space_status '{}'` renders `3D printers: 3D-1 offline (41m ago ago)`: `crates/hackspace-mcp/src/tools_space.rs:159`
  formats `({} ago)` around `clock::ago_text`, which already ends in "ago" (line 103 does it
  correctly). Cosmetic, but it is the string members see. **Blocks:** nothing. **To do:** drop the
  literal ` ago` at line 159 and run the suite. **Re-verify with:** `just slack-agent-call
  space_status '{}'` printing one `ago` per printer. (While reading that output, note both printers
  reporting `offline` is the known `10.3.14.x` reachability state, not a new fault.)

- ~~**Home Assistant on `apps1` runs whatever `:stable` meant today, and it moved on 2026-10-31**~~
  — **closed 2026-11-16 by the move to services1, where the image is a version, not a tag.** Its
  hand-written `home-assistant.service` pulled
  `ghcr.io/home-assistant/home-assistant:stable` in `ExecStartPre` and ran the same tag in
  `ExecStart`, so HA core went `2026.10.0` → `2026.10.4` on the box without anyone changing
  anything, and 2026.10.4 turned the Frigate integration's deprecated `via_device` into a fatal
  `RuntimeError` (12 failed camera-entity registrations on 2026-10-31; the HACS-installed 5.15.6
  fixes it but only on the next restart). The unit was off-flake (apps1, Proxmox VM 109), so
  pinning was a choice on the box rather than a flake edit — and it had drifted again in the
  fortnight since: `.HA_VERSION` read `2026.11.0` on 2026-11-16. HA is now
  `machines/services1/services/home-assistant.nix`, pinned to `2026.11.0` — the core it was
  already running, so the move itself carried no version change — which also makes the nightly
  `update-containers.timer` leave it alone, since that script only restarts a unit whose image
  digest moved. Upgrading HA is now an edit to `HA_IMAGE` plus a switch. Measured state, receipts
  and the diagnostic order: `docs/homeassistant-frigate-via-device-2026-11-02.md`; the move and
  the `10.3.1.30:8123` forwarder that is watching for whoever still dials the old address:
  `docs/home-assistant-on-services1-2026-11-16.md`.

- **The nightly container updater puts a Slack webhook in a world-readable store path
  (2026-11-16)**. `machines/services1/containers.nix` passes
  `webhookUrl = trim (readFile (config.sopsSecretText "slack_url"))`, and nixos-utils renders it
  as a literal `let WEBHOOK_URL = "https://hooks.slack.com/…"` inside
  `/nix/store/…-update-containers/bin/update-containers` — a nushell script any local user can
  read, on every box that substitutes it. This is exactly the cost "What build-time decryption
  costs, in one place" (AGENTS.md, Secrets) says `sopsSecretText` buys, and here it buys a Slack
  notification. **Blocks:** nothing. **To do:** have the script read the webhook from a
  `/run/secrets` path or from `EnvironmentFile` instead of embedding it, or move the notification
  behind a local unit that reads the secret itself. **Re-verify with:** `grep -c hooks.slack.com
  $(systemctl cat update-containers.service | sed -n 's|^ExecStart=\([^ ]*\).*|\1|p')` — a
  non-zero count while `webhookUrl` is set (no sudo needed to read the script, which *is* the
  finding), and no match once it reads the webhook from a runtime path instead of a literal.

- **`nginx.int`'s AAAA record is a ULA, and now points at `gw`**
  (`fd99:dead:beef:225d::1`; `nginx.leighhack.org`'s AAAA is the public
  `2001:8b0:1d14:225::1`). The public `*.int` CNAMEs hand any client outside the
  building one private IPv4 (`10.3.1.1`) and one private IPv6, which is coherent for a
  LAN zone and useful only to a tailnet client (whose subnet router is `gw`). If
  anything is expected to reach the internal apps from the open internet, this record
  should point at the public address or go away. Decide before pruning the `*.int`
  CNAMEs above.

- **Two public status vhosts have no DNS record** (found 2026-10-12 while
  sanity-checking after the `nixbld` fix).
  `machines/services1/services/status.nix` defines the SSO vhosts
  `status.leighhack.org` and `aibox.status.leighhack.org`, and
  `common/monitoring-endpoints.nix` monitors `https://status.leighhack.org`, but
  both names are **NXDOMAIN** in the DigitalOcean zone — so they are unreachable and
  that gatus check cannot be passing. The `*.int` dashboards are fine
  (`services1-status.int` / `services2-status.int` / `aibox-status.int` /
  `network-info.int` all serve 200). dns-sync only manages the `*.int` CNAMEs;
  public names are added by hand in DO DNS and these two never were. Either add the
  records (public apps CNAME → `nginx.leighhack.org`) or drop the vhosts and the
  endpoint.

  **Closed 2026-11-16 by dropping them.** Nothing was reachable at either name, and
  the dashboards have always been used through their `*.int` twins, so the deletion
  was chosen over minting records — and `aibox.status` would have needed a
  `*.status.leighhack.org` SAN as well, which the wildcard does not cover. Gone: both
  rows in `common/front-door.nix`, the `status.leighhack.org` origin vhost, and the
  gatus endpoint, which is now the LAN twin `https://services1-status.int.leighhack.org`.
  What stays
  is the lesson, in `docs/network-upgrade-runbook.md`: a public vhost with no record
  passes every gate this repo has, so the runbook's baseline probe of
  `common/monitoring-endpoints.nix` is what finds them, not `just check`.

- **Three gatus checks point at names that have never existed** (found 2026-10-15
  while taking the upgrade's endpoint baseline). `common/monitoring-endpoints.nix`
  monitors `http://3d-1.int.leighhack.org`, `3d-2` and `3d-3`; `getent` says
  NXDOMAIN and `common/dns-records.nix` has no such devices — the printers are
  `3d-blue` (10.3.14.62) and `3d-lime` (10.3.14.61), and there is no third. So three
  of the network's ~30 checks have been red since they were written, which is worse
  than no check: it trains the reader to ignore the red. Either the names are the
  old ones and the file is stale, or a third printer was planned and never bought.
  Fix the endpoints to the two real names (and say why a check is allowed to be red
  if it ever is again).

- **Nothing monitors the borg server** (`nas2.int:3022`), so a dead `borg-server`
  container only shows up as a failed nightly job — and one nightly run has already
  failed with no recorded reason. Details: `docs/nas-and-borg.md`.

- **Nothing measures services1 from outside itself** (opened 2026-10-17 by the move
  that put gatus, Prometheus and Grafana on services1). Every check that used to
  watch services1 — gatus's ping of 10.3.1.20, and the https checks that go through
  its nginx — now runs on the box they watch, so a dead services1 goes down with its
  own monitor and reports nothing, and the dashboards are unreachable at the same
  moment they stop being updated.  The other boxes are still measured from here
  (gatus pings services2/aibox/router/NAS/switches, Prometheus scrapes their
  exporters), so the gap is one-sided.  The fix is a second, minimal monitor on a
  box that is not services1 — gatus is a small Go binary and aibox has the
  headroom — or a cron/one-shot on aibox that curls services1 and alerts to Slack.
  Do not "fix" it by keeping the services1 self-ping in
  `common/monitoring-endpoints.nix`: a check on the box that runs it can only ever
  catch a hung nginx, and it was removed on 2026-10-17 for that reason.

## Router (gw)

- **The AAISP uplink was dead on 2026-11-07, and the network ran on a hotspot; the
  fallback is now declarative, the primary is still AAISP's to fix** (opened
  2026-11-07). Measured on `gw`: `ping 192.168.20.1` answered while
  `ping -I router-uplink 1.1.1.1` did not, `sudo ip route show table 100` was
  empty (the tunnel cannot dial *through* a hub with no upstream), and
  `journalctl -u xl2tpd-aaisp` was a redial loop. The hand-built lifeline
  (`/run/wpa-backup.conf`, a `/run` `.network` file, typed routes, an `nft`
  `masquerade` added by hand, two `ip rule`s at a table 300) had the LAN's IPv4
  back inside an hour — and was one reboot or one `switch` away from ending,
  because `/run` does not survive the first and nftables' reload deletes the
  second. It is now `machines/gw/wan-backup.nix`: the association, the DHCP
  address (the hotspot moves between `/24`s — `10.112.100.0/24` at 13:48 to
  `10.120.243.0/24` at 14:53 the same day), a 10 s reconciler that promotes the
  wwan's default only when the tunnel *and* the hub probe are both dead, and the
  `masquerade` rows in `./firewall.nix`. The passphrase is
  `wan_backup_wifi_psk` in sops. **`docs/wan-manager-2026-11-09.md`** owns the policy,
  the takeover steps, the verification and the limits.

  **Blocks:** nothing in the flake — the 2026-11-07 rollout and its per-VLAN
  checks passed. What is open is AAISP's: the hub still has no upstream and the
  tunnel still will not dial, so the network is on a metered hotspot and has
  **no IPv6 and no inbound public services** (the `/29` is routed to the AAISP
  line) until that is fixed. When it is, the reconciler demotes the wwan within
  10 s and nothing has to be deployed; the module can stay as a standing
  fallback, or be retired by unplugging the stick (safe, immediate) or by
  removing the import, the sops key and the `wan-backup.nix` module
  in one change (`docs/wan-manager-2026-11-09.md`, "Retiring it"). **Do not** remove the
  sops key while the module still reads it — that fails activation, on the
  router. Two decisions this opens and does not take: whether the fallback should
  stay semi-permanent once the primary is healthy, and whether to rename
  `wlp0s29u1u2` to `router-wwan` (a NAT-keyed rename, so it needs both names in
  the masquerade rule for one deploy; `./constants.nix` says why it was not done
  while the network was *on* the link).

- **The IPv6 half of the reservation table has no mechanism behind it** (found
  2026-10-26 debugging "the PXE clients stopped booting", and **the twelve records
  that were provably unreachable are now emptied** — see `docs/router-and-dns.md`,
  "And the IPv6 half, which is the half that was lost", for the mechanism and
  `common/dns-records.nix`'s `ipv6` field for the rule).

  What the old `config.xml` settled, against what this file first guessed: the
  appliance never ran a DHCPv6 *server* in the sense of a pool — `<dhcpdv6>` has no
  `range6` — it held fourteen per-host static bindings, and it NPT66'd
  `fd99:dead:beef:225d::/64` → `2001:8b0:1d14:225d::/64` and the same for `227d`. So
  the ten PI-ROOM `2001:8b0:1d14:227d::10NN` were never host addresses at all: they
  were created by translation on the way out of `opt3`, and with no NPT66 on `gw`
  they are unreachable in both directions (`ip -6 route get` on the router sends them
  out `dev ppp0`). Emptied, with the reason at each record; `apps1` (`::30`) and
  `yunohost` (`::15`) were in a real prefix but unowned — both boxes answer on IPv4
  while `ip -6 neigh show` here says `INCOMPLETE` — so those two went too, and re-adding
  either is a decision to configure the device, not to edit this table.

  **What is left, and it is deliberately small.** (1) `machines/gw/lib/check-network-
  tables.nix` now refuses a record whose IPv6 is outside every /64 the network
  advertises; the residual class — in-prefix, unowned — is invisible at eval time and
  can only be measured (`ip -6 neigh show`, `INCOMPLETE`), which is why the table's own
  comment carries the rule rather than a fifth assertion pretending to know. (2) While
  writing that assertion: four of this file's failure messages interpolated `r.name`
  over `builtins.attrValues RECORDS`, where `name` is the attrset *key* and not a field,
  so those messages crashed with `attribute 'name' missing` instead of naming the record
  — and two then crashed again on `${r.vlan}` coercing an integer to a string. Fixed
  (verified by making a record misbehave on purpose, once per class); any *new* message
  in that file has the same trap available, since nothing but a failing assertion
  exercises one. (3) Two records keep IPv6 because those boxes configure it:
  `services1` `::20` and `aibox` `::32`. If anything else in the network ever wants
  named IPv6, the choice is a static address on the device or an actual DHCPv6 server
  with a `range6` on `gw` — the old box had neither of those, which is the reason this
  is a decision and not a restoration.

- **The connection table's "ours" is asserted for IPv4 only** (found 2026-10-25 while
  adding `TABLES.localPrefixes`, the `--local-prefix` list `../machines/gw/network-status.nix`
  derives from `common/vlans.nix`). `machines/gw/lib/check-network-tables.nix` now checks
  every derived prefix against the network's own ACL — the same containment relation the
  firewall and the `coveredByAcl` assertion use — and that relation is `blockPrefix`,
  which counts dotted quads: handed `fd7a:115c:a1e0::/48` it dies in `builtins.elemAt`.
  That is why the helper filters to `TABLES.ipv4AclBlocks` and the IPv6 /64s a VLAN
  advertises go unchecked, so a `common/vlans.nix` edit could put a prefix the ACL does
  not allow on the page's "internal" side. (Before the filter, the failure mode was worse
  than silence: a genuinely uncovered prefix walked the ACL list to its IPv6 entries and
  crashed the whole evaluation instead of naming the block.) **Blocks:** nothing today —
  every IPv6 prefix in the derived list is inside one of the ACL's four /48s. **Needs:**
  an IPv6 containment helper in `machines/gw/lib/network-tables.nix` (the mask is a group
  count on this network, so 16-bit groups and a `mask mod 16 == 0` refusal are enough and
  no bignum arithmetic is wanted), then one line adding it to the same assertion block.
- **The leases file is still root-only, so a DHCP client is a number on the page**
  — **closed 2026-10-26, the decision is `machines/gw/lease-reader.nix`.** It was opened
  2026-10-25 with the connection table: `/etc/dnsmasq-records.hosts` is world-readable
  and gives every *static* record a name, while `/var/lib/dnsmasq/dnsmasq.leases` is
  `0640 root:root` and holds the names of everything on the dynamic ranges — most
  phones, laptops and the VLAN 226 cameras' own clients. **The answer was yes:** the file
  goes to the group the dashboard's account already has (`conntrack-readers`, one
  member, no login, `ProtectSystem=strict`), stays 0640, and is not readable by anything
  else. What that group can now see is a device's hostname, MAC and client-id alongside
  the address — which is the same audience the connection table already shows talking to
  whom, over the front door's LAN ACL. The three places the group is re-applied, and why
  a watch on `/var/lib/dnsmasq` rather than a timer, are in that module's header;
  `/api/leases` is the endpoint and `docs/services.md` has its contract. **Blocks:**
  nothing.
- **`ExecStartPre=+` and not a timer is a decision, and it is open to being wrong**
  (2026-10-25, `machines/gw/conntrack-reader.nix`). The group on
  `/proc/net/nf_conntrack` is re-applied when network-status starts and not otherwise,
  so reloading `nf_conntrack` mid-run leaves the dashboard blind until it restarts — it
  logs that state rather than showing an empty router. The first version of the module
  had a oneshot plus a timer to re-apply it, like `common/nixbld.nix`; the timer went
  because a oneshot that fails to start fails `nixos-rebuild switch` on the network's
  router, and the box had already eaten one deploy that way. **Blocks:** nothing; NAT
  goes with that module, so a mid-run reload is not a thing that happens quietly.
  **Needs:** if it ever does, the timer comes back with
  `jobConfig.JobTimeoutStartSec` and `serviceConfig.SuccessExitStatus=` argued out loud,
  not copied from `nixbld-guard`.

- **A flow that moves more than 4 GiB resets its own age** (found and pinned by test on
  2026-10-25, `crates/network-status/src/flows/tests.rs::a_counter_that_goes_backwards_rebaselines_instead_of_going_negative`).
  The kernel's per-connection byte counters are 32-bit, and the tracker cannot tell a
  wrap from a tuple handed to a new connection — both are the same tuple reporting a
  smaller number — so it starts the flow over: `samples` back to 1, `first_seen` to now,
  no rate for that sample. On a gigabit link that is one 5-second disappearance every
  ~35 s of a very large download, and its byte total reads 4 GiB light afterwards.
  The rates stay right and nothing goes negative, which is why this was documented
  rather than papered over. **Blocks:** nothing; it is a wart on transfers over 4 GiB.
  **Needs:** either accumulate `u64` deltas and accept that a reused tuple inherits its
  predecessor's bytes, or read the counters over `netlink` with `CTA_TIMESTAMP` (which
  also fixes the "age is age-as-we-saw-it" caveat for good), and say which in the module
  header of `crates/network-status/src/flows.rs` either way.

- **`firewall.int.leighhack.org` proxies a port nothing listens on** (found 2026-10-24).
  Its row in `common/front-door.nix` sends it to `https://10.3.1.1:60443`;
  `ss -lntp` on gw answers 53, 22, 8091 and tailscaled, and `60443` appears in the
  whole tree exactly once — as that `proxyPass`. The retired appliance's admin UI was
  deliberately never reopened (`machines/gw/README.md`, and `firewall.nix`'s ssh-rule
  comment), so the vhost is a 502 and the row in `common/int-dns.nix` is a name the
  front door answers for an app that does not exist. Either something is meant to be
  listening on 60443 — in which case say what, and on which interface, because "the
  router's admin panel" is exactly what its firewall audit concluded it should not
  have — or the row and the `firewall` label go.

- ~~**`just switch-gw`'s `sync-gw` rsyncs the router's `.git` as well, and `--delete`
  means it.**~~ **Closed 2026-10-25 by deleting the recipes.** Seen on the 2026-10-24 deploy:
  29 deletions, every one of them gw-only git metadata — a local branch and the records of
  three stale worktrees. The excludes were `target/`, `node_modules/`, `/result*` and
  `/.git/config`, so the rest of `.git` was reconciled against services1's copy: **a commit
  or branch made in gw's checkout was not merely unsynced, it was on a collision course**,
  and the router's repo could be left referencing objects a later sync deleted. Nobody hit
  the corruption; the branch it ate was the audit's and the commits were safe on services1,
  which is why it was a paragraph and not an incident. Rather than add `--exclude /.git` and
  keep a half-mirror, `sync-gw` and the push-based `switch-gw` are gone: every box deploys
  itself (`just deploy <machine>` — ssh in, `git fetch` the commit, `switch` and
  `nixos-confirm` there), so gw's checkout is a normal one and the local-network transfer is
  a git fetch rather than an rsync of a working tree. `justfile`, `machines/gw/README.md`
  ("How this box is deployed"), `AGENTS.md` rule 22 and `docs/nix-dev-workflow.md` say so.

- **Public IPv6 inbound still owes a probe from outside the ACL** (opened 2026-10-24,
  after the front door moved). `nginx.leighhack.org`'s AAAA now points at the router
  (`2001:8b0:1d14:225::1`) and the router answers public v6 itself; the DigitalOcean
  record was changed on 2026-10-24. The old services1-forward rule is gone, so
  `nft list chain ip6 router6 input` is the rule to read.

  A LAN `curl -6` proves nothing (its source matches `2001:8b0:1d14::/48`), and the
  pre-cutover behaviour was inferred from pf rules rather than observed. Probe from a
  v6 address that is not in the ACL — a phone on mobile data — and read the counters.
  One deliberate non-change: 3478 stays v4-only (headscale's DERP has a published IPv6
  with a v4-only STUN socket); 50443 and 51820 were removed on 2026-10-21; and
  `yunohost.leighhack.org` has no AAAA so the appliance has no v6 row.

- **The tailnet's routes are applied but still boot-fragile** (measured 2026-10-21,
  re-applied 2026-10-24). `machines/gw/uplink.nix` advertises
  `10.3.0.0/16,192.168.2.0/24,2001:8b0:1d14::/48,fd99:dead:beef::/48` and
  `--advertise-exit-node`; after services1's tailscaled was removed with the headscale
  move, gw is the only subnet router, and the routes were set by hand and approved on
  the live node (`headscale nodes approve-routes`). `extraUpFlags` still reach the
  daemon only through `tailscaled-autoconnect.service`, which runs **at boot**, and a
  hand-typed `tailscale up` outranks the flake — check
  `sudo tailscale debug prefs | jq .AdvertiseRoutes` and the approved list in
  `headscale nodes list` after any reinstall or reboot.

- **`100.64.0.0/16` was the network's idea of the tailnet, and headscale hands out a
  /10 — fixed 2026-10-21.** `common/config.nix` now carries `TAILNET_V4`/
  `TAILNET_V6` and the four consumers read them: the nginx `LOCAL_NETWORK` ACL, the
  Postgres `pg_hba` table, headscale's own `ip_prefixes`, and every tailnet rule in
  `machines/gw/firewall.nix` (the forward ACL, the policy-routing "is this destination
  ours?" set, and the exit-node masquerade). The old box's `TAILSCALE` alias was
  `100.64.0.0/10, fd7a:115c:a1e0::/48`, so this is a correction rather than a
  widening of trust — but note that it *does* widen, in the same change, the nginx
  internal-vhost ACL and `pg_hba`: any address in `100.64.0.0/10` is now trusted by
  them, which is only safe because the tailnet's allocation pool is that block and
  nothing else can route it here. Two leftovers: the ACL's other IPv6 lines are still
  spelled with a trailing zero (`allow fd99:dead:beef::0/48`) where the two
  interpolated lines now render `::/48` — identical to every parser involved, and
  normalising the rest is a pass over the file rather than part of this change — and
  **deployed to gw and services1 the same evening**: the /10 is live in the rendered nginx
  config and in the file `show hba_file` names, and gw's v4 forward set names
  100.64.0.0/10. services2 and aibox pick the ACL up at their own next switch, and the
  tailnet prefs still want the boot described above.

- **The retired box's `config.xml` is a live secret file with five credentials nobody
  has rotated** (found while auditing it 2026-10-21; it is gitignored, and it is a
  slightly older duplicate of `~/tmp/opnsense-archive-20261017/config.xml` on
  services1, so the working-tree copy had no reason to exist — and it is already gone:
  cleaning gw's checkout at 20:21 on 2026-10-21 deleted it, along with an agent's
  untracked scratch file, which is how it was noticed. The archive is now the only copy.
  What is in it, in
  words rather than values: the AAISP L2TP password (already copied to
  `aaisp_ppp_password` in sops), the authentik **LDAP bind password** for
  `cn=pfsense-gw` (used by the old GUI login and by the VPN's `Members` group auth),
  a **headscale pre-auth key**, the **ssh private key of the `backups@nas2` account**
  that its nightly config backup used, an OpenVPN static TLS key, two CA/cert private
  keys including "Leigh Hackspace CA1", and local password hashes. Several of these
  reached a session transcript on 2026-10-21 while being audited, which makes
  rotation the action rather than discussion. Order of cheapness: delete the NAS's
  `backups` key (that backup job is gone with the box), expire the pre-auth key,
  rotate the LDAP bind password in authentik (and only then decide about the VPN),
  and treat CA1 as compromised-if-anyone-ever-used-it. **Do not delete the archive
  copy on services1 first** — it is the only place the CA and server certificates
  exist.

- **What the old box had that this box deliberately does not have**, so that nobody
  re-audits the XML for it: IDS/Suricata, Netflow, monit, Unbound, Kea, CARP/pf-sync,
  captive portal, WireGuard, IPsec/swanctl and cron were all **disabled** there
  (nothing to port); its TrafficShaper pipes and its `snatrules` were empty or off;
  there was **no gateway failover** either (every `route-to` in `pf.txt` is
  single-target, so `firewall.nix`'s static marks are parity, not a regression);
  **the CARP/pf-sync half was re-read on 2026-10-27 and is stronger than "disabled"**:
  the HA page was never filled in at all — two virtual IPs, both `ipalias` with an empty
  `vhid`, and `<hasync>` holding nothing but `pfsyncversion=1400` (a default that looks
  like a decision), `disconnectppps=0` and empty `pfsyncinterface`/`pfsyncpeerip`/
  `synchronizetoip`/`username`/`syncitems`. So there is no redundancy to have regressed
  from, and the answer to "OPNsense does it with CARP" for this network is "it did not".
  What a CARP-style pair would take on Linux, and the four things it would not fix, are
  `docs/router-ha.md`; the one item to carry out of that file is that **the AAISP tunnel's
  failover is unanswered and gates everything else** (can two L2TP sessions share a line,
  and does the routed `/29` and the `/48` follow the session that dials). the
  floating `pass in … ANY` rule described as `EVERYTHING` and the `DELETEME` rule were
  **not** inherited and nobody has reported anything missing since; and its `bogons`
  table, `pf_share_forward`, `lb_use_sticky` and NAT-reflection *helper* are pf/FreeBSD
  concepts with no nftables equivalent needed here. The three real casualties are the
  **Members OpenVPN server** (see `machines/gw/README.md`'s table), the **nightly
  config backup to the NAS**, and the **RRD/system-health graphs** — this box is
  gatus-pinged (`common/monitoring-endpoints.nix`) but exports no metrics of its own,
  so a ppp0 flap or a 100 Mbit LAN negotiation is visible only as an outage.

- **`CONFIG.LOCAL_DOMAIN` exists; most of the network does not use it yet** (added
  2026-10-17). The router needed the zone name in one place because three uses of it
  have to agree exactly (`domain=`, `local=`, and the suffix on every generated
  record — see `machines/gw/dns.nix`), and the failure of them disagreeing is a silent
  empty answer for the whole zone. `common/nginx/`, `common/monitoring-endpoints.nix`
  and the machines' `networking.search` still spell `int.leighhack.org` out. Converting
  them is mechanical and worth a pass of its own, not a ride-along with a router
  change.

- **dns-sync is deleted, and `*.int` is declared instead of synchronised** (2026-10-22).
  What it did — ssh from services1, read `/var/lib/dnsmasq/names.hosts` on the router,
  append the missing (address, name) pairs, `systemctl reload dnsmasq`, and separately
  create a DigitalOcean CNAME — is now a table: `common/int-dns.nix` lists every `*.int`
  name that answers with the front door (now `gw`)'s three addresses, `machines/gw/dns.nix` renders it to
  `/etc/dnsmasq-int-vhosts.hosts`, and that store path is a dnsmasq `restartTriggers`
  entry, so record and restart arrive in one activation. The gate that makes "forgot the
  DNS" impossible to land is `machines/gw/int-dns.nix`: it derives the `*.int` names from
  the front door's `services.nginx.virtualHosts` and refuses to evaluate `gw` unless that
  set equals the table (the origin box's half is `machines/services1/int-dns.nix`, which
  checks the table's `origin` rows instead). The crate, its workspace membership, `machines/services1/dns-sync.nix`
  (including the pinned router host keys, which nothing reads any more), the
  `just dns-sync-check|sync|prune` recipes and every doc pointer are gone; the facts worth
  keeping from the tool are in `docs/router-and-dns.md` ("What `*.int` answers with"). Two
  things follow from the deletion and are still open:

  1. **Delete `/var/lib/dnsmasq/names.hosts`, and its tmpfiles rule, in a later deploy**
     (added 2026-10-22). The file is no longer in dnsmasq's `addn-hosts` list, so nothing
     reads it — but a generation built before this one *does* name it, dnsmasq **exits**
     on a missing `addn-hosts` file, and `system.autoRollback` can put that generation
     back for five minutes after a switch. Removing the file in the same deploy as the
     change that made it dead therefore turns a rollback into a router with no DNS for the
     network. The two halves must go together, in a deploy after the one that is confirmed
     good: the rule in `machines/gw/dns.nix` and the file itself
     (`sudo rm /var/lib/dnsmasq/names.hosts`). Its content is fully reproducible from the
     tables, and the 2026-10-21 snapshot is still in `/var/lib/dns-sync-backups/`.
  2. **DigitalOcean's `*.int` half is now maintained by nobody** (added 2026-10-22).
     ~40 `*.int` CNAMEs → `nginx.int.leighhack.org` remain in the zone; nothing reads
     them for the LAN (the router answers) or for the tailnet (headscale's split DNS sends
     `int.leighhack.org` at 10.3.1.1), so they are for public-DoH clients only. Either
     prune them and add the one record `*.int` CNAME → `nginx.int` — **test that against a
     real ACME renewal first**, because `*.int.leighhack.org` is in the wildcard cert and
     its `_acme-challenge` lives in the same zone — or leave them and accept they can
     drift. While in the zone: DO stores record *labels*, and three records were created
     with FQDNs and therefore answer nothing. **`gatus.int` is fixed (2026-10-22)** — the
     CNAME `gatus.int` → `nginx.int` exists (ttl 3600, verified against `@1.1.1.1`) and
     the doubled record is deleted. Two remain: `AAAA yunohost.leighhack.org` and the
     `_cs-verify` TXT named `leighhack.org`; both want whoever owns yunohost and the
     cert-status verification to say which name they meant before anything is deleted.
     The API gotcha worth remembering: `data` must end in `.` on **create** and comes
     back without it, so the value you read is not the value you post
     (`docs/router-and-dns.md`).

  `network-status` was the next tool down the old "ssh into the router" list and is
  ported and moved (2026-10-21): it runs on gw as a `DynamicUser`, reading
  `/proc`/`/sys` locally instead of OPNsense's commands (`docs/services.md`,
  `machines/gw/network-status.nix`). Its `--router` ssh mode is still there for
  emergencies, its `--known-hosts` default points at a path nothing ships, and
  `just router-known-hosts` prints the keys to pass it.

- **Two dnsmasq bugs found by running the shadow box beside the live one**
  (2026-10-17, both fixed). Neither was visible in the config, in the build, or in
  `systemctl is-system-running`; both were visible only by asking the two routers the
  same question:

  1. **The generated records file had bare names, so the zone did not resolve.**
     `dig @10.3.1.254 services1` answered and
     `dig @10.3.1.254 services1.int.leighhack.org` answered *nothing*, while the old
     box answered the FQDN. dnsmasq's `local=` makes the box authoritative for a zone
     and does not complete a hosts entry, so every vhost, gatus endpoint and
     dns-sync name on the network would have gone NXDOMAIN at the cutover. The old
     box's `/var/etc/dnsmasq-hosts` spells both names on every line
     (`10.3.1.20 services1.int.leighhack.org services1`) and one line per address;
     `TABLES.recordsHosts` now generates that shape, and
     `machines/gw/lib/check-network-tables.nix` refuses a file whose FQDNs are
     missing or whose lines carry more than one address (dnsmasq reads the first
     field as the address and the rest as names, so an IPv4 and its IPv6 on one line
     registers the v6 address as an alias *name*).
  2. **A records change never reached the running dnsmasq.** nixpkgs' module
     restarts it only when `/etc/hosts` changes, and dnsmasq reads `addn-hosts` at
     startup, so a switch that moved a device's address wrote the file, exited 0 and
     kept answering the old address. `dns.nix` now puts the generated file in
     `restartTriggers`; verified by moving a record's address, switching, and seeing
     dnsmasq restart and answer the new value (and then moving it back).

  The habit worth keeping from this: **compare the two routers rather than reading
  the config** — `for n in <names>; do dig @10.3.1.1 $n; dig @10.3.1.254 $n; done`
  is a few seconds and finds the class of bug where the new box is self-consistent
  and wrong.

- **The six `iptables-nft` tables on the router are tailscaled's, not a second
  firewall** (checked 2026-10-17). `nft list tables` on `gw` shows `filter`/`nat`/
  `mangle` for both families alongside this flake's `router`/`routerIn`/`routerNat`/
  `router6`, and `nft` warns `table ip filter is managed by iptables-nft, do not
  touch!`. They are `ts-input`/`ts-forward`/`ts-postrouting`, inserted by
  `services.tailscale`; `ip filter FORWARD` has policy `accept` and only jumps to
  `ts-forward`, and there is no `inet firewall` table (NixOS's host firewall is not
  adding a second set of decisions). So there is nothing to clean up — but at the
  cutover, when forwarding starts working or not, know which chains are whose before
  blaming `firewall.nix`.

- **`gw` is the network's router as of 2026-10-21** (cut over from co-existence
  mode, `./coexist.nix`'s `enable = false`). The box is a Supermicro X9SCL/X9SCM that
  used to run Windows; `machines/gw/README.md` has the install, the layout and the
  post-cutover findings (missing
  `*.int` records, no inbound NAT, dnsmasq's RA timer stopping after a LAN link flap)
  and the last four open items below. The `10.3.1.254` co-existence address is history.

- **dnsmasq stops sending *unsolicited* RAs after a LAN link flap** (found
  2026-10-21, on the cut-over box after the trunk was re-cabled; the trap and its
  checks are in `machines/gw/README.md`, "The third way dnsmasq breaks the LAN
  silently"). Solicited RAs keep working, so a probe hides it; clients lose their IPv6
  default when the 1200 s router lifetime expires, and a host that forwards (services1)
  never takes the RA default at all and so cannot reply to VLANs 226/227. The fix is a
  `systemctl restart dnsmasq` after the flap — there is no config change that restarts
  the timer.

  **Recurred 2026-11-21** off the 2026-11-19 14:17 `router-lan` flap, and it took the
  **wired** LAN's off-link IPv6 (services1 *and* services2) for two days with no failing
  unit, no warning and no complaint until somebody tried to ping a host outside the /64 —
  every in-network `*.int` name is answered on-link, so the missing default route is
  invisible from inside. The measurements, the two meters that work (`nstat
  Ip6OutMcastPkts` on gw, `proto ra ... expires` rising on a client) and the one that
  does not (tcpdump, which never sees this box's RAs even when they arrive) are in the
  README section. **Open**, unchanged and now twice-happened: a link-flap hook (or
  radvd, which re-advertises on link-up) so this cannot be missed by whoever is not
  watching at the time — and a `ra-watch.timer` on gw, in the same shape as
  `lease-reader-watch.timer` and `front-door-cert-check.service`, failing by name while
  `Ip6OutMcastPkts` sits still for longer than `ra-param`'s interval. A flap hook alone
  would not have helped here: the flap was real, the carrier came back, and nothing on
  this box noticed either.

- **`yunohost.leighhack.org`'s certificate expired 2026-10-21 16:59 UTC** (found the
  same evening, from the browser warning). The DNAT now sends the name to the right
  place — `10.3.1.15`, ports 80/443 open, `openssl s_client` reports
  `CN=yunohost.leighhack.org` issued by Let's Encrypt — but `notAfter` had passed, and
  the renewing HTTP-01 challenge can only reach the appliance on `:80` now that the
  cutover's inbound NAT exists. Renewal is a Yunohost-appliance action
  (`yunohost domain cert renew`, or the admin UI) and **the appliance is out-of-band**
  (the machine-hop key is not authorized on `10.3.1.15`), so it needs someone with its
  credentials. Do not "fix" it by adding a services1 vhost: Yunohost terminates its
  own TLS and generates absolute `https://` URLs.

- **`aaisp_ppp_chat_secrets` does not exist** — **done, under another name.** The
  tunnel's password is in `secrets/secrets.yaml` as **`aaisp_ppp_password`** and
  `machines/gw/sops.nix` renders it to the pap/chap secrets files pppd reads, which is
  what let the tunnel dial at the 2026-10-21 cutover. What the original note got right
  is kept: sops-nix fails *activation* rather than evaluation on a missing key, so
  `machines/gw/sops.nix` still declares the secret only when the tunnel is enabled.
  The read itself came out of the old box's `/conf/config.xml`; see that file's own
  item above about the credentials that came with it.

- **Nothing obtains the network's IPv6 prefix** (2026-10-14; **half-fixed
  2026-10-21, still unproven**). The config *does* ask now — `machines/gw/networking.nix`
  requests `::/48` with the VLAN id as `SubnetId`, `firewall.nix` admits the DHCPv6
  exchange on the tunnel, and `common/vlans.nix` carries a static address in every
  advertised /64 so a client is never given a prefix whose gateway does not exist (the
  worst of the two failure modes, spelled out in that table). What is still true is
  that **delegation has never been observed from this box**: `ppp0` holds only a
  link-local address and the /64s in use are the hardcoded ones, which work because
  AAISP routes `2001:8b0:1d14::/48` to the line rather than because anything was
  learned. The exposure is the day the /48 changes.

- **The new firewall's rules are shape-correct, not value-verified**
  (`machines/gw/firewall.nix`, 2026-10-14; **the read has now happened, 2026-10-21**).
  nixpkgs validates the ruleset against a user-mode kernel at build time — which caught
  two real mistakes (ICMPv6's ND types are `nd-neighbor-solicit`, not
  `neighbour-solicitation`; `packet-too-big` is ICMPv6 only) — but "parses" is not "is
  what the old box did". The old box ran **pf**, so `/var/etc/nftables.conf` was empty
  and reading it would have proved nothing; what to read was `pfctl -sr`/`-sn`, archived
  as `~/tmp/opnsense-archive-20261017/pf.txt` on services1, and the 2026-10-21 pass over
  it against the live `nft list ruleset` found exactly one missing behaviour (public
  IPv6 inbound, above) and confirmed the rest: the eight v4 forwards, the NAT-reflection
  hairpin, `41641` for the tailnet, the per-VLAN `route-to` split, `192.168.49.0/24 via
  10.3.1.20`, and that there was no gateway failover to inherit. **Still open in this
  item**: the per-VLAN anti-spoof rule emits *two* match forms (`vlan id N` and
  `iifname "vlanN"`) because which one the kernel matches on is a question about the
  real box — the box is now live and forwarding, so read the counters and delete the
  half that never matches.

- **A stale headscale node called `gw` still owns the name `gw`** (noticed 2026-10-17;
  confirmed at the cutover). `headscale nodes list` has an old `gw` at 100.64.0.11
  (offline since 2026-04-13) and the live box registered on 2026-10-21 as **`gw-1`**
  (id 56, 100.64.0.28). Headscale's node names are what ACLs and humans read, so two
  nodes called `gw` is still a finding. Expire id 15 and rename id 56. Its routes are
  now advertised and approved (2026-10-24); read
  `sudo tailscale debug prefs | jq .AdvertiseRoutes` and the approved list, not
  `status`'s `AllowedIPs`, which hides an unapproved advertisement.

- **The LAN port is doing 100 Mbit, not gigabit** (measured on the box,
  2026-10-17: `eno1` at 100Mb/s, full duplex). Either the cable, the SFP/PHY setting,
  or the switch port it lands on. It does not matter while the box is a shadow and it
  matters enormously when it is the router, because every byte to and from the
  network passes through that one interface. Check the switch port's own config before
  blaming the NIC (`crates/switch-gui/` reads the ProCurve's port state).

- **`mitigations=off` is deliberately NOT on this box's kernel command line**
  (`machines/gw/hardware-configuration.nix`, 2026-10-17). The other machines carry it
  on the "trusted LAN, single operator" argument in `common/base.nix`; a router is the
  one box where that argument is weaker (it is the box with an interface towards the
  internet) and the decision belongs to whoever cuts it over, not to whoever installed
  the OS. Say so in the README when it is decided either way.

- **Which physical port is the LAN is an assumption**
  (`machines/gw/hardware-configuration.nix`). The two `links` rules pin `router-lan`
  and `router-uplink` to the two e1000e MACs, and the assignment came from "this one
  is carrying the network today / that one has no carrier", which is strong evidence
  and not proof. Re-check at the cutover, when the AAISP handoff cable actually moves:
  `ip -br link` and which port answers.

- **The post-cutover exposure audit, 2026-10-21: the port list is now right, the
  source list is not.**  The internet-wide forwards are 80/443 on both public
  addresses plus 3478/udp (headscale's DERP STUN) — 8522, 50443 and 51820 were
  removed, and the v6 rule is 80/443.  Re-read the live rules with
  `sudo nft list chain ip routerNat dstnat`, `ip6 router6 forward` and
  `ip routerIn input`.  Two things still mean "only 80/443 + tailscale" is not the
  whole truth for *specific sources*:
  1. `common/config.nix`'s `LOCAL_NETWORK` contains six public ranges
     (`51.148.168.145/32`, `217.155.231.1/32`, `2a02:8010:6680::/48`,
     `2a02:8011:d000::/48`, `2a0a:ef40:154a::/48`, `2001:4860:7::/48`) and both the SSH
     input rule and the
     forward chain trust the whole ACL, so those addresses get **port 22 and any
     forwarded destination**.  Moving them to the tailnet (or splitting the ACL into
     service clients vs routable admins) is the next decision; it was not made in the
     audit because it cuts off access someone may still intend.  Measured 2026-11-19, the
     Zen rows are now known *why* they are load-bearing: 
     `docs/router-and-dns.md` ("The third answer is a public address") and
     `docs/home-side-hackspace-v6-2026-11-19.md`.
  2. `sshd -T` on gw reports `passwordauthentication yes` and
     `kbdinteractiveauthentication yes` (NixOS' `services.openssh` default).  Anyone in
     the ACL who can reach 22 can try a password.  The fleet's access is key-based
     (`common/users.nix`), so `services.openssh.settings.PasswordAuthentication = false`
     (and `KbdInteractiveAuthentication = false`) is the recommendation; it is not
     applied yet because a password-only account would be locked out.
  Not ports, but the other WAN responses: ICMP echo is answered on every interface
  (v4 and v6), and the DHCPv6 client accepts `udp sport 547 dport 546` on `ppp0` from
  any source.  The DHCP relay is the LNS's link-local in practice, so adding
  `ip6 saddr fe80::/10` to that rule would close an injection path; it is left as-is
  because DHCPv6-PD is not currently relied on and a wrong guess would silently
  remove the prefix if AAISP ever starts delivering it.

- **The two Zen `/48` rows in `LOCAL_NETWORK` are a stopgap, and they have a retirement
  criterion (2026-11-19).** A client whose tunnel carries only IPv4 cannot use the public AAAA
  this box hands out for `*.int` (`common/vlans.nix`'s VLAN 225 answers IPv4 + public IPv6 +
  ULA), so it dials that AAAA over whatever WAN it happens to have and needs a row naming its
  ISP prefix. The failure was 794 × 403 from `2a02:8011:d000:672::/64` on the ACL'd vhosts
  while the un-ACL'd public names answered 200/302, and it exposed that **no request has ever
  arrived here from `100.64.0.19`**, the tailnet address the working path carries. The real fix
  is on the caller's side and is written up in `docs/home-side-hackspace-v6-2026-11-19.md`;
  when that file's `curl -6` returns 200/302, **delete both rows**, and item 1 above moves two
  ranges closer to true.

  **The reading written on the spot — "Zen re-delegated, the old row expired" — was wrong and
  is corrected the same evening:** grafton-router cannot request a different prefix delegation,
  so `2a02:8010:6680::/48` is permanent on their side and stays. What is unresolved is which
  interface egresses `2a02:8011:d000:672::/64`, since it is not their PD; that question is in
  the handover note, and until it is answered the ACL carries both. The lesson generalises
  further than the prefix: a source row can also *outlive* its reason, and the pair "whose is
  this address / is it still theirs" is not answerable from this side — which is a second
  argument for the tailnet, whose addresses name a node. Re-verify with:
  `sudo grep '" 403 ' /var/log/nginx/access.log | awk '{print $1}' | sort | uniq -c`.
  **That prefix was written down three times**, and on 2026-11-19 only one copy was
  discoverable from the 403: `common/config.nix`'s `LOCAL_NETWORK`, the anonymous-access rule
  in `common/nginx-sso-config.nix` (now `^2a02:801[01]:`, covering both), and the single
  `2a02:8010:6680::/48` line in that same file's `PG_AUTH` — which did **not** follow the table
  on 2026-11-19 and nobody noticed, and fixing it means deploying services1's `pg_hba`.
  Deriving the gate's network rules and `pg_hba`'s client rows from the one ACL table, rather
  than copying it into a regexp and a `host` line, is rule 31 applied to source lists; until
  then, a change to the ACL is a three-box hunt performed by hand.

- **`nginx_sso_client_secret` and nginx-sso's cookie `authentication_key` were printed into
  an agent transcript on 2026-11-19 and need rotating.** Self-inflicted, and the mechanism
  is worth keeping: verifying the gate's new `x-real-ip` rule by dumping the config file its
  `ExecStart` names — one JSON line holding both secrets — instead of asking that file the
  one question (`sudo grep -o '"regexp":"[^"]*"' <that path>`). Rule 13 again: reading back
  a rendered file is the right move, and the read has to be *shaped like the question*. The
  store path is world-readable by design (`common/sops.nix` carries the sentence for those
  two names); the transcript is the new audience, not the new exposure. **Needs:** rotate
  `nginx_sso_client_secret` in sops **and** in authentik (client `pyBuPjaPD7xaiy8hAQpy4W02A3G0aIKyUakPtSaD`,
  the id is in `common/nginx-sso-config.nix`), `just switch` gw, and re-issue the cookie key
  in the same window — it logs every session out, so order the two rather than skipping one.


## Postgres

- **Rotate the Postgres application password (`pg_pass`) — it was printed into an agent
  transcript on 2026-10-28.** Not a config fault and not a store-boundary change: `pg_pass`
  is in `infra.buildTimeSecrets` (`common/sops.nix`) and therefore already sits in a
  world-readable store path on every box that builds or substitutes, which is the documented
  cost of that list. What is new is a transcript that quotes it: while isolating
  `error: path '…-sops-secret-pg_pass.drv' is not valid` from `nix flake check --no-build`, a
  `nix eval` of `services.outline.databaseUrl` printed the resolved DSN — password in the
  clear — instead of being answered by the rendered config. The mechanism, and why that eval
  is the wrong probe for a secret-bearing option (it also *realises* the secret's derivation,
  which is what `--no-build` was refusing), is written up in `docs/nix-dev-workflow.md`,
  "Evaluation must not build". **Blocks:** nothing; it is a one-transaction change plus the
  dependent units. **To do:** set the role password, update `secrets/secrets.yaml`, redeploy
  the boxes whose generated config embeds it (services1's URIs — outline and postgres
  itself — and anything in `CONFIG` reading `ENV_FILE`), then restart those units.
  (The `affine` role was one of them until its module was deleted on 2026-11-18; the
  role and its database are still on the box — see the item below.) The
  precedent for treating a transcript as a leak is the RADIUS shared secret (2026-10-12,
  still open below in "Switches / RADIUS").

- **The application databases still record collation version 2.42** while the box
  runs 2.44, so each still logs `database "…" has a collation version mismatch` on
  connect. `template1`/`postgres` were refreshed on 2026-10-13, which is what
  unblocked `CREATE DATABASE` (`template0` refuses the refresh because it is a copy-in-progress;
  that is why only the maintenance DBs were refreshed). Refreshing an application database is a one-liner
  each — `ALTER DATABASE <name> REFRESH COLLATION VERSION;` — but it is a *statement
  about their indexes*, so it wants a decision per database rather than a loop. The
  strict alternative is `REINDEX` of the collated indexes.

- **The `affine` Postgres role and database are still on services1, and nothing deploys
  Affine any more** (found 2026-11-18 while deleting the parked modules: `affine.nix` was
  one of them, so `postgres.nix`'s `dbs` table lost its row — which stops NixOS
  *ensuring* them, it does not drop them). Same shape for the `affine` Redis server, which
  was commented out and is deleted. **Blocks:** nothing. **To do, if anything:** decide
  whether the database holds anything worth keeping (it was never serving a running
  container as far as this tree knows: `grep -rl affine machines/` finds nothing but this
  note), then `DROP DATABASE`/`DROP ROLE` as a one-off and rotate `pg_pass` per the item
  above while the role list is being tidied anyway.

## Switches / RADIUS

- **Rotate the RADIUS shared secret** — it was printed into a session transcript on
  2026-10-12. Both ends: the switch (`radius-server key`) and the authentik RADIUS
  outpost. `docs/procurve-switches.md`.
- **Fix the authentik `agent` password** so RADIUS stops refusing every switch
  login; until then every GUI refresh writes two `login_failed` events and
  `login_failed` alerting is permanently noisy. `docs/procurve-switches.md`.
  Related tidy: `RadiusProvider` pk 7 has `authentication_flow = None` while the
  flow actually used is `radius-authentication-flow`.
- **`switch-gui` still needs its secret committed on services1.**
  `secrets/secrets.yaml` carries the `switch_gui_env_file` key (4 keys:
  `SWITCH_GUI_LOGIN`, `SWITCH_GUI_PASSWORD`, `SWITCH_GUI_OIDC_CLIENT_ID`,
  `SWITCH_GUI_OIDC_CLIENT_SECRET`) but the change is **written and uncommitted**,
  and services1/services2 get `git reset` to `origin/main` periodically.
- **`switch-gui`: the VLAN-management commands have never been sent** (create,
  rename, delete, the per-VLAN `tagged` endpoint).
- **`switch1.int` / `switch2.int` / `switch3.int` are not dnsmasq records**, so the
  gatus "Network Switches" pings use the addresses from `common/hosts.nix`. Add the
  names to the router's host overrides if they are wanted, or drop the idea and keep
  the IP pings.

## Print servers

- **`3d-blue` still carries the cloned SD identity** (machine-id and SSH host keys);
  lime was regenerated on 2026-10-03. Harmless while blue is off-network, but do it
  if blue is ever re-imaged. The `hackspace` password and `authorized_keys` are
  shared by the clone — rotate them if the boxes should not be interchangeable.
  `docs/printer-servers.md`.

## The paper printers, and the Pi Room image that prints to them (opened 2026-11-21, `docs/paper-printers.md` owns the evidence)

- **`toshiba`'s queue name is a guess, and it is the one value in
  `machines/pi-room-sys/printers-data.nix` that is not measured.** The device answers
  `OPTIONS :631/printers/` with `Server: CUPS/1.5` — a CUPS-shaped share, so the URI is
  `ipp://<addr>:631/printers/<name>` — but that server will not enumerate: `GET /printers/`,
  `/admin/` and `/jobs/` send 0 bytes, POST is refused the same way, and `OPTIONS`
  returns 200 for **every** name, so there is nothing on the network to read the name
  from. It is one line in that file, and the source of the truth is someone who already
  prints to it: `lpstat -p` on `apps1` (10.3.1.30:631's `cupsd`, which Home Assistant's
  `ipp` "Big Laser Printer" dials) once that box is back on-network — it had **no lease**
  and nothing answering on 2026-11-21, the same day `10.3.1.11` and `10.3.1.21` were dark
  too. Until it is settled, the row's queue exists, enables, and prints nothing.

- **The first deploy registered no queues at all, and the fix is a unit of its own (fixed
  2026-11-21, kept here because the symptom is what a user reports).** A Pi Room user who
  booted the image got **no printers in the dialog over either device**. Measured on a
  guest booted from that image (10.3.14.204): `cups.service` was `failed` with
  `start-limit-hit`, `lpstat: Scheduler is not running`, and the journal's first cause was
  `cups-post-start[1124]: lpadmin: Bad device-uri scheme "net+ippt"` — the `brother` row
  spelled a scheme `lpadmin -v` has never accepted. Because `hardware.printers`'
  post-start runs `set -e` under `cups.service`, that one row failed the scheduler and the
  second row was never attempted; the second row would have failed too, for a different
  reason, since `-m everywhere` asks the device and the `toshiba` answered `Host is down`
  all day. Both are now impossible: the scheme is `ipp` (measured working on the same
  guest, rc=0) and registration moved to `paper-printers.service`, which retries each row
  and never fails CUPS. **What remains by design:** a printer that is asleep when the
  client boots has no queue until it answers (`journalctl -u paper-printers` says which),
  and the test print above is still the outstanding proof.

- **A placeholder queue exists while its printer is off, and its upgrade to driverless has
  not yet been watched happen (opened 2026-11-21).** The second report of the same evening
  was a dialog with only "Print to PDF" over both printers, because both devices were
  switched off and `-m everywhere` asks the device at registration;
  `machines/pi-room-sys/printers.nix` now registers a held placeholder (`lpadmin` without
  `-m`, plus `cupsreject -r`) and upgrades it through a TCP gate when the device answers.
  The upgrade is the same `lpadmin` call that is proven against an awake `brother`, and the
  placeholder is proven on a booted client, but nobody has yet booted a client, switched a
  printer on and read `registered (driverless)` in `journalctl -u paper-printers`. That one
  observation — and then the sheet of paper in the item below — closes both.

- **Whether driverless `model = everywhere` rasterises a page from these two is
  unmeasured.** The Brother's IPP reply announces 327 bytes of attributes and sends 72,
  and neither device has a vendor PPD in nixpkgs (Brother's `beh` PPDs and Toshiba's
  `estudio*` are not packaged), so `cups-filters` + ghostscript is the whole filter
  chain this flake can build. The measurement is a sheet of paper from a booted client
  (`just pxe-client-uefi`, then `lp` inside the guest — `docs/paper-printers.md` has the
  commands), and if it comes out garbage or not at all, the options are a PPD from
  outside nixpkgs (a flake-input decision) or dialling `apps1`'s cupsd instead of the
  devices — a row edit, not a redesign.

- **The image builds, its closure is measured, and so is the squashfs the switch cut.** `nix build --no-link '.#nixosConfigurations.pi-room-sys.config.system.build.toplevel'` on aibox finished rc=0 on 2026-11-21 (479 derivations — a fresh build, since the image had not been built in that store since the 2026-11-19 netboot rebuild), and `nix path-info -h --closure-size` on that toplevel answers **20.2 GiB over 2,389 paths**: that is the store the image's `squashfsStore` carries, compresses at zstd level 10 with one thread, and hands out over NFS at boot. The printers' own share of it is `cups-2.4.19`, `cups-filters-2.0.1`, `libcupsfilters-2.1.1` and ghostscript — read off the `dry-build` listing — small against a closure already dominated by libreoffice and `nix-software-center`. **It happened on 2026-11-21 and both sides are measured.** aibox switched (generation 479, confirmed in the same invocation, zero failed units), and its netboot export — `/exports/netboot-squashfs`, mounted off a store path named `…-squashfs`; `/srv/netboot` was the wrong path to look at — went from **7,506,362,368 bytes / 2,360 entries** to **7,552,843,776 / 2,390**: the printers cost **+44.4 MiB (+0.62%)** and 30 entries. The switch itself was ~8 min wall, well under the 2026-11-19 run's numbers, because nixpkgs did not move and the image's closure had already been built. Receipts, including the loop-mount listing that shows the old image already had CUPS the *package* but not the *service*: `docs/paper-printers.md`.

5. **`machines/aibox/STRATA.md` has no receipt for the engine leg that this switch crossed.** aibox was one of the boxes left behind upstream's 2026-11-21 generations, so its switch moved `strata.service` from `sr4rrkb7…-strata-unstable` to `wd40i8bg…-strata-unstable` on upstream's 0.1.63 pin — a bump whose HIP-side receipts in that file stop at 0.1.59.1. Nothing says the engine misbehaved (zero failed units, unit active), but its answers across the bump are unscored, and that is the same shape as `docs/follow-ups.md`'s strata item on services2: score the answers before believing the bump.

6. **The 2026-11-18 warning baseline is stale at this lock: `nix flake check` printed two warnings on 2026-11-21** — `slack-agent-digest.service is ordered after 'network-online.target' but doesn't depend on it`, and Grafana's plaintext `secret_key` ("passwords will be stored as plaintext in the Nix store"). AGENTS.md's "Nix commands" section says neither reproduced at the 2026-11-18 lock; both reproduce now, on services1's modules, and the deploy's `check-build` run logged exactly these two and nothing else, so the printers add no warning. A baseline is a fact about a lock, so this line is the re-read (AGENTS.md rule 9); the fixes belong in `machines/services1/services/monitoring.nix` (the digest unit's ordering) and in Grafana's `secretKey` provider, not in a per-box recipe. **Re-read at the 2026-11-22 lock: four classes, not two.** The
  five per-machine gates of that day printed, besides those two, `getExe: Package
  "ups-watchdog-0.1.0" does not have the meta.mainProgram attribute` (three times,
  while `gw`'s toplevel was evaluated) and the `stdenv.is*` deprecations — the second
  class AGENTS.md rule 9 names as "open at the 2026-10-14 lock, did not reproduce at
  2026-11-18" — so it reproduces again now. The `ups-watchdog` half is ours to fix
  (its package needs `meta.mainProgram = "ups-watchdog"`, and `getExe`'s
  deprecated-then-broken assumption is what the router's `ups-fleet-shutdown`
  /`ups-notice` units are currently relying on); the `stdenv.is*` half belongs
  upstream or behind `packages.strata-cuda`.

- **Decide once `apps1` is reachable whether the image should dial the devices or the
  network's existing print server.** Home Assistant's `ipp` entry dialling `10.3.1.30:631`
  says the hackspace already has working queues on that box; if those queues *are*
  these two printers, the Pi Room rows should name that server (shares) rather than the
  devices' own endpoints, and the two devices stop being four protocol facts to maintain.
  That also decides what retiring `apps1` does to printing, which nothing in the tree
  currently says.

## Authentik

- **`RadiusProvider` / `groups` ScopeMapping tidy.** The `switch-gui` provider's
  `groups` scope reuses gocardless-dashboard's ScopeMapping row rather than owning
  one, because creating a fresh child row through the ORM trips a unique-violation
  on `propertymapping.name`. Fine for now; the proper fix is the web UI or the real
  API. `docs/authentik-oidc-notes.md`.

- **The authentik cutover is done** — 2026-10-28, `staged = false`, `10.3.1.36` moved onto
  services1 as a second address, four minutes of IdP-down, no client reconfigured. The
  appliance VM is stopped and must stay stopped. What is left is below; the receipts and
  the four easy mistakes are in `docs/authentik-container-migration.md`.

- **End state for `10.3.1.36` and its DNS record is undecided.** The record has to keep
  `ipv4 = 10.3.1.36` (repointing it at `.20` trips `check-network-tables.nix`'s "no address
  claimed twice", because `services1` already claims `.20`), it cannot have empty
  `macs`/`clientIds` (rule 22 of that guard), and it cannot borrow either services1 MAC
  ("no MAC claimed twice"). So it currently keeps the **retired VM's** MAC, which is a
  deliberate lie documented in `common/dns-records.nix`, and it means the VM must stay
  stopped while the record exists. Deciding means either accepting the lie permanently or
  changing the guard so a table can say "this address is a second address on a flake
  machine" — nothing in the tables can express that today. Deleting the VM, or the record,
  needs `common/int-dns.nix`'s `exempt.authentik` handled in the same change: `authentik.int`
  resolves *because* that record exists.

- **RADIUS still authenticates nobody, and the row-level reason is now known.** There is
  **no `agent` user in authentik**, so the identification stage fails, the flow (written
  for interactive prompting) recurses, and the outpost sends no reply —
  `exceeded stage recursion depth`, which the appliance did identically. Fixing it means
  creating the account the switches actually send, gating it on the `Infra` scope, and
  setting `radius-server key <new secret>` on each switch — **NVRAM, so ask first**
  (rule 16) — plus deciding whether `mfa_support = true` is right for a device login. Until
  then every switch login falls back to the local account database, as it has for months.
  `docs/procurve-switches.md`.

- **The outpost containers have no resource limits, and the stack is on the default
  `podman` network, not `authentik`.** `createCommand` (`--memory 1G --cpus 2`) is ignored
  by this nixpkgs' `virtualisation.containers` — the rendered start script is
  `podman run --replace` with no such flags — and `network = "authentik"` is ignored the
  same way, so the stack rides `podman` and resolves each other by name anyway. Both are
  dead config that reads like a setting; the fix is `extraCreateArgs` for the limits, and
  the network is **not** safe to add back casually: a running container cannot join a
  second network, so the whole stack has to be recreated together or the outposts lose
  `authentik-server`. Found by reading the rendered unit (rule 10).

- **Rotate the IdP's postgres password (was: the appliance's; 2026-10-15).** The first 40
  characters of `PG_PASS` and of `AUTHENTIK_SECRET_KEY` reached a transcript through a
  verification command whose redaction did not work. The database password is cheap now:
  it is read from `authentik_env_file` and only used by this box's own `authentik-postgres`
  container — change it in sops, `nixos-rebuild switch`, and reset it inside the cluster
  (`ALTER ROLE authentik …`), then re-dump. `AUTHENTIK_SECRET_KEY` is not cheap and is now
  **unrotatable in practice**: authentik cannot re-encrypt what it has already encrypted,
  so it would mean regenerating every stored provider key, LDAP bind password and the
  outpost tokens — all of which live in the database being sealed with it.

- **aibox's sssd: the GSSAPI/`krb5_validate` item is still open, and its test canary is
  gone.** The LDAPS path works after the cutover (`id Catryan` → groups `Infra`), but sssd
  had logged 13 `Unable to establish connection` failures *before* the window, so the
  underlying flakiness recorded in `machines/aibox/sso.nix` is untouched. And
  **`sss_userdk` no longer exists in authentik** (queried 2026-10-28), so the
  end-to-end check that file recommends cannot pass — use a real member username.

- **The netboot image boots and authenticates since the Authentik move; only the `pam_exec`
  filestore mount is still unobserved** (re-verified 2026-10-29 on aibox, `just
  pxe-client-uefi` against the deployed config, and the boot is now observed end to end:
  TFTP → the blank `autoexec.ipxe` fallback, lease from the dynamic pool, then menu/logo/
  `bzImage`/`initrd` all 200 **from the client's IPv6 address**, the initrd mounting the NFS
  store, `root=tmpfs` with `/nix/store` an overlay, zero failed units,
  `display-manager.service` active with an sddm session, NAS `/home` mounted, and root SSH
  into the guest on the image's own `leigh-admin@aibox` key — which is how the next half was
  measured). **Authentication was checked from inside the image**, not beside it:
  `id Catryan`, `id cjdell` and `getent group Infra` all correct over
  `ldaps://authentik.int.leighhack.org` with `ldap_tls_reqcert = demand`, and the `demand`
  is live rather than dead config, because the identical bind pointed at a bogus CA returns
  `Can't contact LDAP server (-1)`. **Re-run that control whenever this is doubted** — a
  passing bind only proves certificate validation when a failing one is possible. The
  server agreed: the LDAP outpost logged the guest (`10.3.14.180`) binding as `pgina` and
  returning results, and `authentik_events_event` records 57 LDAP-Outpost `login` events
  from `10.3.14.0/24` after the cutover — 54 service binds and **3 human authentications
  from `10.3.14.101`/`.102`** (the real boards; `machines/pi-room-sys/README.md` says how to
  attribute a board in aibox's nginx log), **zero `login_failed`**. So the earlier form of
  this item — "verified on the wire, but nobody has booted one" — is closed on both halves.
  **What remains:** the `pam_exec` login-script half
  (`machines/pi-room-sys/configuration.nix`, the CIFS mount of `//10.3.1.6/filestore` as
  `hackspacer`), which runs only on an interactive login and is `optional`, so nothing fails
  visibly if it breaks. **To do:** log in at a Pi Room console once and confirm `~/Filestore`
  appears; both boards were powered off and not answering ping as of 2026-10-29, with their
  reservations still live on the router. **Do not substitute a deliberately-failed bind
  against `pgina` for that** — every LDAP client in the fleet, aibox included, binds as
  `pgina`, so a staging or lockout event on it is a fleet-wide auth outage. That is why this
  item stops short of the last inch rather than testing it.

- **sssd's nested-group walk fails against the LDAP outpost on every box, and nested group
  membership really is being lost** (found 2026-10-29 reading a booted netboot client's
  journal, where it is not optional reading: `Error processing direct membership [22]:
  Invalid argument` → `Nested group processing failed` → `Group lookup failed` → `Due to an
  error we will return cached data`, three times in one `id` call; the same six lines on
  aibox this boot). **This started as a "probably noise" note and is not one.** The data
  shape: four groups carry **groups** as `member` (`Members` nests `Directors`, `Infra`,
  `Trusted`; `Public` nests `Members`), and every user's primary `gidNumber` (e.g. 2019) has
  **no group object at all**, so the nested pass asks LDAP for something it cannot answer —
  and the walk dies, taking the nesting with it. Measured both ways, not assumed: `Members`
  is a member of `Public`, and of its 50 users 42 are **not** direct members of `Public`, so
  those 42 should gain `28164(Public)` and do not — `id bobm6tma`, `id jeff`,
  `id peterashford` → `(Members)` and nothing else, while `Johnze`, a **direct** member of
  `Public`, gets `(Public)` normally. Same in the other direction: `hack_admin` is in `Infra`
  ⊂ `Members` and gets `(Infra)` only. So the outpost does *not* flatten, and every
  indirect membership is silently dropped for NSS/PAM consumers.
  **Blast radius, which is why this is a finding and not an incident:** the Unix side of this
  fleet gates on `Infra` alone — `security.sudo.extraRules` in
  `machines/pi-room-sys/configuration.nix`, the same on aibox, and `Infra`'s 11 members are
  all direct, which is exactly what does resolve (`id Catryan`, `id cjdell`, `id phyushin`
  → `21159(Infra)`) — and nothing in tree reads `Members` today (the module that did,
  `gitlab.nix`, was deleted 2026-11-18). App-level group gates
  (OIDC `groups` scope for Grafana/switch-gui/filestore/gocardless-dashboard) go through
  authentik's API, not sssd, so they are unaffected. Nobody is locked out today; the day a
  share, unit or `required_groups` names `Members`/`Public`, it will name a group most of its
  members do not have. **It is not the Authentik move's doing**: the config that walks
  nesting (`ldap_schema = rfc2307bis`, `ldap_group_member = member`) predates it by a year,
  and it only *starts* on the cutover day in aibox's persistent journal because 19:22:34 that
  day is the last time sssd restarted.  **Do not silence it with
  `ldap_group_nesting_level = 0` either** — measured 2026-10-29 on a booted netboot client, the
  option does not reduce the failures (9 cold-cache lookups became 18 with it set), because the
  failing step is direct-membership processing, not a depth limit.  **To do:** fix the shape, not
  the symptom — every user's primary `gidNumber` names **no group object at all** (105/105),
  which is what sssd is erroring on, so give users a real primary group or stop treating this
  tree as a Unix group source.  `docs/auth-posix-logins.md` F3. **Do not confirm the effective
  config with `sssd --diagnostic` into a transcript** — it dumps `ldap_default_authtok` in
  cleartext (rule 13). Reproduce: `just pxe-client-uefi`, ssh in as root from aibox, then
  `journalctl -b -u sssd | grep -i nested` and `id bobm6tma` beside `id Johnze`. **And do not
  probe this with hand-rolled bind scripts that read `ldap_default_authtok` from a file: a
  `sed` anchored at column 0 silently produced an empty password and two `login_failed`
  events against `pgina` on 2026-10-29** (account still `is_active = t`, verified). Extract
  the token with `grep … | awk '{print $2}'` and assert it is non-empty before binding — a
  lockout on `pgina` is a fleet-wide auth outage.

- **Every uid in the fleet is `2000 + an authentik Postgres primary key`, and that number *is*
  the ACL on people's home directories** (measured 2026-10-29: `uidNumber == 2000 + id` for
  105/105 users LDAP serves; no uid column, 0/106 users carry a `uidNumber` attribute, redis
  `dbsize` 0; and `ds-home/cjdell` on the NAS is `uid=2013 gid=2013` mode `0700` while cjdell's
  pk is 13).  Group gids are generated too, stored nowhere, and are not any simple hash of the
  group name or uuid (11 known pairs; crc32/adler32/md5/sha1/sha256/blake2b × 7 input spellings
  × 17 moduli, no match) — so the honest finding is that nothing pins them.  **Blocks:** any
  restore that does not preserve primary keys; any delete-and-recreate of a user — reachable by
  accident, because `gocardless-dashboard`'s nightly sync *creates* authentik users; and any edit
  to the provider's `uid_start_number`, which is a field in the web UI.  Failure order: the
  person loses their home (`0700`, so not even an admin group can clean it up), then whoever is
  later assigned that number gains it.  **To do:** `crates/authentik-posix --audit` first — dump
  the map, cross-check against the NAS, alert on drift — then pin the numbers as stored
  attributes, with the tool allowed to write only a number the files already use and required to
  refuse otherwise.  `docs/auth-posix-logins.md` §P1.

- **The netboot image ships a bind credential it cannot keep, and there is a way to stop needing
  one** (measured 2026-10-29: the outpost is *not* the problem — empty bind `Invalid credentials
  (49)`, wrong `(49)`, anonymous `Insufficient access (50)`; so no bypass.  But the credential
  reads all 105 members' `cn`, `displayName`, `mail` and `memberOf`, which is the member list and
  therefore personal data, and it lives in a squashfs anyone who can plug in an Ethernet cable
  mounts.  `attributes` are not served, so door-card UIDs and customer ids are safe.)  **Blocks:**
  nothing; it is a standing disclosure.  **To do:** serve the POSIX map from aibox the way aibox
  already serves the client's own root filesystem — no new trust, no credential in any built
  image, survives an IdP restart, and makes the uid map an auditable artefact — and authenticate
  console logins with a per-user bind, which needs no service secret because `cn=<user>` plus
  that user's password is already all a bind is.  Decide deliberately first: the provider runs
  `bind_mode = direct`, so a per-user LDAP bind **bypasses authentik's flows and MFA entirely**
  and lands in the event log as a plain `login`.  `docs/auth-posix-logins.md` §P2.

- **`id`/`getent` cannot show this subsystem is alive, and more than one thing here assumes they
  can** (measured 2026-10-29: a client with `ldap_uri` pointed at `no-such-host.invalid` kept
  answering `getent passwd` for a user never queried that boot — nscd runs in front of sssd on
  both boxes, and sssd keeps its own sysdb; both are tmpfs on a netboot client and *still*
  survive a config change within one boot, which is why an earlier item today wrongly called
  those lookups live).  A health probe has to bind and search, not `getent`.  Three instrument
  traps, each of which cost time: sssd's cache is `/var/lib/sss/{db,mc}`, **not**
  `/var/lib/sssd/`; `ldapwhoami` cannot probe this outpost (no Who-Am-I — a *successful* bind
  answers `Protocol error (2)`), use `ldapsearch`; and sssd's `option# = /file` secret convention
  is **unverified here** — every test was defeated by those caches, so it does not go into the
  config of a machine that logs people in until it is proven.  Recipe in
  `docs/auth-posix-logins.md` §F5.

- **Every origin behind the front door still rewrites `X-Forwarded-Proto` and `X-Real-IP`;**
  `id.int` and `outline` have opted out, the rest have not. nixpkgs' `recommendedProxySettings`
  sets `$scheme`/`$remote_addr` in each origin's `location /`, so an origin believes its
  clients are `10.3.1.1` and that nothing uses TLS — the door's honest values arrive and are
  then overwritten. It broke authentik the day `id.int` became an origin (mixed content; fixed
  2026-10-28 with `preserveForwardedHeaders = true`), and it broke `outline.leighhack.org`
  **completely** for three weeks from the same cutover: an app that enforces HTTPS itself
  redirects to the https URL, the door answers and re-proxies it as `http`, and every request
  on the site 301s to itself forever. Symptom measured 2026-11-16 — `ERR_TOO_MANY_REDIRECTS`,
  and the fix was `preserveForwardedHeaders = true` on that vhost after reading Outline's own
  middleware and confirming it has **no inbound IP allowlist** (auth is OIDC only), so nothing
  could start refusing traffic; receipts and why `forceHttps = false` is the wrong fix are in
  `common/nginx/origin-vhost.nix`, which is also the file to read before flipping another
  origin. Auditing the rest means reading each app's own IP allowlists first: today a LAN-only
  rule in one of them admits the internet, so fixing the header is a behaviour change and each
  flip wants its own commit. The invariant that makes the flip safe is already true —
  `originReachabilityRules` in `machines/gw/firewall.nix` plus `CONFIG.ROUTER_ONLY` mean no
  client reaches an origin except the door, so the forwarded headers cannot be forged.
  A sweep of all 54 door names on 2026-11-16 found exactly two still showing the lie:
  `outline` (fixed) and `user-tweaker.int`, which is **not** an origin — the door proxies it
  straight to `10.3.1.30:8084` (`apps1`, off-flake), where the container re-derives the scheme
  itself, so nothing in this flake can fix it. It emits `redirect_uri=http://user-tweaker…`,
  which means the OIDC authorization code crosses one cleartext hop before the door's
  `forceSSL` catches it — worth fixing on `apps1`, and the only other victim found.

- **`outline.leighhack.org` has no gatus endpoint, and that is why a public site answering
  nothing but `301`s was nobody's alarm for three weeks** (found 2026-11-16, while fixing it
  above). `common/monitoring-endpoints.nix` carries rows for `user-tweaker` and the APIs but
  none for Outline, so `gatus` reported every one of its 37 endpoints UP while the site was
  completely unusable — the same failure shape as a stale dashboard: a monitor that does not
  watch a thing cannot distinguish "quiet" from "dead". The row is one `mkHttp` and a
  `[STATUS] == 200` condition, which is precisely what a self-redirect loop fails (a loop
  answers `301`, never `200`); it is deliberately not bundled into the fix commit. Audit the
  rest of `common/front-door.nix` against the endpoint list while adding it — the sweep found
  four other names answering `502` (`jenkins.int` and `ai.int` have dead upstreams, `voron`
  is offline, `firewall.int` is the deliberately-dead row) and none of them are monitored
  either.

- **`https://authentik.int.leighhack.org/` serves nothing, by design, and never did.** The
  name resolves to `10.3.1.36` (services1) rather than to the door and services1 serves no
  TLS; the appliance answered web on `:9000` only, so the same was true before the cutover.
  The alias in `common/front-door.nix` is unreachable through DNS, and the `exempt` row in
  `common/int-dns.nix` is what keeps RADIUS/LDAPS working. Settling it means choosing:
  leave the name a bare-address LDAPS/RADIUS service, or move it to the door and re-point
  the two LDAP clients (aibox and the netboot image) at a name the door answers — a redeploy
  of two machines, not a migration.

- **`10.3.1.36` in the operator's `known_hosts`.** Three lines hold the retired VM's host
  keys; ssh to `.36` now reaches services1, so those entries can never validate again and
  the first ssh after the cutover fails hard (it did, mid-window).

- ~~`id-shadow.int.leighhack.org` has no DNS record~~ — **closed by deletion, 2026-10-28.**
  The name is gone from `common/int-dns.nix` and from the module, and staging needs no
  name: `curl -H 'Host: id.int.leighhack.org' http://127.0.0.1:9000/…` reaches the app with
  neither DNS nor the door involved.

- ~~The nightly authentik dump timer is off~~ — **on since 2026-10-28** (`staged = false`).
  `authentik-db-dump.service` was run by hand once (`Result=success`, 27 M custom-format
  dump into the backups share); the first *unattended* 03:15 run is unobserved, so check
  `journalctl -u authentik-db-dump.service` after the next one.

- **The retired appliance VM is now doubly kept down** — HA desired state `stopped` plus
  `qm set 108 -onboot 0` (it was `onboot: 1`, the only VM on monster1 changed, deliberately:
  it owns an address that belongs to another host now). If the VM is ever decided to be
  permanently gone, the `exempt`/`dns-records` question above has to be answered in the same
  change, and the two `/srv/authentik` copies are the only pre-cutover state.


---

## Cameras (OpenIPC, VLAN 226) — opened 2026-10-25, `cameras/README.md` owns the fleet

Filed the same day the fleet was first looked at; everything measured is in
`cameras/README.md` ("Faults found on 2026-10-25") — these are the five things
that **need an owner or a decision** rather than another measurement.

- **A Telegram bot API key is in a comment in cam7's `/etc/crontabs/root`**
  (line 20 of the vendor's own example block; the command it comments out would
  post a snapshot of the camera to that bot). **The value is deliberately not in
  this repo**: `cameras/config/cam7/root` has the line replaced by a redaction
  note, and `cameras/inventory.sh` reports that file's md5 and line count rather
  than cat'ing it, because `cam-pull` puts these files into a store path and a
  future transcript (rule 13). **Blocks:** nothing — it is a commented line, so
  nothing runs, and crond reads `/etc/crontabs/root` (0 active lines today).
  **Needs:** whoever owns that bot to rotate the token, and the line deleted on
  the device (it is the vendor's, not ours). Do not "just delete it from the
  repo copy": the device still has it.
- **frigate's camera config is not in the repo and is deciding to do nothing.**
  `/srv/frigate/config/config.yaml` is a **runtime bind mount**
  (`machines/services1/services/frigate.nix` mounts it, nothing generates it),
  and its go2rtc table (`/dev/shm/go2rtc.yaml`, in-container, regenerated at
  start) is the thing that maps a feed name to a camera. Today it **disables**
  `3d_1`/`3d_2`/`3d_3`/`voron` and comments out `rack_corridor`'s
  `cam1.int.leighhack.org:554/0/av0` line while leaving an `ffmpeg:`
  transcode line in play. **Blocks:** a camera fault that looks like "frigate
  is fine but the picture is old" cannot be told from "the config stopped
  meaning what the flake says". **Needs:** that file tracked (a root-owned file
  under `/srv`, or generated into the mount the way `strace-launcher.sh`
  generates `go2rtc.ini` into `/data`) **and** the mapping
  device→stream→camera-name in one table `cam-*/frigate` can read, since the
  three names for one feed (the DNS name, the go2rtc stream key, the frigate
  camera) are what `common/monitoring-endpoints.nix`'s gatus rows and
  `just cam-probe` both depend on today being right by hand.
- **`cam1`/`cam5`/`cam7`/`cam9` are `*.int` names that answer with a LAN device
  rather than the front door, and no guard describes that path.**
  `curl http://cam5.int.leighhack.org/` answers **from cam5's own busybox
  `httpd`** (401, realm `Authentication`) — the name resolves to the device, not
  to `gw` — yet none of those four names is a row in `common/int-dns.nix`, the
  table rule 14 says every `*.int` name belongs to, and `machines/gw/int-dns.nix`
  (gw's vhosts must equal that table) and `machines/services1/int-dns.nix`
  (the origin's must equal the `upstream = "origin"` rows) both pass with the
  names absent. **Blocks:** the decision, not a build: adding rows on a guess is
  how a second front door gets built for a name whose real answer is a camera.
  **Needs:** read `docs/router-and-dns.md` + `docs/router-front-door-migration.md`
  first and decide whether a device's own `*.int` name belongs in `int-dns.nix`
  at all (it is a *different* kind of row: "serves, must not answer with the
  door's address", i.e. the `exempt` half of that table) or whether these names
  stay outside the table and the gatus rows that use them stay hand-written.
- **The fleet's tooling is shell today; the house rule wants Rust** (rule 1:
  no new services/CLIs in other languages, `crates/status-dashboard/` is the
  pattern). The justfile's `cam-*` recipes **work** and were validated against
  the devices on 2026-10-25, which is why they are the shipped thing. **A
  `crates/camera-monitor/` attempt was started the same day and deleted**: its
  `src/main.rs` was 10 lines of not-Rust (`[const string]`, `?(bool, string)`
  — a code-shape mix-up that no compiler error had taught yet), and it was never
  a workspace member, so it was a trap for the next agent rather than a start.
  **Do not "fix" that stub: write the crate from a clean slate against
  `cameras/README.md`.** What the pass learned for whoever does:
  `crates/common-rs/http-client` has **no auth support at all**, so the tool needs
  its own RTSP `DESCRIBE` client (the `401 / realm WallyWorld` receipts in that
  doc are the evidence the feed is gated), the write path must be
  md5-then-replace-then-restart, and `just check`/`just check-build` must stay
  gw-safe via `_build-host-guard` (the recipe in `just/deploy.just`, `hostname`-based) —
  nothing about these devices is a nix build, so the crate stays off the deploy
  sets. **Blocks:** nothing.
- **~~OpenIPC's current `ssc337` builds have dropped the C110's WiFi driver, and `cam8` is off
  the air because of it~~ — RECOVERED 2026-10-28 from the serial console.** (found
  2026-10-28; `docs/camera-sd-card-firmware-flash.md` §4 gates on it, §9 records the updater
  behaviour). `cam8` = the spare C110 **v1** at
  10.3.2.112 (`ssc337 (infinity6b0 family), sc3338`, `wlandev=ssw101b-ssc333-tapo-c110`,
  env `soc=ssc337`), which ran OpenIPC `master+0f21c01, 2024-08-30` and was **not** in any
  table — no row in `common/dns-records.nix`, so its address is a plain DHCP lease. A
  rootfs-only flash of `nightly-20261028-9a84dce` `ssc337_lite` left it addressing nobody:
  the new rootfs has `/etc/wireless/usb` naming `ssw101b` and **no `ssw101b_wifi_usb.ko`**
  (the 2024 rootfs did; `ssc337_ultimate` from the same night is the same), so it boots
  without WiFi. Confirmed from the LAN — no ICMP, `ip neigh` `FAILED`, no lease renewal
  since 21:43 — and the same is true of every current nightly, so **rolling back, not
  forward, is the repair**. A verified 5/5 flash dump and the `/etc`+`uboot-env` archive
  exist in `~/camera-backups/cam8-flash-20261028T213947Z` and `cam8-etc-20261028T214018Z`
  (mode 700, never in the repo), and the SD card holds that dump's kernel+rootfs as
  `uImage.ssc337`/`rootfs.squashfs.ssc337` with the 2026 pair in `2026-9a84dce/`.

  **How it was fixed, 2026-10-28 (3.3 V UART at 115200 8N1 on `/dev/ttyUSB0`, the card in
  the slot):** Enter-spam during power-up interrupted `bootdelay=0` and landed at
  `OpenIPC #`; `run setsdcard`; `fatls mmc 0` showed the dump pair at the card root
  (2 097 152 / 5 242 880 B — full-partition dumps) and the 2026 pair in `2026-9a84dce/`
  (2 003 528 / 5 156 864 B); the **rootfs only** was written (the kernel was never what
  broke), `reset`, and the camera came back as the 2024 build — `wpa_supplicant` up, DHCP
  `10.3.2.112` from `10.3.2.1`, ICMP clean, `:80` and `:554` both `401`. **The kernel was
  not touched and `ubnor` was not run**, so the env, the WiFi PSK, the host keys and the
  jffs2 settings partition are all as they were.

  **The finding worth keeping: `run urnor` does not work on this U-Boot, and it lies.**
  Its `sf erase 0x250000 0x500000` is refused (`ERROR: attempting erase past flash size
  (0x800000)` — a range that is inside the chip), the following `sf write` runs anyway and
  prints `5242880 bytes @ 0x250000 Written: OK`, and the partition is then a bitwise-AND of
  old and new. Only a read-back `cmp.b` caught it. `sf update` (erase-then-write per 64 KiB
  sector, ~26 s) plus `cmp.b` over the whole partition is the sequence that works; the
  procedure, the receipt and the corrected "never trust the byte count" rule are in
  `docs/camera-sd-card-firmware-flash.md` §8, and §11-1/§11-2 (card socket; Enter
  interrupts autoboot) are closed by the same session.

  **Closed 2026-10-31:** `cam8` joins the fleet properly — it has a `cam8` row in
  `common/dns-records.nix` (MAC `3c:52:a1:49:01:05`, vlan 226, **10.3.2.11**; it had been
  arriving on a *dynamic* 10.3.2.112, inside VLAN 226's own range), deployed through
  `machines/gw` normally, and a frigate camera of its own. It was **off the air** while
  that was done and came back on `.11` minutes later — 5/5 fps in frigate — so the
  reservation is confirmed on a real boot, not just in `dnsmasq.leases`. Its ssh is not
  keyed (the changeover wipes the data partition): `just cam-key cam8` first.

  **Which build to use, measured 2026-10-28.** The `ssw101b` gate is necessary but not
  sufficient on its own — **it has to be run on the image you are about to flash, and only
  the board-specific asset passes it.** On `master`, `BR2_PACKAGE_SSW101B` is selected by
  exactly three configs: the builder device files `ssc337_lite_tp-link-tapo-c110-v1` (ours),
  `ssc335_lite_*` and the `ssc333`/`ssc335` *ultimate* SoC configs. The **generic**
  `ssc337_lite`/`ssc337_ultimate` defconfigs never select it — which is why the generic
  build bricked this camera. Verified against the published assets:

  | image | `unsquashfs -l … \| grep -c ssw101b` |
  | --- | --- |
  | `nightly-20261010-a40b399/ssc337_lite_tp-link-tapo-c110-v1-nor.tgz` | **1** (`lib/modules/4.9.84/extra/hal_apollo/ssw101b_wifi_usb.ko`) |
  | `nightly-20261027-d20e5c3/ssc337_lite_tiandy-tc-c321n-v2-nor.tgz` (generic) | **0** |

  **`nightly-20261010-a40b399` is the newest published build for this board** — from
  2026-10-11 every dated nightly is missing the `ssc337_lite_tp-link-tapo-c110-v1` asset
  entirely (the `Firmware (ssc337_lite_tp-link-tapo-c110-v1)` job still runs and fails at
  *Build firmware*; `ssc337_lite_h3c-tc2101` and the `c110` v2/v26 builds fail the same way
  while `ssc337_lite_tiandy` still succeeds), and the rolling `nightly` tag still serves the
  2026-10-10 file (`updated_at 2026-10-10T21:55:54Z`). That image is rootfs 5 238 784 B of
  a 5 242 880 B partition — **4 KB spare** — with `md5 23fdb543…` / `fb28af07…` matching the
  release's own `.md5sum` files.

  **Two things to check before flashing it.** (1) The image **prunes `sc3338`**: its
  exclusion list drops `/etc/sensors/sc3338.bin` and `sensor_sc3338_mipi.ko` (the board is
  SC3336 per the firmware repo's board guide), leaving sc3335/sc3336/sc401ai — and `cam8`'s env says
  `sensor=sc3338` and its current rootfs loads that module with no `insmod` error. So WiFi
  would come back and the *picture* is what needs proving; `ipcinfo -s` (a real probe, not
  the env) decides it. (2) The build's own kernel is 2 003 264 B of a 2 097 152 B partition,
  so the kernel can be written too if the `uImage` is ever suspect.

  **DONE 2026-10-28: `cam8` runs that build, and it is on the air with a picture.** The
  probe (`ipcinfo -s`) said **`sc3338`**, so the pruning mattered, and two things had to
  be true that the release alone does not give you: the **`sc3338` module had to come from
  the outgoing rootfs** (9 956 B, works unchanged on the new kernel); and the **kernel had to
  be flashed with the rootfs**,
  because the new `ssw101b` driver oopses on the old one (`Unable to handle kernel paging
  request at virtual address 000040c0` in `Sstar_usb_memcpy_fromio`, no `wlan0`, and
  `reboot` then hangs, so the recovery from that state is a power cycle). Keeping the old
  driver in the overlay instead is not an option: it is 744 504 B against a 704 KB overlay,
  and filling that partition stalls the box. Verified end to end: `Linux 4.9.84 … Oct 10
  2026`, DHCP `10.3.2.112`, H.264 **2304×1296 @ 20 fps**, `:80` index 200 / CGIs 401,
  overlay 216 KB of 704 KB. The measured recipe is `docs/camera-sd-card-firmware-flash.md`
  §9a. **Still open:** whether `cam8` joins the fleet (a `dns-records.nix` row by MAC) or
  stays a bench spare — and the 2024 pair is still at the card root as its rollback.
- **`cam8`'s OpenIPC-era TX ceiling was never the camera** (measured 2026-10-29 as ~350 kbit/s
  up against 716 KB/s down, and attributed to the camera's transmit chain). That attribution
  was wrong and the item that told someone to swap the `ssw101b_wifi_usb` module to fix it is
  deleted for that reason: the fault was the AP and its channel, `cam8` streams at 1.38 MB/s on
  the replacement AP, and `docs/camera-wifi-cells-2026-10-31.md` §3 says **don't** run that
  module swap. Read the cell before the camera.
- **`cam5`'s pulled `S99sdconf.sh` is not cam5's** (device md5 `ae29e67d…`,
  `cameras/config/cam5/S99sdconf.sh` md5 `1b4ac291…`, which is cam9's copy).  Every
  other pulled file's md5 matched its device byte-for-byte on 2026-10-25 (the
  report's `etc_md5s` line vs `cameras/config/`), and cam7's crontab disagrees
  **by design** (the repo copy is the redacted one, see fault 1). **Blocks:**
  nothing — no recipe deploys `S99sdconf.sh`, and the SD hook can never fire on
  these boxes anyway (no `/dev/mmc*` node). **Needs:** `just cam-diff cam5`, then
  `just cam-pull cam5` to replace the copy, then re-check the md5s; do not
  hand-copy cam9's file over it and do not trust the record for that one file
  until the md5s agree.
- **These three cameras are a fleet of three different hand-configured sets**
  (cam5's `/etc/overlay/upper` has 15 files, cam7's 25, cam9's 21; cam9 has no
  `isp:` section in `majestic.yaml` while cam5 and cam7 have one; cam7 has no
  `bitrate`/`profile` lines while cam5 does; cam5 has no `S96autonight` while
  cam9 does). **Needs a decision:** unify them by pushing one reviewed file to
  all three and re-`cam-diff`ing, or keep them per-device and let
  `just cam-diff` be the record. Unifying means picking the sensor pad per
  device (they are **not** all the same sensor: cam5/cam7 are `sc3338`-based
  SSC009A, cam9 is `sc401ai` SSC33X), so it is a decision with a hardware
  consequence, not a config tidy-up.
- **Camera firmware: the two physical blockers were closed on `cam8` on 2026-10-28, and a
  flash has now been run once** (re-measured 2026-10-25 with the cameras keyed; the first
  real flash was `cam8`'s rootfs rollback on 2026-10-28 — see the item above). The two
  questions that stopped an update are both answered on that board: **there is a card
  socket** (populated, `mmc0: new high speed SDXC card`, `fatls mmc 0` lists it) and **Enter
  does interrupt `bootdelay=0`** on `U-Boot 2015.01 (Nov 05 2023)` / `Version: I6gf3a2dcf`
  (a CR stream at 10/s during power-up lands at `OpenIPC #`). What the session also found is
  that the documented sequence was wrong: **`run urnor` refuses its `sf erase` and then
  writes over un-erased NOR while reporting `Written: OK`** — `sf update` plus a read-back
  `cmp.b` is the usable form, and `docs/camera-sd-card-firmware-flash.md` §8/§11 now carry it.
  **Blocks:** nothing for `cam8`; the same two facts are still *unverified* on cam5/cam7/cam9
  (pads, socket population, and the layout of their PCBs are per-board). **Needs:** one bench
  session per board, following
  **`docs/camera-sd-card-firmware-flash.md`** §5–§6; that document is the procedure and its
  §11 is this list. Everything else it needs is already taken: verified flash dumps and `/etc`
  archives for all three, the staged builds, and a pre-flight that passes 9/1/0 on cam7.
- **cam7's web admin answers `200` with no authentication** (measured 2026-10-25, from
  services1 **and from the device**: `:80` and `:85`, a 228-byte body, by IP and by
  `cam7.int`, `Access-Control-Allow-Origin: *`), while cam5 and cam9 answer `401`.
  `/etc/httpd.conf` is byte-identical across the three and there is **no
  `/etc/httpd-htpasswd.user` on any of them**, so nobody has yet found what makes cam5 and
  cam9 gate — which is the actual blocker: the fix is "make cam7 look like cam5" and the
  mechanism is unread. Now reachable from the inside (keyed), so this is a diagnosis task,
  not an access task. **Blocks:** using cam7 as the first flash target, and any claim that
  the fleet's admin plane is closed. **Needs:** find where the `401` credentials come from,
  restore parity, re-probe, and re-read cam5/cam9 while keyed.
- **The WiFi PSK for `Leighhack` is in the repository in cleartext, and was also printed
  into a working transcript; rotate it.** Found while checking this commit for leaks:
  `machines/pi-room-sys/configuration.nix` builds a CIFS `mount` line with
  `user=hackspacer,pass=<the same PSK>` **in the source of a netboot image** — so the
  credential is in git history, in a store path on every box that builds or substitutes
  `pi-room-sys`, in the squashfs the Pi Room clients boot over NFS, in the cameras' U-Boot
  env, and (as of today) in the flash dumps under `$HOME/camera-backups`. It is not a
  camera-only secret and never was: it is the network key and a file-server login. The
  transcript half happened because `fw_printenv` dumps the env, which holds `wlanpass`, and
  the call predated the masking; both masks exist now (`just cam-env` masks by key name in a
  single `sed -E` pass; `cameras/sd-card-check.sh` and `cameras/preupdate-check.sh` never
  print env values). **Blocks:** nothing, and it predates this pass — which is the point of
  writing it down. **Needs:** (1) a rotation window; (2) the pi-room mount moved to a sops
  secret or an `EnvironmentFile`, the pattern every other service here uses
  (`AGENTS.md` §Secrets — the rule that says reference paths, not values); (3) a decision on
  whether one string should be the WiFi key, a CIFS login and the camera admin baseline
  simultaneously, because it currently is three credentials in one rotation. `fw_printenv` on these boards dumps the environment, and the environment holds
  `wlanssid` and `wlanpass`; an ad-hoc call made before the masking existed put the value on
  screen. Both masks are in place now (`just cam-env` masks by key name with a single
  `sed -E` pass; `cameras/sd-boot-probe.sh` masks in pure shell), and `cam-flashdump`
  directories are mode 700 — but the dump of the `env` partition **contains the PSK in
  cleartext**, as does every `/etc` archive (`/etc/wpa_supplicant.conf`), so this is a
  rotate-the-key item and not a delete-the-file item. **Blocks:** nothing. **Needs:** a
  rotation window (it moves every client on the network, the cameras included — their copy
  lives in the jffs2 overlay *and* the U-Boot env, so a camera needs `cam-init`-style
  re-provisioning or an env write, and §12 of the flash guide says do not `fw_setenv`; the
  no-console route that avoids the env altogether is now `docs/camera-wifi-recovery.md`).
- **The camera admin password is not the secret this directory believed it was** (found the
  evening the cameras were keyed). It is the RTSP password, and the RTSP passwords are in
  `/srv/frigate/config/config.yaml` because frigate must authenticate — so
  camera password = RTSP password = a string in a runtime bind-mounted YAML, and the rule
  that shaped every `cam-*` recipe ("the password is on no machine, so keying needs a human
  at a tty") was false. **Blocks:** nothing; it retires a threat model. **Needs:** a decision
  on whether camera RTSP credentials move into a secret-rendered env file like every other
  service's, and — separately — whether three cameras should share one password at all,
  since they also share the `wlanpass`.
- **`/etc/custom.ok` is settled, and the answer is about cam7** (measured with `[ -e ]`):
  present, size 0, on cam5 and cam9; **absent on cam7**, which is the marker the OpenIPC
  install story says a fresh rootfs *deletes*. So the device with the open admin plane is also the device
  whose install never reported completion — consistent with it being two firmware versions
  behind. **Blocks:** nothing. **Needs:** nothing, unless cam7 is reflashed, in which case
  its absence is the baseline and not a symptom.
- **cam5 runs a boot-time script that executes `/sstar_mmc_upgrade.sh` from the root of any
  mounted FAT volume, as root** (`/etc/S99sdconf.sh`, md5 `1b4ac291…`, which the repo
  already holds and marks "delete this"). It also `mount -o remount,rw rootfs /`, and `/`
  turned out to be a **writable jffs2-backed overlay**, not the tmpfs this fleet's docs
  described — so the write it performs persists. cam7 and cam9 do not carry the file.
  **Blocks:** any claim that cam5's `/etc` is unreachable to someone with physical access
  and a card (which is exactly the access a firmware flash needs). **Needs:** execute the
  already-recorded decision (`just cam-init cam5 delete etc/S99sdconf.sh`) or write down why
  the file stays.
- **Tooling state after the first real run**: `just cam-preflight` / `cam-flashdump` /
  `cam-backup` / `cam-image` / `cam-env` / `cam-card` all run against real devices, and the
  first run found four silent-wrong-answer bugs in them (a dump that stopped after one
  partition and printed `OK`; the `mtdNro` siblings; `ssh -n` eating a piped script; a
  `case`-in-`while-read` mask collapsing a 34-line env to one line). All four are fixed and
  listed in `cameras/README.md` *How this was verified*. **What is still unverified** is the
  write path: no recipe in this directory has ever written a camera's flash, and the dumps
  and archives are the only rollback anyone has. **Blocks:** nothing until someone flashes.
  **Needs:** re-run `cam-flashdump` + `cam-backup` on the day of any flash, since the dumps
  in `$HOME/camera-backups` are the undo and they are a snapshot of *today's* config.
- **cam1 serves its video with no credentials at all** (measured 2026-10-25, from services1,
  unauthenticated and read-only): `DESCRIBE rtsp://10.3.2.6:554/0/av0` and `/1/1` answer **`200`**,
  where cam5/cam7/cam9 answer `401, realm "WallyWorld"`. Its banners fingerprint it while we were
  there — RTSP `Server: TAS-Tech Streaming Server V100R001`, HTTP `Server: Ginatex-HTTPServer`
  (302 → `/index.asp`, `/login.cgi` → `401`), telnet `:23` **open** — that banner pair is the
  Xiongmai-family application stack (an inference from two banners: nobody has seen cam1's SoC),
  so it is not OpenIPC and not Ingenic, and none of this fleet's verbs apply to it either way.
  **This is fault 13 in `cameras/README.md` and it outranks cam7's open admin**, because cam7's
  open door is a web UI and this one is the picture, pulled anonymously by anything on a VLAN
  whose gateway is the router itself. **Blocks:** nothing; the exposure is passive and live, and
  frigate's `ffmpeg:` line for `rack_corridor` is fed by it today (the device's own admin plane
  has never been inventoried, so nobody knows whether this is a setting or the factory default).
  **Needs:** decide whether `rack_corridor` is *meant* to be open; if not, put credentials on the
  device (which means finding its login, likely the telnet one) or make 10.3.2.6 reachable only
  from services1 at the router — the `docs/router-and-dns.md` VLAN 226 rules are the place that
  decision lands, not frigate's YAML. Read `cameras/README.md` fault 13 first: "fix it by editing
  frigate" is explicitly not the fix.
- **`cam1` is offline most of the time, and is now deliberately unmonitored** (measured 2026-11-23
  from gatus's own check log: **3662 of 10913 checks failed over seven days — 33 %** — in 28 runs
  of ≥5 consecutive failures, i.e. 28 Slack down+resolved pairs, interleaved with runs of 386, 683,
  709 and 710 consecutive failures ≈ 11–12 h stretches offline; today alone it dropped eight checks
  in a row between 13:17 and 13:23 then answered in 7 ms). Its gatus row (`Cam 1 - Rack`) was
  therefore **deleted** 2026-11-23 — a page that is right every time and ignored because of how
  often it fires is no signal, and the `flappingSlackAlert` debounce was already doing its job on
  it. **Blocks:** nothing; what was watching it was the only thing watching it, so now nothing is,
  and the device's state is visible only where it is used — `rack_corridor` in frigate and HA.
  **Needs:** on 2026-11-23 nobody wanted a page for this camera, so decide which of the two it is
  — worth fixing (find why it drops for a day: power, its AP's cell per `docs/camera-wifi-cells-2026-10-31.md`,
  or the device itself; `just cam-list` / the controller stats are the verbs) or worth retiring, in
  which case `rack_corridor`'s feed goes with it and fault 13's anonymous RTSP stops mattering.
  Re-verify with `sudo journalctl -u gatus --since "-10 min" | grep -c "endpoint=Cam 1"` (0 after
  the deploy; keep the window short, the pre-deploy entries are still in the journal), and if it
  ever comes back, `ping -c 20 10.3.2.6` first — that loss figure is what would justify the row.
- **thingino on this fleet: answered, twice, and the second answer won.** The 2026-10-25
  measurement said "Ingenic-only, not a route off OpenIPC" (252 device configs, no
  SigmaStar board, platform PR #1424 closed unmerged); the port existed by 2026-10-31 and
  `cam5`/`cam8` ship it (`docs/camera-thingino-migration-2026-10-31.md`). `cam7`/`cam9` are
  the OpenIPC units, and the update they can take is already staged
  (`./camera-sd-card-firmware-flash.md`). **Blocks:** nothing. **Needs:** nothing —
  `cameras/README.md` carries the four commands that re-answer "does firmware X support our
  SoC" for any X, and two of three is not support. The one purchase note that still stands:
  an Ingenic-board camera (Tapo C110 **v3** = T23N) is a thingino device, so read the label
  before buying.
- **A second copy of the camera feed table was wrong in a justfile comment** (found
  2026-10-25 while writing the thingino note): `cam-probe`'s `CAM_FEED` hint said
  `cam5 -> main_space, cam7 -> pi_room, cam9 -> woodwork`, where `cameras/README.md`'s measured
  table says cam5 → `pi_room`, cam7 → `cnc`, cam9 → `social_space` and puts
  `main_space`/`woodwork`/`workshop` on **other devices**, VLAN 227. The comment is now a pointer
  to that table rather than a duplicate of it. **The general form is the finding:** three names
  for one feed (DNS name → go2rtc stream key → frigate camera) with no single table is fault 8,
  and the copy that drifts is always the one written from memory. **Blocks:** nothing — the
  recipe takes `CAM_FEED` from the caller and never trusted its own comment. **Needs:** the fault-8
  table, in one place both `just cam-probe` and the gatus rows can read; until then, treat any
  feed name quoted in prose as suspect and re-read it off `/srv/frigate/config/config.yaml`.

**Opened 2026-10-31 by the thingino migration (`cam5` + `cam8`), `docs/camera-thingino-migration-2026-10-31.md` owns the detail:**

- **`pi_room`'s feed: URL fixed 2026-10-31 19:49, exposure open.** The `/stream=0` 404 is closed -
  `/srv/frigate/config/config.yaml` now asks `rtsp://cam5.int.leighhack.org:554/stream0` (`/ch0` is
  also a 404). What remains: thingino RTSP is **unauthenticated by default**, so the feed is open to
  anything on VLAN 226 until auth is enabled on the camera or a firewall row fences it; and **the
  pre-migration fleet root password still authenticates as root over ssh on both thingino units**,
  while that same value sits in plaintext in a *commented-out* RTSP URL in the config named two
  lines above (measured 2026-10-31; deliberately unquoted, rule 13). **And it is not merely a tidying
  item:** cam8's data partition *was* wiped by the migration flow, and cam8 still takes that password -
  so the credential is baked into the image, which puts it in the same exposure and the same rotation as
  the PMKs in the item below (the fork window, not just the frigate file), and any rotation has to
  re-bake rather than re-key. **Blocks:** nothing running; both exposures are passive. **Needs:** that
  rotation, sequenced with the PSK one below it because both are changeover-sized; a decision on where a
  camera root password belongs afterwards (sops, rendered to `/run/secrets`, never in frigate's runtime
  bind mount); and neither end of this pipe is in the flake (fault 8), which is the concrete case for
  that fix.
- **~~Pin `cam8`~~ - closed 2026-10-31, same evening.** `common/dns-records.nix` reserves
  `10.3.2.11` by MAC `3c:52:a1:49:01:05` (the dynamic `.112` was inside VLAN 226's own pool, so that
  was correctness and not tidying), and frigate and the gatus rows follow the table.
- **cam5 is not RF-marginal - it is on the wrong AP. Corrected 2026-10-31, same evening, and the
  correction is the item.** The -63 dBm above is **AP2's average client signal**, not cam5's: the AP
  sees cam5 at **-47 dBm** and the camera hears that AP at -52. Measured ceiling **61 KB/s up /
  76 KB/s down**, 5-10 % loss and 331 ms mean RTT *to its own gateway*, on a 2.4 GHz BSS that also
  carries a -79 dBm client, where 26 BSSs are audible and one on ch 5 is louder than the serving AP.
  cam8 at -46 dBm on an unshared ch-11 BSS does 1.38 MB/s on byte-identical firmware and encoder
  config. **Blocks:** calling `pi_room` dead, and any camera diagnosis that stops at the camera.
  **Needs:** section 7 of `docs/camera-wifi-cells-2026-10-31.md` (AP2's 2.4 GHz radio, AP4's
  config-versus-measured channel, whether the Pi Room gets a wire or 5 GHz). Re-measure with the
  two-direction `dd` ceiling test in its section 5 - no console needed - and read the AP's retry
  counters with it, because a per-cell average read as a per-camera fact is what started this.
- **~~Re-measure cam8's TX fault under thingino~~ - answered, and the answer is neither candidate.**
  Measured 2026-10-31: the fault is the **cell**, not the driver and not the board. cam8 recovered
  when AP1's failed unit was replaced and the replacement auto-selected channel 11, where cam8 is now
  the only 2.4 GHz client; cam5 carries the identical ceiling on the channel it stayed on. Read that
  with the caveat in section 3 of the new doc - cam5 measured *healthy* on that same channel on
  2026-10-29, so it is the cell's load and neighbours that are fatal, not the channel number. The tell
  the 2026-10-29 write-up never took is that **both directions are capped** (a weak transmit chain is
  asymmetric), while the AP retries 50 % of its own frames to a -47 dBm client. That doc is corrected
  at its headline, and **the 2024-vs-2026 `ssw101b_wifi_usb` module swap it proposes must not be
  run**. Receipts: `docs/camera-wifi-cells-2026-10-31.md`.
- **The `cam-*` verbs assume OpenIPC `/etc`** (`majestic.yaml`, `/tmp/webui/signature.txt`,
  the env credential mask): `cam-check`/`cam-pull`/`cam-diff` on cam5/cam8 will report
  misleading absence until `inventory.sh` grows a thingino mode (raptor conf,
  `fw_printenv bootcmd` for the rescue armament, `/overlay/root/etc` upper-listing — the
  one command that would have saved two hours on cam5, migration doc §3).
- **Decide cam7 + cam9.** Both still OpenIPC, both with open faults of their own (cam7's
  unauthenticated admin plane, cam9's disputed sensor identity); if they follow, use the
  **network two-phase flow** (it wipes the data partition by construction) and confirm the
  target **MAC** from the fleet table before writing — today's near-miss on addresses was
  doc rot in the bench tree, and the table is the authority.
  *2026-10-31 update: the flow is a verb now — `MIG_PASS=… GO=1 just cam-migrate <cam> all`,
  gated by `just cam-fw-verify` and the migration script's rehearsed-hash check
  (`docs/camera-thingino-build-deploy-2026-10-31.md`).*
- **The thingino port is on GitHub now — the open item moved from *push it* to *rotate
  the PSKs it leaked*** (2026-10-31: SSOT `cjdell/thingino-firmware`, private; the
  branch + the flashed artifacts as release assets + the bench docs bundle + the
  `c110-1.bin` stock dump; the build verified from a fresh clone). The overlay commits
  the raw PMKs for Leighhack and The Lab 2.4GHz, and for ~half an hour the port was a
  **fork**, whose commits are readable anonymously through the public parent repo even
  after the fork goes private — the exposure window is what forces rotation, not just
  discretion. **Blocks:** nothing running; Leighhack and The Lab PSKs and the baked
  credentials on every thingino camera are all in scope. **Needs:** rotate both PSKs,
  re-bake the overlay (a changeover-sized event — credentials are baked), keep the repo
  private until then; the full story and the fork-network trap:
  `./camera-thingino-build-deploy-2026-10-31.md` §7. **Deleting the fork did not
  revoke the exposure** — anonymous by-SHA reads through the public parent were still
  serving the PMK blob an hour later (fork-network object purge is a GitHub support
  action; deleted repos stay restorable for 90 days). **Also needs:** the support
  request, and until it lands treat this as a completed leak, not a contained one.
  Still-open upstream half: the
  patch drafts the bench tree carried in `docs/upstream/` (inside the release's docs
  bundle now) were never submitted.
- **cam5 is off the air and needs a hand, not a deploy.** A BSSID pin written into its
  `/etc/wpa_supplicant.conf` over ssh to test the conclusion above: the ssw101b client will not join
  a pinned BSS and will not fall back to the AP it was using, the detached self-revert died with the
  ssh session (dropbear tears down the process group), and the bad file lives in the **persistent**
  overlay - so a power cycle does not undo it. **Blocks:** the `pi_room` feed (which was running at
  1.3 fps before the test). **Needs:** either the console fix
  (`rm /overlay/root/etc/wpa_supplicant.conf` then `kill -HUP $(pidof wpa_supplicant)`), or - with no
  serial rig to hand - a FAT SD card carrying `uenv.txt` (`wlan_ssid=`, `wlan_pass=`), because
  `S38wpa_supplicant`'s `credentials_from_card()` hook rewrites the whole config at boot.
  **`wlan reset` does not work on this build** (it removes `/overlay/etc/...` and the upper dir here
  is `/overlay/root`, so it deletes nothing and exits 0). If the payload rather than the credentials is
  the suspect, `just cam-rescue-card /dev/sdX` is the verb that writes the one-shot reflash card
  (`docs/camera-thingino-build-deploy-2026-10-31.md`) - heavier than a credentials card and it should
  not be needed for this. Runbook, why `/rom` makes the `rm` sufficient, why a power cycle does not, and
  the rule (never change an association over the association): section 8 of
  `docs/camera-wifi-cells-2026-10-31.md`.
- **`just cam-*` fails on cam5 for a second, unrelated reason:** the operator account's
  `known_hosts` still holds the pre-reflash OpenIPC host key for `10.3.2.10`, so every recipe dies
  at `Host key verification failed`. **Needs:** `ssh-keygen -R 10.3.2.10` and a *human* accepting the
  reflashed key - an agent pinning a new host identity is not TOFU, which is why this is an item and
  not a done thing. (The hop key itself is re-installed on both thingino units as of 2026-10-31;
  cam5's `authorized_keys` also holds three operator keys, which is the re-check the migration doc
  asked for.) **Re-measured 2026-11-04: `10.3.2.11` (cam8) is in the same state** — `ssh` to either
  thingino unit dies at `Host key verification failed`, offending entries at `~/.ssh/known_hosts:51`
  and `:54`, so nothing in this repo can currently read either unit, including its running payload
  hashes (the next item). Whatever re-keys them, `cam-*` and that read are both closed by it.
- **The thingino pin moved out from under this repo, and its hash table is now a statement
  about a release rather than about the fleet** (measured 2026-11-04, rule 30). Two separate things, both verified from a fresh
  clone of the SSOT: **(a)** recreating that repo as a standalone (the fork-network fix, the
  build-deploy doc §7) **replayed the branch**, so the pinned commit `1286efbd3` is on no branch
  at all — `git merge-base --is-ancestor c110-fleet-20261031 ssc333-infinity6b0` exits 1, and its
  tree survives as branch commit `46602f946` — so `git clone -b ssc333-infinity6b0` gets a head
  the build gate refuses, and the range `c110-fleet-20261031..ssc333-infinity6b0` counts **30
  commits, 10 of them genuinely new** (the speaker/audio bring-up and the `prudyntctl` shim, the
  two night-latch fixes, the Wi-Fi reconnect watchdog, `docs/cameras/tapo-c110.md` itself, the
  repack/container-trap docs, the patch re-sign; the other 20 are replays of work already in the
  tag's lineage, since the re-creation rewrote every SHA and `A..B` compares reachability, not
  content); **(b)** that guide's outcome log records a payload
  pushed to `cam5` over ssh on **2026-11-02** whose `uImage`/`rootfs.squashfs` md5s are **not**
  `cameras/thingino-manifest.sh`'s — a deploy path that never touched this repo, so the manifest
  and §3 of the build-deploy doc describe the bytes of record and not any unit. Nothing is
  broken: the units boot, frigate is fed, `cam-fw-verify` still guards the staged files. **This
  is the drift the pin exists to catch, caught in the other direction.** **Blocks:** believing
  `cameras/thingino-manifest.sh` about "what cam5/cam8 run", and re-keying the units (previous
  item). **Needs:** a human to read the running hashes off both thingino units and then either
  move the manifest + `FW_COMMIT` to what they run (one commit, narrative line included —
  build-deploy §5) or record deliberately that this repo pins the release and the fleet drifts
  from it; and decide whether payload pushes from the firmware repo are the way this is going to
  work, because if they are, the manifest should say so instead of pretending to be the fleet.
  Re-verify with `git -C ~/Projects/thingino-firmware log --oneline
  c110-fleet-20261031..ssc333-infinity6b0`, `just cam-fw-verify`, and
  `gh release view c110-fleet-20261031 --repo cjdell/thingino-firmware`.
- **The wireless access layer has no owner in this repo and is now the camera fleet's binding
  constraint.** Four APs, one SSID, three channels, `auto` on three of them and a *pinned* channel on
  AP4 that disagrees with the frequency its own BSSID is heard on (config `6`, measured 2447 = ch 8);
  which radio a camera joins is invisible to the flake and unpinable from it. **Blocks:** every
  "camera X is bad" report until the cell has been read. **Needs:** someone to decide the 2.4 GHz
  plan in the controller UI and record it here - the AP/BSSID map and the measurement recipe are
  sections 4 and 5 of `docs/camera-wifi-cells-2026-10-31.md` - plus a decision on whether the Pi Room
  camera stays on Wi-Fi at all.

- **The thingino units' RTSP credential is thingino's factory default, and frigate now depends on
  it** (found 2026-11-11 while fixing the "Stream Offline" pair). Both `cam5` and `cam8` answer
  RTSP `401` with `WWW-Authenticate: Digest realm="Raptor"`, user `thingino`, password = `[rtsp]
  password` in each unit's `/etc/raptor.conf` — read off both, **identical on both, and not one we
  set**. The `root` login on the same boxes is a different credential, so the fleet no longer has
  one "camera password". **Blocks:** nothing; the exposure is that VLAN 226 (and anything the door
  can be tricked into) can pull both pictures with a default credential, and the value is written
  in `/srv/frigate/config/config.yaml`, the file fault 8 says nobody tracks. **Needs:** a real
  credential on both units and in that config in the same change — or, better, fold it into fault
  11 (camera RTSP credentials into a sops-rendered env file rather than a bind-mounted YAML) and
  do all of them once. Note `[rtsp] max_clients = 2` while you are in there: go2rtc plus one direct
  viewer fills it, so direct viewers belong behind the restream.
  Re-verify with `ssh -i ~/.ssh/agent-hop-key root@10.3.2.10 'awk "/^\[rtsp\]/{f=1} /^\[/&&!/^\[rtsp\]/{f=0} f" /etc/raptor.conf | grep -c password'` and go2rtc's `/api/streams`.
- **go2rtc cannot authenticate to thingino 2's RTSP server, so both feeds run on the `ffmpeg:`
  workaround** (2026-11-11). The server's digest challenge carries **no `qop`**; frigate's go2rtc
  1.9.14 answers its own attempt with `streams: wrong response on DESCRIBE` and resets, while
  `curl --digest` and the container's ffmpeg 7.0 *and* 8.0 complete the same handshake against the
  same camera. Both feeds are therefore `ffmpeg:rtsp://thingino:…@camN.int…:554/ch0`, which costs a
  remux process per feed (`-c copy`) and buys nothing else. **Blocks:** any future attempt to
  "simplify" those two lines back to a plain `rtsp://` source — it will look like the camera is
  broken and be the client. **Needs:** either a go2rtc version that does `qop`-less digest (test
  before believing: the failure is a `WRN [rtsp]` line, not a 401), or the cameras configured to
  offer Basic too. **Also needs:** `just cam-probe` should stop presenting a `200` from
  `curl -w '%{http_code}'` as a working feed path — measured 2026-11-11, it printed `200` for
  `/stream0` on a build where ffmpeg got `404 Unknown stream endpoint` for that exact URL; the
  recipe's own wording invites the wrong conclusion.
- **`cameras/thingino-manifest.sh` is now two payloads behind the fleet** (2026-11-11; extends the
  2026-11-04 item above, which was written when it was one behind). Both thingino units run
  **thingino 2 (Figata)**, kernel built `2026-11-11 14:34 UTC`, and this repo still pins
  `1286efbd3` / tag `c110-fleet-20261031` with that image's five artifact hashes — so
  `just cam-fw-build` and `just cam-fw-verify` refuse, by design, the very firmware the cameras run,
  and `cam-fw-fetch` would restage the wrong bytes if anyone ran it. The new build's commit and
  hashes exist only on the bench: there is no `~/Projects/thingino-firmware` tree on services1 **or**
  aibox (checked 2026-11-11). **Blocks:** believing the manifest, and any future `cam-rescue-card`
  (it writes verified-but-wrong images). **Needs:** the same decision the 2026-11-04 item asks for,
  now with two more payloads to account for — move `FW_COMMIT` + the five hashes to what the units
  run, or record on purpose that this repo pins a release and the fleet drifts from it. Re-verify
  with `just cam-fw-verify` against the staged tree and `uname -a` + `/etc/os-release` over the hop
  key on 10.3.2.10.
- **Fault 8 cost 2 h 35 min of two dead cameras on 2026-11-11.** The outage was invisible to every
  check this repo owns: no flake file names a feed URL, the two lines that had to change live in
  `/srv/frigate/config/config.yaml`, and `cam-probe` cannot check the go2rtc end without `CAM_FEED`
  set by hand. Nothing detected it for the network either — gatus watches `cam5.int`/`cam9.int`
  (HTTP), which answered `200` the whole time, so "camera up" and "camera recording nothing" were
  indistinguishable from the monitoring side. **Blocks:** nothing; it is a repeat-cost estimate.
  **Needs:** fault 8's fix (track that file, and the three-name table, in the flake) plus a gatus
  row that asks frigate `/api/stats` for `camera_fps > 0` per camera rather than asking the device
  for HTTP.
- **`cam5`'s thingino 2 audio track emits bad timestamps, so its frigate feed is video-only as a
  workaround** (2026-11-11, found while fixing the same outage — the feed was still dead after auth
  and path were right). `[audio] codec = l16` / `enabled = true` in `/etc/raptor.conf`, and the PCM
  stream's DTS is not monotonic: ffmpeg says `non monotonically increasing dts to muxer in stream 1`
  all the way down and go2rtc's `ffmpeg:` source fails with `Invalid data found when processing
  input`. `-an` is clean; `/ch1` is as broken as `/ch0`; **`cam8` with the byte-identical config
  file is clean**, so it is the unit, not the payload. `pi_room` therefore has no audio in frigate
  until this is fixed properly. **Blocks:** audio detection/recorded audio on `pi_room`, and any
  assumption that the two thingino units are interchangeable. **Needs:** `[audio] enabled = false`
  in cam5's `/etc/raptor.conf` — whole-file replace + md5 verify + `/etc/init.d/S31raptor restart`,
  backed up first, the operation `cameras/README.md` rule 2 prescribes — **and** the same change in
  that board's profile in the firmware repo, because the next payload will re-enable it; then drop
  `#video=h264` from the `pi_room` line in `/srv/frigate/config/config.yaml` and re-enable audio in
  frigate if the fleet wants it. Re-verify with the container's own ffmpeg: `-t 5 -c copy -f null -`
  against `rtsp://…@cam5.int…/ch0` is silent when the track is gone.

## UPS and the power-cut path (gw) — opened 2026-10-28, `docs/ups-and-fleet-shutdown.md` owns it

Deployed 2026-10-28 and drilled the same evening: a Back-UPS XS 1400U on `gw`'s USB
port, NUT (`machines/gw/ups.nix`), a 300 s `upssched` grace timer, notices that
`wall` every server, and `crates/ups-watchdog` powering
`services1`/`services2`/`aibox` off over the hop key before the router powers
itself off. The notice path has now crossed ssh for real (`just ups-drill-notify`);
the shutdown path has run only as a drill. **None of it has been through a real
outage**, so the items below are ordered by "what would embarrass us at 04:00", not
by size.

- **The UPS is not on `gw`'s USB bus as of 2026-11-22, and NUT on the router is
  therefore blind — measured, not inferred.** After `gw`'s switch-and-reboot of that
  day (kernel `6.18.56` → `6.18.63`), `upsdrv.service` fails at boot with
  `No matching HID UPS found` and `upsc apc` answers `Driver not connected`; a retry
  an hour later fails the same way, so it is not a boot race. `sudo lsusb` on `gw`
  lists eight devices — the RT5370 Wi-Fi, the Realtek gigabit adapter, two Intel
  rate-matching hubs, an ATEN and a Genesys hub — and no UPS, while
  `/etc/nut/ups.conf`'s `apc` row still carries the desc that named it:
  `APC Back-UPS XS 1400U on usb 2-1.2 (serial 4B1836P34201)`. Everything downstream
  of the read is dark: `upssched`'s 300 s countdown, the `ONBATT` notices, and
  `crates/ups-watchdog`'s decision all hang off a driver that has nothing to talk
  to. Same boot also failed `ups-notice.service` with three recipients unreachable
  (`services2: ssh exited 255`, `aibox: Network is unreachable`) — the notice unit ran
  while `gw`'s own network was still flapping, which is a second thing to know about
  that unit rather than the first. **Blocks:** claiming any power-cut behaviour works
  today. **Do:** check physically whether the UPS is plugged into `gw` at all (a
  moved cable, a bench, a dead UPS-side port), and `sudo systemctl restart upsdrv`
  plus `upsc apc` after whatever is done — re-verify with `sudo lsusb | grep -i`
  <the UPS vendor id> and `upsc -l`. Note `sudo systemctl reset-failed` was run on
  `gw` that day to clear transient gate units, which also hides these two units from
  `systemctl list-units --failed`; `systemctl is-failed upsdrv` and the journal are
  the truth, not that list.

  **Closed on 2026-11-25 at the box, and the closing found a second fact the next
  `gw` deploy has to survive.** The UPS is back on `gw`'s USB bus: `sudo lsusb | grep -i
  051d` answers `Bus 002 Device 006: ID 051d:0002 American Power Conversion
  Uninterruptible Power Supply`, and the kernel's line for that day reads
  `hid-generic 0003:051D:0002.0003: hiddev96,hidraw2: USB HID v1.10 Device [American
  Power Conversion Back-UPS XS 1400U …]` — the endpoint is `hidraw2` now, not the
  `hidraw4` `machines/gw/ups.nix`'s header names, because **the hidraw number follows
  the bus order and so is a fact about what else is plugged into that hub** (the Realtek
  gigabit adapter and the RT5370 that serve the WAN backup), never about the UPS. `sudo
  systemctl start upsdrv.service` then answers `Result=success`, `is-active active`, and
  `upsc -l` prints `apc`, so `upssched`'s countdown, the `ONBATT` notices and
  `ups-watchdog`'s decision all have a driver to talk to again.

  **The second fact: `upsdrv` failing *during an activation* is a race, not a fault, and
  it makes a `gw` deploy exit 4 with the router's rollback countdown armed.** Today's
  `just deploy gw` printed `switching to system configuration /nix/store/…-nixos-system-gw…
  failed (status 4)` at **16:46:33** with `upsdrvctl … No matching HID UPS found`, while
  the kernel printed the UPS *arriving* at **16:48:29** — two minutes later. The WAN
  backup's USB handling (`crates/wan-manager` and `uhubd` own that bus, docs/wan-manager-2026-11-09.md)
  takes the link and puts it back, so the driver's start loses that window some times. The
  consequence is rule 4's shape and the recipe cannot see it: `/run/current-system` moved to
  the new generation, the activation died at the end, the recipe exited 4 so the GOLDEN RULE's
  confirm never ran, `system-good` stayed on the *previous* generation, and
  `auto-rollback.timer` was `active` — a five-minute countdown to rebooting the network's
  router. What was done inside the window, and what should be done every time: read the
  failing unit's **name** first (`upsdrv`, a unit that is racing for a device, is not
  evidence the configuration is wrong), read `readlink -f /run/current-system
  /nix/var/nix/profiles/system-good`, `sudo nixos-confirm`, restart `upsdrv.service`
  after the device rejoins, and only then decide whether anything else needs fixing. Never
  `systemctl list --failed` as proof — the item above shows `reset-failed` hides these two
  units. **Blocks:** nothing that is running. **Re-verify with:** `sudo journalctl -u upsdrv
  --since "-1 h"` read *beside* `sudo journalctl -k | grep "New USB device found, idVendor=051d"`
  — the two timestamps are the whole story — plus `upsc -l` and `sudo lsusb | grep -i 051d`.
- **Does this firmware return its load after dropping it?** (armed 2026-10-28 —
  `allow_killpower = yes` in `machines/gw/ups.nix`, after measuring that without it
  NUT 2.8 silently ignores the shutdown request: `driver.flag.allow_killpower does
  not permit this - request was currently ignored!`, verbatim from the driver. So
  the cut happens now; **the return is the unmeasured half**.) If the UPS drops the
  outlets and keeps them down until someone presses its front button, an unattended
  recovery becomes one button instead of four — better, but still a walk, and worse
  than before for anything on those sockets that this flake does not manage.
  **Blocks:** claiming the network recovers unattended. **Do not** probe with
  instant commands on a UPS carrying the router. **Needs:** one controlled discharge
  **with a spare box on the UPS as the sacrificial load**, watching
  `journalctl -u ups-killpower` for the cut and then whether the outlets come back
  by themselves when mains are restored — and do it knowing the sequence at the end
  of that discharge really does power the fleet off.
- **Autostart after AC is stated, not measured.** The operator's fact: these boxes
  power on when AC is supplied following an interruption. §5's whole mechanism rests
  on it, so it is recorded rather than re-derived — but nobody has read the BIOS of
  `gw` itself, and the two boxes that matter most for a dark building are outside
  this feature (next item). **Needs:** at the next physical visit, the setting on
  `gw` and whichever of the NAS/monster1 turn out to be on this UPS.
- **The NAS and `monster1` are not in the shutdown list, and it is not known
  whether they are even on this UPS.** `nas` is TrueNAS — not a flake machine, its
  own shutdown verb — and monster1 hosts VMs this flake does not own; the second
  `docs/monster-powercut-runbook.md` is the runbook for *that* box being down, not
  for a plan. **Blocks:** claiming the network survives an outage cleanly: today
  the router can go while the NAS is still serving NFS. **Needs:** which sockets
  the NAS and monster1 are actually plugged into, then either their shutdown in
  the right place in `shutdownTargets` (NAS after `services1`, before the router)
  or a sentence saying why they are on a different UPS.
- **`battery.runtime: 860` is a manufacturer's number, not a measurement — and the
  pack is eight years old.** (measured 2026-10-28 at `battery.charge: 100`, with
  `battery.runtime.low: 120` and `battery.charge.low: 10`; `battery.mfr.date` is
  **2018-09-08** and `battery.date` is the nonsense `2001-09-25`.) The 300 s grace is
  about half the claimed runtime, which is defensible arithmetic; what is not
  defensible is the 860. **Needs:** one timed discharge with the real load on it,
  the grace period restated as `runtime − (the sequence + margin)` rather than a
  round number, and probably a replacement pack (which also re-opens the `OB` →
  `LOWBATT` timing the whole policy is sized around).
- **`upssched`'s own bookkeeping goes nowhere.** It logs to syslog facility
  `daemon`, which this box does not collect, so `Timer daemon started` and
  `Cancelling timer: …` are absent from the journal: you cannot tell from the
  journal whether a timer was armed or cancelled. `pgrep -a upssched` is the
  workaround (the daemon self-starts and exits when its queue empties) and
  `just ups-events` prints it. **Blocks:** reading an outage after the fact. **Do
  not** make alerting a dependency of the shutdown path. **Needs:** either
  `upssched -D` in a debug run during a drill, or a decision on collecting NUT's
  syslog; and if it is wanted, a Grafana/Alertmanager rule on the NUT metrics or a
  gatus endpoint (`common/monitoring-endpoints.nix`) — not a webhook embedded in
  `ups-fleet-shutdown.service`.
- **Nobody who was not at a keyboard has seen one of these notices.** The notices
  are `wall`, so an unattended building reads none of them — the 2026-10-28 drill
  proved `ssh` + `wall` returns 0 on three boxes, not that anyone read it. **Needs:**
  the next drill to be announced and acknowledged, and a sentence here saying a
  person saw one on a terminal.

**Closed by the 2026-10-28 deploy**, kept here so the questions are not re-asked:
`upsc` over loopback needs no credentials (the nixpkgs module writes no `ACCEPT`
line and the units' own read of `ups.status` returned `OL`; `upsd` binds
`127.0.0.1:3493`/`::1` only, which the module's assertions insist on); a second
`ups-notice` in one boot does fire (`Type=oneshot`, no `RemainAfterExit`), so every
outage of a boot gets its broadcast; and `usbhid-ups` on `port = auto` finds this
one UPS cleanly and reports it as an APC HID unit.

## Frigate detection quality (services1) — opened 2026-11-12, `docs/frigate-false-positives-2026-11-12.md` owns it

- **The measured fix for Frigate's `person` false positives is two `min_area` lines, and they
  are not applied** (opened 2026-11-12, `machines/services1/services/frigate.nix`,
  `docs/frigate-false-positives-2026-11-12.md` §7). Box **height** in detect pixels separates
  the two populations almost cleanly: a floor at 150 px drops **66.5 % of never-moving `person`
  tracks and 3.9 % of moving ones**, and Frigate expresses that only as an area, so the
  measured per-camera cuts are `main_space: 22932` (`0.00276` of frame, 74.9 % / 6.9 %) and
  `workshop: 472236` (`0.05693`, 74.7 % / 6.5 %) at each cohort's p75. The same table says
  **do not** do it on `cam8`, `woodwork` or `rack_corridor` (cost 22–51 % of moving detections
  — the populations overlap there). **Blocks:** nothing; it is detection-quality debt —
  18.4 % of `person` events and 20.5 % of `person` event-hours network-wide, main_space
  25.8 % / 24.2 %, are tracks that never move a fiftieth of the frame width. **To do:** add
  `objects.filters.person.min_area` to those two cameras in `/srv/frigate/config/config.yaml`
  — which is a runtime bind mount, so the edit is **out-of-band and the flake will not notice
  it** (see the 2026-10-22 item "frigate's camera config is not in the repo and is deciding to
  do nothing") — restart `podman-frigate`, read the values back from `/api/config` rather than
  evaluating anything (rule 21), and re-measure after seven days with the query in §9 of the
  doc. Both of the operator-confirmed false positives fall under these floors: 82 px and
  147 px tall. Re-verify with:
  `curl -s http://127.0.0.1:5000/api/config | jq '.cameras.main_space.objects.filters.person'`.

- **`stationary.max_frames` was recommended on 2026-11-11 and withdrawn on 2026-11-12 — do not
  re-recommend it from the old numbers** (opened 2026-11-12,
  `docs/frigate-false-positives-2026-11-12.md` §7). The expiry is
  `motionless_count - detect.stationary.threshold > max_frames` (`track/centroid_tracker.py:95-106`,
  default unset = never), and `250` frames at 5 detect-fps ≈ 50 s of stillness measured as
  704 events (4.8 % of all, 25.9 % of never-moving) and 163.5 hours (19.5 % of hours) — with
  36.4 % of *moving* events longer than the same cutoff. The reason it is withdrawn is that the
  never-moving cohort is not all junk: labelling crops from it produced "a person sitting on a
  bench" and an 11.7-hour track that is a real person per the detector's own account, so
  expiring stillness at 50 s risks suppressing people who sit still, bought with hours nobody
  asked to keep. **Blocks:** nothing. **To do:** only revisit if the size filter lands and
  leaves a residue, and then only with labelled crops. Re-verify with: label 20 crops from the
  never-moving cohort first; the proxy is not the ground truth.

- **The GPU question for Frigate is closed on this box and re-opens only with a Turing or newer
  card** (opened 2026-11-12, `docs/frigate-false-positives-2026-11-12.md` §3–§4). Recorded so it
  is not re-derived: the Coral is at ~37 % of its ceiling so throughput is not the cause of any
  false positive; the iGPU matches the Coral at 300² (8.2 ms vs 8.87 ms) but cannot run a 640²
  model (34.8 ms → ~28 inf/s against ~41 inf/s of demand); and **no detector plugin in Frigate
  0.18.2 can address sm_61**, so the `services3` GTX 1080 Ti — 8 GB free of VRAM, plenty of
  FP32 — can only be a sidecar, through `type: zmq` (`tcp://` endpoints work; the request is
  `[header_json, tensor_bytes]`, the reply `[20,6] float32`, `request_timeout_ms` defaults to
  200) or `type: deepstack` (whose own docstring says *Not recommended*, 0.1 s timeout). Both
  **return a zero array on timeout**, so a dead sidecar is indistinguishable from a quiet
  building. **Blocks:** nothing. **To do:** if a bigger primary model is ever wanted, buy the
  Turing-or-newer card and use upstream's `tensorrt` path rather than writing the ncnn+`zmq`
  server this network would then own and feed, on a card whose driver is `legacy_580` — the last
  branch with Pascal, verified present only at this flake's nixpkgs pin. Re-verify with:
  `ls /opt/frigate/frigate/detectors/plugins/` in the container, and
  `nix-instantiate --eval -E '(import <nixpkgs>{}).linuxPackages.nvidiaPackages.legacy_580.version'`.

- **A vision-LLM verifier is possible on `services3` with no new code, and its *verdict* is not
  usable — only its self-consistency is** (opened 2026-11-12,
  `docs/frigate-false-positives-2026-11-12.md` §6). `Qwen2.5-VL-7B-Instruct` Q4_K_M + mmproj
  (5.6 GB) ran on the installed `llama-cpp-0.9.0` (`bin/llama-mtmd-cli` proves mtmd is compiled
  in) at 9,445 / 11,264 MiB **alongside** production LFM3, at a median 1.62 s/image. Across two
  rounds the verdict flipped with the order of two output lines (round 1: verdict-first said
  PERSON 7/7, description-first 4/7; round 2: verdict-first NOT_PERSON 6/8, description-first 7/8
  PERSON), and it offered `a photo of a person` nine times including on moving humans. What
  separated the populations was **disagreement between the two orders**: 7/8 of never-moving
  crops vs 3/15 of moving ones, so "both orders agree it is a person" is 92 % precise and
  escalates 7/8 of the junk — a triage ranker, not a filter. **Blocks:** nothing. **To do:**
  nothing unless §7's filter proves insufficient; if it is, round 3 is one download, because the
  pinned `llama-cpp-upstream` (`7bcbc04`) already has `QWEN3VL` in `tools/mtmd/mtmd.cpp` and
  official `Qwen3-VL-2B/4B-Instruct-GGUF` repos exist. Note two hard constraints learned the
  expensive way: **retention makes verification retrospective- impossible** (the same
  `snapshot.jpg` URL answered 200 on 2026-11-11 and 404 on 2026-11-12), and a 2193×2160 crop
  costs 17.9 s of vision prefill (~214 tok/s, against text prefill at 1,618) — cap inputs at
  ~1024 px. The 5.6 GB of model files are parked at `services3:~/vlm-eval/`, off-flake, on the
  box with the least free disk. Re-verify with: `ls $(dirname $(readlink /proc/$(systemctl show
  llama-server -p MainPID --value)/exe)) | grep mtmd`, and the two prompt-order scripts in §6.

- **Three Frigate API/MQTT facts that invert an obvious design** (opened 2026-11-12,
  `docs/frigate-false-positives-2026-11-12.md` §5). `PUT /api/events/{id}/false_positive` is
  **not a local flag** — it is the Frigate+ submission endpoint, 400s without `PLUS_API_KEY`, and
  it uploads the crop off-box; the local surfaces are `POST /api/events/{id}/sub_label`,
  `PUT /api/events/{id}/end` and the review routes `set_multiple_reviewed` / `set_not_reviewed`,
  all `require_role(["admin"])`. `review.genai` writes descriptions and sub-labels only — severity
  is computed by `review/maintainer.py` from config, so **GenAI cannot demote an alert**.
  And `average_estimated_speed` is 0.0 in every event not because the feature is broken but
  because it is only computed inside a zone with `distances` (`track/tracked_object.py:242,250`) —
  so a speed gate is available, not dead. **Blocks:** nothing; it blocks *designs*. **To do:**
  nothing; write any future alert-triage integration against the local routes and zones.
  Re-verify with: `grep -n "false_positive\|sub_label" /opt/frigate/frigate/api/event.py` and
  `grep -n "distances" /opt/frigate/frigate/track/tracked_object.py` in the container.

## pi's inference endpoints (every box) — opened 2026-11-14, `docs/pi-agent-models.md` owns it

- **Rotate `strata_api_key`.** Until 2026-11-14, `~/.pi/agent/models.json` on **gw,
  services1, services2 and aibox** dialled the engines' addresses directly
  (`http://10.3.1.21:8080/v1`, `http://10.3.1.32:8080/v1`) and so held the engine's real
  Bearer key in a home directory on four machines — four copies of a credential that the
  flake otherwise keeps in exactly one place precisely so the door can inject it. The
  `programs.pi-agent` rewrite removed all four (the row in `common/pi-endpoints.nix` is the
  front-door URL and `apiKey` is a placeholder; a kept-stale provider rebuilds its shape from
  the config, so an off endpoint cannot preserve the old key either). **Blocks:** nothing
  technical — it is a decision with a blast radius: the key is read by `common/nginx/front-door.nix`
  (`strataBearer`) and by both engines' own generated run configs (`machines/services2/strata.nix`,
  `machines/aibox/strata.nix`), and those two engines must keep one key because the door
  renders a single `proxy_set_header Authorization`. So it is: re-issue in sops, switch
  `gw` and both engine boxes in the same pass, then re-run `just pi-agent-sync` on a client and
  `curl -fsS -o /dev/null -w '%{http_code}' https://strata2.int.leighhack.org/v1/models` (200
  proves the door has the new one). Never print it (rule 13). Re-verify the exposure is gone
  with: `grep -c strata_api_key ~/.pi/agent/models.json` on each box — 0.

- **Nothing watches the timer.** `pi-agent-<user>.timer` re-runs the discovery every 15
  minutes and its status file carries `lastSuccessUtc`, but no health check reads it, so
  "the model list is three days old because the timer is dead" is indistinguishable from a
  quiet config. `machines/gw/lease-reader.nix`'s `lease-reader-watch.timer` is the precedent
  for a unit that goes red when its channel dies. **Blocks:** a decision about who is paged
  for a stale model list (nobody has asked to be), and a place to put it — Prometheus scrapes
  the boxes that matter and gatus checks endpoints, not local files. **To do:** export
  `pi_agent_last_success_seconds` from somewhere the monitoring already reaches, or a
  `pi-agent-watch.timer` that fails past N hours, whichever the owner prefers.

- **`strata-services2` answered late, not never.** First probe of 2026-11-14: the engine was
  `inactive` and the door 502s, so the run kept the previous list (`ctxSource: "previous run"`)
  — the designed behaviour, exercised for real within minutes of the change landing. At 15:09
  the same day it answered `meta.n_ctx` **131072**, which is what the hand-written row had
  always claimed: that one number was right, and the two that were wrong were aibox's (131072
  in the file, 262144 live) and zen3's model id. Nothing to fix here — the value of the item is
  the contrast: two engines, same model family, 262144 and 131072, and only the endpoint knows
  which is which. **Blocks:** nothing. **To do:** nothing; see the table in
  `docs/pi-agent-models.md`.

- **Renaming a provider id leaves a provider behind on every box, and `pi-agent-status` says
  so as `not managed here`** (measured 2026-11-22, the day `llama-services3` became
  `strata-services3`). `~/.pi/agent/models.json` on gw now holds **both**: the live
  `strata-services3` row, and `llama-services3` — whose endpoint no longer resolves — sitting
  in the file because `pi-agent-sync`'s rule is that only what it can prove it wrote may be
  removed, and after a rename the previous id is no longer in its own provenance list, so it
  is foreign and preserved. That is the rule working as designed (it is the same rule that
  stops it mangling a hand-edited file), but the consequence is that pi lists a model nobody
  can dial, on every box, until someone notices. The same run showed the **other** half of
  the rule doing its job: on aibox, one timer interval after the door took the name away,
  `pi-agent-status` read `llama-services3 FAILED … Could not resolve host` with
  `kept from last time: llama-services3` — the previous answer kept while the socket is dead,
  which is rule 31's deliberate choice, not a bug — and `just pi-agent-sync` closed it in
  seconds. **Blocks:** nothing; pi's default provider on the boxes checked is
  `strata-services2`, so no session lands there by accident. **To do,
  one of:** (a) teach `pi-agent-sync` to reap an id that is absent from the table **when the
  status file from the previous run lists it as managed** — the provenance to make that sound
  already exists, it is just keyed to the new ids only; or (b) declare the retired ids and
  delete exactly those, with the reason in the code. Not (c), hand-deleting on five boxes:
  that is the habit this file exists to replace. **Re-verify with:**
  `just pi-agent-status` on any box (look at the `not managed here:` line), and
  `grep -c '"llama-services3"' ~/.pi/agent/models.json` (1 on gw today, 0 is the done-state).

- **`strata-services3`'s `compat` block is inherited from `strata-services2` and has never
  been measured on the Pascal box** (the row was `llama-services3`, with an *empty* `compat`,
  until services3 replaced llama.cpp with Strata on 2026-11-21; the block arrived with the
  merge of that work). pi's guidance is to describe verified differences rather than enable
  them because an endpoint advertises OpenAI, so this row currently makes eight claims about
  a card no arm of services2's tuning ever ran — same engine binary and same Coder IQ1_M pack,
  which is a strong reason to expect them to hold and is not a measurement. **Blocks:**
  nothing but truthfulness. **To do:** one image session and one long tool-call session
  through `strata3.int.leighhack.org` on this endpoint, then either keep the block and record
  that it held here too, or delete the fields that did not and say which. Re-verify with:
  `nix eval --impure -f common/pi-endpoints.nix strata-services3.compat` (that table is DATA,
  so it reads the declaration without evaluating a machine — the shape `just check-eval`
  already uses),
  `just pi-agent-status` on any box, and the two sessions themselves.

- **`ai.int.leighhack.org` / `ai.leighhack.org` answer 502 by design, and nothing in this
  repo can tell that from an outage** (found 2026-11-15 while verifying the fleet after a
  whole-network repin: the post-run endpoint sweep showed a 502 with no baseline to diff
  against, so it read as damage from the run and cost an investigation it did not need).
  `machines/aibox/strata.nix` clears `llama-server`'s `wantedBy` (`displaceLlamaServer`)
  because llama-server's ~33 GB of GTT and strata's 35.5 GiB arena cannot be resident at
  once, and its own comment states the consequence: those two vhosts "502 while strata has
  the box". `llama-server.service` on `aibox` is `linked`, `inactive`, last stopped
  **2026-10-28** — so the door has been proxying `10.3.1.32:8081` to nothing for weeks,
  legitimately. Two things are wrong with that, and neither is the 502: **(a)** neither
  name is in `common/monitoring-endpoints.nix`, so no gate sees the row go red — same class
  as the `llama.int` gap above, but here the endpoint is *expected* to be down, which is a
  fact no check can currently express; **(b)** `settings.json` still carries
  `llamaServerUrl = "https://ai.int.leighhack.org"` (the item above), so pi's
  router-integration provider dials a known-dead endpoint on every box that keeps it.
  **Blocks:** nothing; `programs.pi-agent`'s `strata-services3` provider is the live path.
  **To do:** pick one — add a check whose expectation is stated (`up` iff strata is
  displaced), or make the door answer those two names `503` with the reason rather than
  proxying into a dead port, or retire `llamaServerUrl` per `machines/aibox/STRATA.md`.
  Re-verify: `systemctl is-active llama-server.service` on `aibox`, and
  `curl -sk https://ai.int.leighhack.org/health | head -c 40`.

- **pi now has two ways of finding local models, both switched on.** `settings.json` carries
  `llamaServerUrl = "https://ai.int.leighhack.org"`, which gives pi its own `llama.cpp`
  provider via the router integration (four GGUFs on aibox's `llama-server`), while
  `programs.pi-agent` writes a `strata-services3` provider for services3's engine. Different
  servers, so no conflict — `pi --list-models` (measured 2026-11-14) lists both families side
  by side — but the overlap is a decision nobody wrote down, and `machines/aibox/STRATA.md`
  tells the next person to drop `llamaServerUrl` if llama-server stays displaced. **Blocks:**
  nothing. **To do:** per box, choose the endpoint row or the router integration for each
  server and retire the other; `ai.int.leighhack.org` as a row would fold the displaced
  `llama-server` in and make `pi --list-models` one story.

- **`settings.json` (`defaultProvider`, `defaultModel`, `defaultThinkingLevel`) is still
  hand-edited per-person state, and it names a model id.** `pi-agent-sync` does not touch it
  (it is a preference, and the program has no business rewriting one) but it *can* see when the
  saved default has vanished from the discovered list and says so as a note. The next step, if
  the owner wants it, is for the module to be able to *declare* a default per machine
  (`programs.pi-agent.settingsDefaults`) — which needs a decision first about whether the
  flake may write into a person's settings file at all, given it already writes into their
  agent directory. **Blocks:** that decision, not any code.

## pi's web search (every box) — opened 2026-11-16, `docs/pi-web-search.md` owns it

- **Rotate `brave_api_key`: it was pasted into an agent transcript, and the transcript is
  the one place a secret can be that the flake cannot reach** (2026-11-16). The key is
  now held only in `secrets/secrets.yaml` and read from `/run/secrets/brave_api_key` at
  request time, so nothing in this repo has to change to rotate it — `sops --set`, a
  switch, and the timer picks it up. What cannot be undone is the copy in the session
  log. **Blocks:** nothing (search works). **To do:** issue a new key in the Brave
  dashboard, `sops` it in, switch, and confirm with `just pi-search-status` (it prints
  the HTTP code and never the key). Re-verify with: `sudo stat -c '%a %U:%G' /run/secrets/brave_api_key`
  → `640 root:brave-search-readers`, and that recipe's credential block reading
  `readable`.

- **`npm:pi-web-access@0.48.0` is third-party code running inside every pi session on
  five machines and its source has not been reviewed** (opened 2026-11-16, the day it
  was deployed). What is *measured* is that the Brave path does what its README says —
  the credential command source, `provider: brave`, a live search returning two sources
  — and that its dangerous defaults are off (browser-cookie extraction, hosted
  page-fetch fallbacks, SSRF guard armed); the last of those is why `fetch_content` is
  not a route from an agent session to the LAN. What is not measured is the other ~150
  modules: an MCP server, a `bin`, HTML/PDF parsing, and 211 npm dependencies.
  **Blocks:** nothing. **To do:** read the pinned tag for anything that phones home or
  writes outside `~/.pi/agent`, and record the version reviewed here. Bumping the pin is
  upstream's CHANGELOG, then `packageSource` in `common/pi-search.nix`, then
  `just pi-agent-sync && just pi-search-status`.

- **Nothing watches the timer's two file merges** — same gap as the model list
  (§Open of `docs/pi-agent-models.md`): if `settings.json` becomes unparseable, the run
  records a note and pi quietly loses web search on the next start. A
  `lease-reader-watch`-shaped check (a red unit, not a heartbeat) would need the same
  decision about who is paged. **Blocks:** a decision, not a technical problem.

## `just clippy` has been failing, unobserved, on services1 (found 2026-11-14)

`just clippy` exits 1 with:

```
/run/user/1234/just/just-XXXXXX/clippy: line 193: out: unbound variable
```

— before cargo runs. **It is not the tree's fault**: the same failure reproduces from an
unmodified `git show HEAD:justfile` (`just -f ./justfile.headcheck clippy`), so no
`pi-agent-sync` change is involved; the recipe's `out=$(mktemp)`/`trap … "$out" EXIT` pair is
inside the single-quoted argument to `nix develop --command bash -c '…'`, and by the time
bash expands it the quoting is gone. The recipe was hardened in the pass that made a clippy
*failure* fatal ("a lint gate that cannot fail is worse than no gate") — and the effect here is
the same failure mode one level up: **a gate that always dies looks identical to a gate that
ran and found nothing**, except in reverse, so anyone who typed it and saw red assumed the
code was at fault (this author did, twice) and anyone who saw it green had never seen it.

**Blocks:** nothing, because the workaround is one line and is what was actually used for the
2026-11-14 change:

```bash
nix develop --command bash -c 'set -euo pipefail; cd crates; cargo clippy --workspace --all-targets --offline'
```

(reports the same diagnostics; workspace currently has ~35 pre-existing warnings in
`wan-manager`, `common-pg`, `filestore`, `frigate-monitor`, `network-status`, and **none** in
`pi-agent-sync` or `common-json`). **To do:** rewrite the recipe without a shell variable
across the `nix develop` boundary — a fixed log path under `${TMPDIR:-/tmp}` chosen by the
outer shell, or drop the `grep -v` filter and let `--message-format=short` do the trimming —
then verify it can both fail and pass: `just clippy` on a tree with an introduced
`let _ = 1u8 as usize;`-style lint must exit non-zero.

## `cargo test --workspace --offline` cannot resolve on aibox (measured 2026-11-22)

`just test` — and any `cargo test --workspace --offline` in the dev shell — dies before
compiling:

```
error: failed to download `allocator-api2 v0.2.21`
Caused by: attempting to make an HTTP request, but --offline was specified
```

`allocator-api2` is a dependency of `hashbrown`, so this is the **workspace's** dep graph
(one crate in it wants a hashbrown build with the `allocator-api2` feature), not anything
`crates/voice-assistant` or `crates/slack-agent` pulls. **Verified against clean HEAD** in a
throwaway `git worktree` with `CARGO_TARGET_DIR` pointed outside the tree: identical failure,
so it is not this month's change and no reverted flag caused it.

**Blocks:** `just test` on aibox, and therefore the "run the whole suite before a deploy"
habit, which is why `docs/work-log-2026-11.md` records per-crate runs instead.

**Workaround (what was actually used 2026-11-22):** per-crate, which resolves a smaller
graph and is fully offline — `cargo test --offline -p common-web -p common-http -p
slack-agent -p voice-assistant`. Note `just test` also runs
`bash common/shared-rs-links.sh` first, which the per-crate form does not need here only
because the symlinks already exist.

**To do:** either warm the cargo store for the full graph once (an online `cargo fetch` in
the workspace, or let `check-build` populate it via crane's vendored deps — note the two
caches are *not* the same thing, so this may mean building `cargoDeps` for the offending
crate), or make `just test` say so out loud instead of exiting 101 with a download error
that reads like a broken tree. The second is a `justfile` change and the first is an
hour's work; the failure mode of doing neither is that the workspace-wide gate quietly
stops being run.

## Strata's engine corrupts its own store path, which is why aibox's substitution has kept failing (diagnosed 2026-11-14, while deploying `pi-agent`)

`just check-build` dies on the aibox toplevel while **substituting the engine**:

```
error: hash mismatch importing path '/nix/store/97dahyfv…-strata-unstable';
         specified: sha256:1qz5jabmdmmhgcfcizbj30cxc5ksk6j59fjgf62q6wi321mv4h3n
         got:       sha256:1vzph3ijra05z1z8glr6c5gw10a2lg94v02gclmdsgyrzdhn4k2r
error: Cannot build '…-unit-strata.service.drv'  →  system-units  →  etc  →  nixos-system-aibox
```

`nix store verify` on aibox agrees, and named two paths, not one — every `*-strata-unstable`
that has ever **run** there:

```
$ sudo nix store verify --no-trust /nix/store/97dahyfv…-strata-unstable
path '…' was modified! expected hash 'sha256:1qz5jabd…', got 'sha256:1vzph3ij…'
$ find /nix/store/97dahyfv…-strata-unstable -newer "$P" -type f
  2026-11-14 02:54  share/strata/strata_control/__pycache__/tune_machine.cpython-314.pyc
  2026-11-14 02:54  share/strata/strata_control/__pycache__/private_files.cpython-314.pyc
  …                      (every entry: `__pycache__/*.cpython-314.pyc`, nothing else)
$ find /nix/store/f0jwild…-strata-unstable -name "*.pyc" -printf "%TY-%Tm-%Td\n" | sort -u
  2026-11-04        (the 0.1.56 build, pycache from the day it was benched)
```

**The cause is that the Strata engine is a Python program with its control layer in `$out/share`,
and it writes its bytecode caches into its own store path.** The unit runs as root, root may
write to a `0555` directory it owns, so the first import creates `__pycache__/`, and from then on
the path's contents no longer match the hash in its own name. Reading it never notices (local use
does not re-hash); **copying it out does** — so the failure appears as `hash mismatch importing
path` on whatever machine asks for the closure: services1 substituting from aibox, `nix copy`, or
`just check-build`. It is not bit-rot, nobody patched a binary, and it is not aibox being sick: it
is the shape of the package.

Three consequences to know before debugging anything else on this box:

* `just check-build` **cannot pass for aibox** while the pinned engine path has been run, so any
  agent that runs it after an aibox-touching change will be handed a red gate that has nothing to
  do with their change. It is worth being explicit about, because the failure text names the
  engine and the ordering suggests a build bug.
* The `nix store delete` mystery in the §Nix / build item above ("the unreferenced ones still
  refused as `still alive` … unexplained, so check `nix store verify --all` if it recurs") is now
  explained by the same class of thing: a store path whose contents drifted cannot be treated as
  a pure, disposable copy — `verify` is the tool that says so, and for the strata paths it says it
  every time.
* The mtime of the `__pycache__` files is a useful clock: it says when an engine from that build
  last started. `97dahyfv…` at 02:54 today, `f0jwild…` at 2026-11-04, matching the bench passes
  `machines/aibox/STRATA.md` records.

*And it is the live engine, not a stale leftover.* Re-running the aibox toplevel build from
services1 at the **new** pin (2026-11-14, after the deploy) fails on that same path:

```
error: hash mismatch importing path '/nix/store/97dahyfv…-strata-unstable';
         specified: sha256:1qz5jabmdmmhgcfcizbj30cxc5ksk6j59fjgf62q6wi321mv4h3n
         got:       sha256:1vzph3ijra05z1z8glr6c5gw10a2lg94v02gclmdsgyrzdhn4k2r
```

which is the output of the drv the current `flake.lock` wants, the path
`systemctl show strata -p ExecStart` names, and therefore **the engine aibox is running is not
reproducible from the flake by anyone but aibox** — the box will happily serve its own copy
forever (`just deploy aibox` succeeded at 16:0x today, reusing exactly this path, so the engine
never even restarted), and every other machine's attempt to obtain it fails. `verify` →
"modified", `find -newer` → `__pycache__`: the two readings belong to the one mechanism.

**The fix is one line per engine box**, and it is deliberately not applied here because it
restarts a loaded inference box and lands in the middle of the 2026-11-14 tuning work
(`machines/services2/STRATA-TUNE-2026-11-14.md`, and aibox's engine was started at 03:16 today):

```nix
# machines/aibox/strata.nix and machines/services2/strata.nix, in the unit's serviceConfig:
Environment = "PYTHONDONTWRITEBYTECODE=1 PYTHONPYCACHEPREFIX=/run/strata/pycache";
```

(`PYTHONPYCACHEPREFIX` alone is friendlier: caches still get written, to a tmpfs the service owns.
`RuntimeDirectory=strata` would give it the directory. A non-root `User=` is the deeper fix and a
bigger question — the engine touches `/dev/kfd` and the card.) Then, to repair what is already
dirty, delete the caches — they are not in the original nar, so removing them restores the
recorded hash: `sudo find /nix/store/*-strata-unstable -name __pycache__ -type d -exec rm -rf {} +`
and re-check with `nix store verify`. **Blocks:** `just check-build` for aibox, and any
`nix copy`/substitution of an engine closure. **To do:** the two lines above plus the cleanup, in
the same pass as the next engine deploy so it costs no extra restart. **Re-verify with:**
`sudo nix store verify --no-trust $(systemctl show strata -p ExecStart --value | grep -oE "/nix/store/[^ ]*strata-unstable")`
(no output = repaired), and `just check-build`.

## aibox's own system depends on the Pi Room image's whole desktop closure, so aibox switches rebuild browsers

The `pi-agent` deploy to aibox (2026-11-14) began with **405 local builds on aibox** —
`firefox-158.0`, `code-1.140.0` with its `electron`/`fhsenv` halves, `squashfs`, ~400 more — on
the box that is also the fleet's builder. Load average 11 on 12 cores inside two minutes, and
every other machine's build queued behind it. The change being deployed adds a oneshot unit and a
timer.

It is not aibox's desktop: `programs.firefox` and `vscode.fhsWithPackages` are in
`machines/pi-room-sys/configuration.nix` (the netboot image's own config), and
`machines/aibox/netboot.nix` makes **aibox's system** depend on building that image and
bind-mounting `build.squashfsStore` at `/exports/netboot-squashfs`. So the Pi Room image's entire
package closure is an input to aibox's `system.build.toplevel`: any heavy package added to, or
version-bumped in, the netboot image becomes a multi-hour compile on the fleet's builder, and it
surfaces on whichever aibox switch happens to be first. It landed all at once here because
aibox's checkout was 28 commits behind (see the divergence note below), so one switch carried
every intervening closure change.

**Blocks:** nothing — the builds finish, and the next switch of that same closure is instant.
But it means an aibox deploy has an unbounded, invisible cost budget that belongs to a *different
machine's* config, which is the wrong place for it to be discovered. **To do,** pick one: build
the netboot image somewhere that is not the fleet's builder (or push `squashfsStore` to a
substituter aibox reads, so the image is copied rather than compiled); or keep aibox's dependency
on the image *narrow* (export a path to the already-built squashfs, produced by a job rather than
by the system's own closure); or simply decide that the Pi Room image gets no browser-class
packages, which is the cheapest fix and probably the honest one for a kiosk netboot.
**Re-verify with:** `grep -cE "^building " <deploy log>` for an aibox switch (a routine one should
be near zero), `nix eval --raw .#nixosConfigurations.aibox.config.system.build.toplevel | xargs nix
path-info -S | numfmt --to=iec`, and `nix path-info -S` on
`${build.squashfsStore}`'s output closure from `machines/aibox/netboot.nix`.

## The hackspace Slack bot (services1) — opened 2026-11-16, `docs/slack-agent-2026-11-16.md` owns it

- **Nobody has seen the Monday digest fire in public yet.** `slack-agent-digest.timer`
  (`Mon 09:00`, `Persistent=true`) is armed and `just slack-agent-digest` prints exactly what it
  will post to `#general`, but a preview is not a receipt: the first real firing is Monday, and
  the failure modes are the boring ones — the token revoked, `#general` renamed, the space API
  down at 09:00. **Blocks:** nothing. **To do:** after the first Monday, confirm the message
  exists and `systemctl status slack-agent-digest.service` is clean. **Re-verify with:**
  `systemctl list-timers slack-agent-digest.timer`, `systemd-analyze calendar "Mon 09:00"`, and
  `journalctl -u slack-agent-digest.service -n 20`. A missed Monday still posts (`Persistent`),
  which is deliberate but untested here — a power-cut Monday is the day to notice.

- **The `#doors` access feed was dropped, deliberately, on 2026-11-16** ("doors not needed") when
  `vulcan-bot` was stopped. Recorded here so a future reader does not find `access_poster` in the
  old plugin and file it as a regression: MQTT `access/log` is still published by services1's
  broker and still read by whatever else dials it, and `#doors` simply stops receiving Slack
  posts. **Blocks:** nothing. **To do:** nothing, unless someone misses it — the port is then
  small and belongs in `slack-agent` next to the digest (subscribe, fixed format, no LLM), with
  the old container on `apps1` as the format reference until it is deleted. **Re-verify with:**
  `mqtt.int` answering and `journalctl -u slack-agent` showing no door traffic, which is the
  expected state, not a fault.
- **It is live now** (`dryRun = false`, `--channels "*"`, since 2026-11-16): it answers
  unmentioned in whatever channel the request came from, gated by `triage::answers`. What is
  unproven is the *judgement* at scale — whether the gate is too eager in a busy `#general`,
  and whether answers people want are phrased in a way the gate recognises. **Blocks:** nothing.
  **To do:** read `journalctl -u slack-agent` after a day of real use — every refusal is logged
  with the reason, so both "too chatty" and "should have answered" are answerable from the
  journal — then tune `triage.rs`'s topic/veto lists, and if it is noisy set
  `infra.slackAgent.dryRun = true` (one line) rather than reasoning about it cold.
  `slack_agent_turns_total` counts answers; there is no skip counter yet — **and stop `vulcan-bot.service` on `apps1` in the same change**: measured
  2026-11-16 12:44, Slack delivers every event to *both* Socket Mode consumers of the shared app
  token, so two live bots means two replies to one `@Vulcan` (`docs/slack-agent-2026-11-16.md`
  §13 has the receipts from both journals).
  Until then the bare words `temps`/`status`/`events`/`printers`/`humidity`/`members` are
  deferred to Vulcan by `triage::vulcan_command`, so the two bots never both answer them.
  Then phase 3 (port the `#doors` MQTT feed and the Monday digest,
  both deterministic, no LLM) and phase 5 (park `vulcan-bot.service` on `apps1` in §10's
  order, keeping `/srv/vulcanbot` and the container for a week). **Re-verify with:**
  `systemctl is-active slack-agent hackspace-mcp`, `curl -s 127.0.0.1:819{1,2}/health`,
  `just slack-agent-status`, and `grep -o '\-\-dry-run' /etc/systemd/system/slack-agent.service`.

- **Nobody has scored the eval yet, so "the answers are good" is an impression.** The gate in
  §11 phase 2 is ≥ 11/12 canonical questions correct with no invented number, per engine; the
  fixture is `crates/slack-agent/questions.txt` and the runner is `just slack-agent-eval`, but
  scoring a grounded answer against reality ("is it cold?") needs a human who has been to the
  space. **Blocks:** going live honestly. **To do:** run the eval, score it in that file's
  comments, and record which engine won — the ordering in `common/llm-endpoints.nix` is
  currently a guess about *quality* (it is a measured guess about *availability*).

- **The Home Assistant token `hackspace-mcp` dials with is neither scoped nor its own**: it is
  `DE_HOME_ASSISTANT_ACCESS_TOKEN`, the door-entry system's long-lived token, copied under a new
  name so a rotation is a decision rather than an accident (no minting was needed to ship the
  prototype). A HA long-lived token carries its user's whole authority, so "read-only" here is
  enforced by *this code containing no service-call path*, not by the credential. That is the
  boundary §7 relies on, and it is weaker than it should be. **Blocks:** nothing today. **To
  do:** mint a token for a HA user with no admin and no entities to write, put *that* in
  `slack_agent_env`, and leave door-entry's alone. **Re-verify with:**
  `just slack-agent-call home_assistant_states '{"limit":1}'` (works) and, in the HA UI, that
  the token's owner is not an admin.

- **The Slack app is Vulcan's, scopes and all**: reusing its tokens was the zero-config path,
  and its manifest asks for ~25 scopes including `files:write`, `pins:write`, `usergroups:read`
  for a bot that reads text and posts text. Its display name is also still Vulcan's, which is
  confusing during the overlap. **Blocks:** nothing. **To do:** either trim the manifest in the
  Slack app admin (cheap, keeps history and `#doors`' webhook) or mint a minimal app with
  `app_mentions:read`, `channels:history`, `groups:history`, `im:history`, `chat:write`,
  `reactions:write` and re-sop two tokens. Decide before phase 5, because after the cutover the
  old app is the only thing that proves which one the workspace was talking to.

- **Adding one key to `secrets/secrets.yaml` re-keys every `config.sopsSecretText` derivation
  in the fleet**, so the first `nix flake check --no-build` after any secret edit stops being
  eval-only and fails with `path '…-sops-secret-<name>.drv' is not valid` until that
  machine's closure has been built once (`builtins.readFile` on a derivation output is the
  eval-builds flaw this repo already documents, and `services.outline.databaseUrl` and the two
  strata modules are where it bites). Measured 2026-11-16: clean HEAD passed, `pg_pass` cleared
  after the services1 switch, `strata_api_key` needed services2's and aibox's toplevels, and the
  same edit restarted every service embedding a build-time secret on every box switched
  afterwards. `nix build '<that .drv>'` and `nix-store -r '<that .drv>'` both *refuse* it ("no
  substituter that can build it" / "failed to obtain derivation") because the `.drv` is not in
  the store — the only route is building the config that references it. **Blocks:** a green
  `just check` on a box whose sibling machine has not been switched since the secret edit. **To
  do:** either split `secrets.yaml` so build-time names live in a file nothing else touches, or
  give `just check` a repair step that builds the named closure, or (least honest, most
  convenient) stop embedding secrets in derivations at all. **Re-verify with:**
  `nix flake check --no-build 2>&1 | grep -o 'sops-secret-[a-z_0-9]*' | sort -u` after
  `echo x | sops --set '["a","b"]' /dev/stdin secrets/secrets.yaml`-shaped no-op edit.

- [ ] **Wake recordings: the engine owns its directory, so the tmpfiles rule in
      `voice-assistant.nix` must go.** Deployed 2026-11-17 and verified end to end (a session's
      audio landed as `wake-<epoch>-<id>-<label>.wav`, 0600), but the directory is created by
      `dump()` running as the unit's **DynamicUser**, and `systemd.tmpfiles.rules` carries
      `0700 root root` — so the first boot on which tmpfiles wins the race chowns the recordings
      directory to root and the engine starts failing to write, quietly, with only a journal line.
      Fails safe (no recordings rather than a broken detector), which is the only reason it is a
      follow-up and not an emergency. **Do:** delete the rule and prune inside `dump()` — keep the
      newest N files, mirroring `wav::prune_wavs` on the answer side, so the writer owns retention
      and there is no second mechanism to disagree with it. **Re-verify with:** `sudo ls -l
      /var/lib/voice-assistant/wake-record` after a reboot, and one synthetic session against
      `127.0.0.1:8195` (POST `/v1/session` → chunks → DELETE) producing a file.

- [ ] **The wake page shows a score, but never shows whether it is still feeding the engine.**
      Fixed the fault (2026-11-17, `5d15955`): a reaped session made the page stop streaming, its
      score froze at the last value, its level meter kept moving, and it could not be re-armed
      from the select — reported as "server seems to lock up after a minute or so, score stuck",
      and provably not the engine (`NRestarts=0 Result=success`, `chunks 114 → 114` over 10 s,
      journal `reaped 1 idle session(s); 1 live` then `0 live`). The contract the recovery relies
      on was checked end to end through the front door rather than assumed, because a proxy that
      rewrote the status would have made the fix dead code: `POST /v1/wake/session` → `DELETE` →
      `POST /v1/wake/chunk` returns **410 `{"error":"no such session (idle too long? open
      another)"}`** with the status intact, and a chunk to a live session returns
      `{"score":0.0000,"peak":0.0000,"detected":false,…,"infer_ms":0.0,"cpu_ms_per_audio_s":25.8}`.
      **Still open, and it is the same class of fault as rule 29 (a channel can die while still
      answering):** nothing on the page distinguishes "the room is quiet" from "the engine has not
      heard from me in four minutes" from "the tab lost its session" — a frozen number reads as a
      low number. **Do:** put `audio_secs`/`chunks` from the engine's own replies on the card as
      *seconds since the last accepted chunk*, and banner it past a few seconds like
      `/api/leases` does for a stale table; the engine already answers `sessions_total` and each
      chunk reply carries `audio_secs`, so it needs no new server field. **Re-verify with:** arm
      the server engine, `sudo systemctl stop voice-gateway`, and confirm the card says it is no
      longer being fed within a few seconds instead of quietly holding its last score.

- [ ] **Should a browser tab be able to keep a wake session warm across its own take?** It
      deliberately streams nothing while capturing a command (the ear must not hear it), so the
      session ages for the whole take plus the answer. The 2026-11-17 fix is a widening (idle
      120 → 300 s) plus recovery, both of which are treating the symptom; the clean version is a
      keepalive the engine can see (`POST /v1/keepalive`, or refresh `last_seen` on any request
      naming the session), which decouples "still listening" from "still sending audio" instead
      of picking a timeout that happens to exceed today's worst answer.

- [ ] **Decide: port openWakeWord to the server, or run the browser's own JS on it.**
      Settled by measurement (2026-11-17, corpus from `wake.record`, 418 s of the room with
      people in it): the LiveKit model peaks **0.583** on ordinary speech and **0.626** on a
      clean utterance of the phrase — a factor of two, so **no threshold separates them** (8
      accepts at 0.30 = 68/hour, 2 at 0.50). Threshold is therefore not a tunable, and the
      detector has to be replaced. openWakeWord is the candidate because it is the detector
      the room already trusts. My Rust port (`--engine oww`, `crates/voice-wake/src/oww.rs`)
      is **not it**: it scores ~1e-5 and decays, and the exclusion list is now long —
      identical model bytes (sha256 against the browser's own `dist/models`), identical f32
      input scale, identical `/10+2` normalisation, identical 76-frame window / 8-frame step /
      zero-initialised 16-embedding history / row-major `[1,16,96]`, and the window sweep
      proves the graph's dim is 16 (8/12/24/32 error on shape). What is left is a
      **tract-vs-ONNXRuntime numerical difference**, probably inside the TCN, which is not a
      bug I can see by reading. **Option B, recommended:** stop porting and run the reference
      code — `crates/whisper-ws/web/openwakeword/engine/WakeWordEngine.js` unmodified, under
      `onnxruntime-node` — behind the `/v1/session|chunk|session` contract
      `voice-wake serve` already implements, so `voice-assistant` and the page need no
      change. That bends AGENTS rule 1 (Rust for anything new) and should be recorded as a
      deliberate exception **with its reason**: the JS is not new code to maintain, it is the
      only implementation of this detector that has ever worked here, and the port is what
      failed. CPU is the price, measured: 287 ms per audio-second natively (~3.5 listeners
      per core, `oww-bench`) and worse under WASM — fine for the two or three microphones
      this could ever have, and the reason it is not fine for *many* microphones is the same
      reason detection belongs on the device (`docs/esp32-voice-satellites.md`). **Do not
      deploy it before it has been scored on the corpus** (`voice-wake bench` over
      `/var/lib/voice-assistant/wake-record`) and watched for a day with `wake.record` on.
      Also worth stealing from the browser regardless of which option wins: it does not emit
      on score alone — `score > threshold && isSpeechActive` (silero VAD) and a 2 s cooldown,
      so a big score on a non-speech frame is discarded. Our server gate exists but its
      silero export will not load in tract (`If` op), so we are missing exactly that
      half of the web version's precision.

- [ ] **Fix the embedding → keyword handoff in `voice-wake`'s openWakeWord port, before anyone
      trusts `--engine oww`.** Built 2026-11-17 and measured immediately: the mel graph loads under
      tract, runs, and emits the browser's own log-mel (`shape=[1,1,5,32]`, values in the −20…−60
      range, `/10+2` applied, 5 frames of 32 pushed per 80 ms frame, 76-frame window stepped by 8,
      `[1,76,32,1]` into the embedding graph — every one of those read off
      `crates/whisper-ws/web/openwakeword/engine/`, which is the working oracle). But the keyword
      head's output runs 8.8e-5 → 1.7e-6 and *falls monotonically* across a clip containing the
      phrase, and the embedding probe never fires. A score that decays toward zero while fresh
      audio arrives is not a model saying no — it is the keyword head being fed the same stale
      window every time, i.e. `history` never receiving the embeddings the loop computes. Start
      with `OWW_DEBUG=1`, which now prints mel shape/values, embedding shape/values and each
      keyword output: the fix is verified when embeddings print and the score stops decaying.
      **Until then `--engine livekit` stays deployed**, because a wake engine that is silently
      always-no is worse than one that is merely mediocre — it is indistinguishable from nobody
      speaking. **Re-verify with:** `OWW_DEBUG=1 voice-wake oww-bench <assets>/models /tmp/pos16.wav
      --gate off` and read whether the score rises anywhere.
- [ ] **Record real room speech to a WAV so the two wake detectors can be compared on bytes.**
      `voice-wake oww-bench` (`docs/voice-assistant-2026-11-16.md`, "Which detector, where") runs
      openWakeWord and LiveKit on the same runtime and the same model hashes, so the A/B is built
      and has no valid input: piper-synthesised audio is out-of-distribution for openWakeWord
      (0.000 there, 0.626 here) and the room recording has no wake word in it. What is needed is
      a person saying the phrase into `cam8` and into a laptop — the same bytes through `bench`
      and `oww-bench` — plus one busy hour of room audio through both for false accepts per hour.
      Also unresolved by the same recording: **how much of the browser's edge is the silero VAD
      gate**, which tract cannot load as exported (`If` op). Either re-source the VAD (older
      export, or our own energy gate) or settle it with `--gate off` against the shipped engine
      and say honestly that the gate is unmeasured. **Re-verify with:** `voice-wake bench
      <hey_jarvis.onnx> <dir>` and `voice-wake oww-bench <model-dir> <dir> --gate off` on the
      same directory.

- **Strip the wake phrase before the fast path sees the transcript.** Measured 2026-11-17:
  `"how warm is the workshop"` answers in under a second through `Agent::quick_answer`,
  `"hey jarvis how warm is the workshop"` takes 4 rounds and 19.1 s, because the recognizer
  counts two named words and correctly backs off. The camera satellite already strips its
  phrase (`wake::strip`); the browser and server-wake paths must do the same, or the fast
  path is a lab result. **Do:** strip in the caller that owns the wake event, not inside
  `quick` — the phrase list belongs next to the detector that produced it. **Re-verify
  with:** `curl -s -X POST http://127.0.0.1:8194/v1/ask -d '{"text":"hey jarvis is the space
  open","voice":true}'` and read `rounds` (0 = stripped, 2 = not).

- **Piper is the voice floor now.** A fast-path answer costs 40 ms and the turn still costs
  2.5-3.4 s wall, because `piper` is spawned per turn and reloads its voice every time.
  **Do:** a warm synthesizer (a persistent child, or `wyoming-piper` on loopback behind the
  gateway) plus audio that streams to the browser as it is made, so first sound lands with
  the first sentence. **Blocks:** nothing; it is latency only. **Re-verify with:**
  `time just voice-ask "is the space open"` — the fast path is 0.0 s of agent, so whatever
  remains is synthesis and transport.

- **The event list and the Monday digest still print ISO dates, and the voice says "dash".**
  Measured 2026-11-17 through the same espeak-ng front end piper uses (`espeak-ng -v en-gb -x -q`):
  `2026-11-17` renders `t'u: T'aUz@nd @n tw'Enti s'Iks d'aS Il'Ev@n d'aS s'Ev@nt,i:n` — the word
  *dash*, twice — where `17 Nov 2026` renders as a date. `clock::nice_date` now exists and is
  what `what_time` prints (the tool was written ISO first and the measurement is what changed
  it), but `space_events` still heads its groups `YYYY-MM-DD` and the digest reuses that
  renderer, so a spoken "what's on" reads a heading aloud with dashes in it. **Blocks:** nothing
  except a slightly stupid voice. **Do:** route the event headings through `clock::nice_date` in
  the same change as the digest's own format pass, and keep the ISO form wherever it is a key.
  **Re-verify with:** `cargo run -q -p hackspace-mcp -- --call space_events '{"days":7}'` and
  `just slack-agent-digest`, then `nix run nixpkgs#espeak-ng -- -v en-gb -x -q "<first heading>"`.

- **`hackspace-mcp`'s compiled-in default source URLs still name `apps1`.** `--space-url` and
  `--hass-url` default to `http://10.3.1.30:…`, which is where Home Assistant lived before
  2026-11-16; the unit on services1 passes `127.0.0.1:8081`/`127.0.0.1:8123` explicitly, so the
  running server is right and only a **hand-run** binary or a fresh box inherits the old
  address. Measured locally 2026-11-17: `--self-test` with no arguments reports
  `connect 10.3.1.30:8081: Connection refused` for `space_status`, which reads like an outage.
  **Blocks:** a quiet first run on a new machine. **Do:** default them to loopback (that is
  where both live now) and leave the flag for anywhere else. **Re-verify with:**
  `grep -n "10.3.1.30" crates/hackspace-mcp/src/main.rs` (zero hits) and
  `cargo run -q -p hackspace-mcp -- --self-test` from a box with no arguments.


## `nixos-rebuild switch` did not re-arm `auto-rollback.timer` on services1 (measured 2026-11-16)

AGENTS' GOLDEN RULE says activating a configuration restarts the auto-rollback timer and
"it re-arms itself at the next activation either way". Measured twice on services1 at the
`2074cc6` lock, against the running units rather than the docs: a switch that built and
activated new units, and a second switch minutes later that changed nothing, both left
`auto-rollback.timer` `inactive` with **no** `NextElapseUSecRealtime`, and the only thing that
armed it was `sudo systemctl restart auto-rollback.timer`. (`Active: active (waiting) since
11:00:22` stayed at my manual start time across a switch at ~11:01.) Where the timer comes
from is also not obvious: `system.autoRollback.enable = true` is set in `common/base.nix`, the
generated timer is `OnActiveSec=5min` + `RemainAfterElapse=false` with no `[Unit]`/`WantedBy`
beyond `timers.target`, `grep` over `/run/current-system/bin/switch-to-configuration` and
`/run/current-system/activate` finds no reference to it, and the module that defines the option
was not found under any `/nix/store/*-source/nixos` tree — so the arming path, whatever it is,
is not in the switch script.

**Why this matters and what does not:** the net is a *safety net*, and the confirm is the
control — the rule to keep is `switch && nixos-confirm` in one invocation, and the health check
is `readlink -f /run/current-system /nix/var/nix/profiles/system-good` (equal = the box has
decided to stay), not `is-active auto-rollback.timer`. What changes if the docs are wrong is
the *failed*-activation case: the section "the five-minute window is a countdown to a reboot"
describes the window in which to decide, and if nothing armed the timer, a half-activated box
is not counting down to anything and nobody is told. **Blocks:** nothing (services1 is
confirmed good, both new units healthy). **To do:** find the module that owns
`system.autoRollback` (not under `common/`, not in nixpkgs' visible module tree — probably a
flake input), read what starts the timer, and either fix the arming or fix AGENTS and
`docs/nix-dev-workflow.md` to say that on these boxes the net must be armed by hand
(`systemctl restart auto-rollback.timer`) after a switch whose confirm did not run. **Re-verify
with:** `sudo nixos-rebuild switch --flake .` then immediately `systemctl show
auto-rollback.timer --value -p ActiveState -p ActiveEnterTimestamp -p
NextElapseUSecRealtime` — `ActiveEnterTimestamp` moving to the switch time is the documented
behaviour; staying at an older time is this finding.

## The intercom (services1, off Proxmox on 2026-11-16) — opened 2026-11-16, `docs/pbx-on-services1-2026-11-16.md` owns it

- **`do this first`: the pool exclusion for `.166` is DEPLOYED as of 2026-11-16 (c6a4592),
  and the reason it is called out here is how it was nearly missed.** The first
  `just deploy gw` of the day built from **gw's own checkout at the last commit**, so the
  then-uncommitted `dhcp-host=10.3.1.166,ignore` never travelled: the deploy exited 0, the
  router was healthy and confirmed, and `sudo grep -n 10.3.1.166 /etc/dnsmasq.d/network.conf`
  still found nothing while `dhcp-range=tag:vlan225,10.3.1.150,10.3.1.253` went on covering
  the address. **`just deploy` carries a commit, never a working tree** — so the check that
  belongs at the end of any router change is the grep on the *rendered* file, not the exit
  status. Deployed and verified now: `git log -1` on `gw` = c6a4592, exclusion at line 82,
  `readlink /run/current-system` == `system-good`, dnsmasq active, no failed units, both
  handsets still registered afterwards. What is left is the actual fix, in the next bullet:
  move the address out of the pool and delete this line in the same change.
- **The endpoints have no ACL, because every form of `acl=` tried failed closed.** Inline
  CIDRs (`/16` and dotted-netmask alike) are resolved **as names** on Asterisk 22, and the
  named form the shipped `acl.conf` sample prescribes parsed cleanly yet never loaded —
  `asterisk -rx "acl show lan"` answers `Could not find ACL named 'lan'` with `acl show`
  listing nothing. What it cost and why the log was misread: the whole story in
  `docs/pbx-on-services1-2026-11-16.md` §"The four things the deploy actually taught". The
  current boundary is: no DNAT for 5060 on the router, no TCP listener on the host at all, and
  digest auth on every endpoint. **Precondition for putting it back:** `acl show lan` answers
  with the ACL. Re-verify with: `sudo journalctl -u asterisk | grep -c "invalid ACL"` → 0.
- **Three extensions share one SIP password, and it was on a screen.** Extensions 1, 2 and 3
  all carry the same secret (verified by hashing `asterisk.devices`-equivalent rows against
  the installed auth file: 1/2/3 identical, 123 and 1337 distinct), and `asterisk -rx "pjsip
  show auth <n>"` prints the password in **cleartext**, so it entered a session transcript on
  2026-11-16 while diagnosing the ACL. Rotation is cheap server-side — edit
  `secrets/pbx.yaml`, `just switch`, and `preStart` re-installs it (no restart of anyone's
  call: the secret is runtime-only and deliberately outside the `restartTriggers` hash) — but
  the two handsets must be retyped, which needs the phone admin logins (Phyu), and VLAN 227's
  phones are hand-configured: there is no TFTP or provisioning server to push it from.
  Re-verify with: `sudo grep -c '^\[[0-9]*-auth\]' /var/lib/asterisk/pjsip_auth_secrets.conf`
  → 5, and `just pbx-status`.

- **`10.3.1.166` is a static address sitting inside VLAN 225's dynamic pool, and only a
  dnsmasq line keeps it from being leased to something else.** `machines/gw/dns.nix` emits
  `dhcp-host=10.3.1.166,ignore` — the one non-table-projection line in the generated
  `network.conf`, and it is a workaround, not the shape. The fix is to move the intercom to an
  address outside `.150`–`.253` and give it a name (`pbx.int.leighhack.org`, a socket name, so
  per AGENTS rule 24 it is an `answeredBy` row in `common/int-dns.nix` and **not** a
  `common/front-door.nix` row — `machines/gw/lib/check-network-tables.nix` is its guard, not
  `machines/gw/int-dns.nix`), then delete the `ignore` line in the same change. **What blocks
  it:** the two Snom handsets hold the address in their SIP accounts and their web UIs answer
  `401` — so it needs the phone credentials (Phyu has them), or a decision to re-provision by
  hand at the two desks. Nothing else dials `.166`, so the blast radius is two phones.
  Re-verify with: `just pbx-status`, `sudo grep -n "10.3.1.166" /etc/dnsmasq.d/network.conf`.
- **Remove the injected root SSH key from `mercury` and settle who owns the PBX's
  credentials.** `authorized_keys` on VM 128 carries a line
  `pi-migration-20261116 (remove when done)` alongside the pre-existing `Phyu@moogle.com`, and
  the private half is `~/.ssh/freepbx-migration-20261116` on services1. Nobody has the box's
  root password (that is why the key was injected — procedure and receipts in the doc). It
  matters least if the VM is deleted (next item) and matters a lot if it is not. Re-verify
  with: `grep -c 'pi-migration-20261116' ` on the guest, as root through the key itself.
- **Delete Proxmox VM 128, its 9 GB disk, and the two images on monster1** once a fortnight
  passes with no phone complaint. Everything worth keeping is already off the box:
  `services1:/srv/freepbx-mercury/` holds a sha256-verified 9.36 GB image plus
  `mysqldump`/`/etc/asterisk`, and it is inside the nightly `/srv` borg job;
  `/var/lib/vz/offline-images/` on monster1 holds two more images, which are **not** a backup
  (same machine as the thing they back up) and should be deleted with the VM, not instead of
  it. **Blocks on:** nothing but the fortnight. Also set the record straight in
  `docs/monster-proxmox.md` when it happens — its inventory table still lists `mercury` as
  live HA-managed infra.
- **The Proxmox boot race that took the phones down for ten days is still live for every other
  VM on that host.** Every HA start attempt of VM 128 failed with `storage 'nas2-nfs' is not
  online` after the 2026-11-06 host boot — third occurrence of the
  `docs/monster-powercut-runbook.md` failure (2026-09-30, 2026-10-07, 2026-11-06), and VM 128
  has `max_restart 3` where 108/109/127 have 5, so it gives up first. Moving the intercom off
  removes this victim, not the race: `pve-ha-lrm` still starts guests before the NAS export is
  mounted. Off-flake host, so the fix is a drop-in ordering `pve-ha-lrm` after
  `mnt-pve-nas2\x2dnfs.mount` — or moving the surviving VMs' disks off the NAS. **Blocks on:**
  a decision about what monster1 is for, since `apps1`/`web1` are also on it
  (`docs/monster-proxmox.md`). Re-verify with: `journalctl -u pve-ha-lrm | grep -c "not online"`.
- **`modules.conf` is `autoload=yes`** (the nixpkgs module's default, kept deliberately so the
  first deploy could not fail on a missing module). A PBX this small wants an explicit load
  list; do it as its own change so a regression is one commit, not one feature removal.

## The voice assistant (2026-11-16): the four things left open

Design and receipts: `docs/voice-assistant-2026-11-16.md`; the pipeline's own traps:
`crates/voice-assistant/README.md`. Shipped live but **silent in the room**: the gateway
answers the browser page, the satellite listens on cam8 and writes its answers to the state
directory.

1. **Has anyone heard `cam8`'s speaker?** The software path is measured (`rac play` accepted
   a 350 ms tone and reported it played); audibility through a C110 piezo across a working
   room is not, and no number from a desk can settle it. Until it does, `speaker = "off"`.
2. **An identity for the speaker path.** Playing means `ssh root@cam8 'rac play - -r 16000'`,
   and the fleet hop key is in operator accounts (`~/.ssh/agent-hop-key`), unreadable by a
   `DynamicUser`, and must not be copied into the store. Two options, neither taken:
   (a) a dedicated `voice-speaker` Unix account whose **public** key is installed on the
   camera's dropbear `authorized_keys` (a device write, in a maintenance window, and thingino
   keeps `/etc` on the overlay — see `cameras/README.md`), or (b) a small systemd-managed
   speaker agent holding the hop key, which re-opens rule 32's root-known_hosts question.
   `infra.voiceAssistant.sshKey` is wired for whichever is chosen.
3. **The gate's real false-probe rate.** `just voice-probe` (satellite minus the assistant)
   exists to answer this and nobody has run it for long enough to. The gate's criterion is a
   number someone has watched, not a threshold invented now; `probe_false_positives` in
   `/status.json` counts it and is deliberately not asserted. This is the gate before the
   speaker.
4. **A smaller whisper model on aibox.** 2.7 s per 3 s clip on CPU `medium`; `--device 1` no longer exists in
   whisper.cpp 1.9.5 — the `machines/aibox/whisper.nix` header still says the unit cannot
   restart, which is stale: the service is up and running on CPU, and that comment needs
   re-reading against the current pin). The **resident piper** half of this was built on
   2026-11-22 (`tts.rs`'s `Synthesiser`, `--output-dir` mode with a counted cold fallback)
   and is undeployed — see the streaming section at the end of this file. A **barge-in**
   design is still open if the speaker ever turns on: no AEC exists, so it is server-side
   against the playback clock, for which `audio_ms`/`played_at` are already recorded.
5. **`cam8`'s RTSP credentials are the firmware defaults** (`thingino:thingino`), plaintext
   in `/srv/frigate/config/config.yaml` — off-flake runtime state with no flake owner.
   Pre-existing for the video stream, but an always-on microphone raises the price; a
   rotation must land in the same change as frigate's config and the camera's own
   `/etc/raptor.conf`.

## aibox's strata paths: diagnosed and repaired 2026-11-17; two things still open

Diagnosed — it was never rot or tampering. `strata.service` has no `User=`, so it runs as
root; root ignores a store path's 0444/0555 on ext4; CPython therefore wrote
`__pycache__/*.pyc` **into the package's own store path**, and since a NAR records the file
list but **not mtimes**, the added files made the directory stop matching the hash nix
recorded. aibox kept calling the path valid (`nix path-info` fine, `--sigs` says
`ultimate`, the engine runs perfectly) while every other box refused to import it — which is
why this surfaced as `just check-build` dying 898 derivations in rather than as anything
visible on the builder. Mechanism, both symptom texts, the repair and the general rule:
`docs/nix-dev-workflow.md`, Gotchas.

Repaired by deleting exactly the post-registration caches (bytecode caches are regenerable,
so that *is* the repair for this class, and it needed no restart): all five strata paths on
aibox now `nix store verify --no-trust` clean, engine still `active` with `NRestarts=0`, and
`nix copy` of the path to services1 — the operation that had been failing — succeeded. The
other four boxes scan clean. `common/store-hygiene.nix` (via `common/base.nix`) now walks
every store weekly at idle priority and **fails its unit** naming the paths, so the next
occurrence is a `systemctl --failed` line rather than a lost build.

1. **`PYTHONDONTWRITEBYTECODE=1` on `strata.service` — the real prevention, deliberately not
   written yet.** Either that or a non-root `User=`; both *restart the engine to apply*, and
   aibox's engine is a multi-minute, tens-of-GB load, so it belongs with the kernel reboot
   aibox already owes (`just pending-reboot`), not on top of an unrelated switch. Until it
   lands, the pollution returns at the next strata restart and the weekly check will say so.
   Re-verify with `sudo nix store verify --no-trust /nix/store/*-strata-unstable`.
2. **gw's eval warns about `ups-watchdog-0.1.0`** — `getExe: package does not have
   `meta.mainProgram``, implicit behaviour deprecated by nixpkgs. One line
   (`meta.mainProgram = "ups-watchdog";`, or `getExe'` at the call site) makes the router's
   eval quiet, which rule 9 asks for. Pre-existing; spotted while deploying the hygiene
   timer, not caused by it.

## `update-containers.service` fails every night on services1 (found 2026-11-17, pre-existing)

Noticed while reading fleet state after the strata deploys: `systemctl --failed` on
services1 has had one unit in it **10 nights running** — `update-containers.service`,
nixpkgs' `oci-containers` updater, which does `podman pull <image>` for every declared
container:

```
Error: unable to copy from source docker://uosserver:c9603dec9010:
       reading manifest c9603dec9010 in docker.io/library/uosserver:
       requested access to the resource is denied
```

That is not a registry outage and not auth: the UniFi OS Server container's image is
declared as a **bare local tag** — `machines/services1/services/unifi-os-server.nix:61`
sets `passthru.imageTag = "uosserver:${imageVersion}"` and `:151` feeds it to
`oci-containers.containers.<x>.image` — and an unqualified tag makes podman resolve
`docker.io/library/uosserver`, which is not a thing.  So the updater is being asked to pull
an image that only ever exists in this box's own store-and-container storage, and it fails
every single night.

Why it matters more than a nightly red line: the same unit is what keeps the *real* pulled
containers current (Home Assistant is deliberately pinned and left alone — see
`docs/home-assistant-on-services1-2026-11-16.md`), and a service that fails for a reason
everyone knows is benign is the reason nobody reads `systemctl --failed`.  One of these is
right, and it is a small change either way:

- **name the image locally** — `localhost/uosserver:<ver>` (podman will not go to a registry
  for a `localhost/` reference), which is probably what the module should have said anyway;
  or
- **tell the updater to leave that container alone** — `pullPolicy = "never"` on the
  container if that option exists at this nixpkgs pin; I could not confirm it (the deep
  `nix eval` of a `oci-containers` attr does not resolve against this flake, and guessing an
  option name into a config file is how a comment becomes a lie), so check with
  `nix eval --raw "nixpkgs#..."`/the module source before writing it.

Re-verify: `sudo systemctl status update-containers` (green at the next 02:00), and
`journalctl -u update-containers --since "-10 days" | grep -c "Failed to start"` going to 0.
Untouched: this is the UniFi **trial** box's container (`docs/unifi-os-server.md` owns that
story, including that the appliance is a downgrade of the running controller), and a
container change there wants its own look, not a drive-by inside a strata deploy.

## The wake tier: what has to be measured before any of it is switched on

`crates/voice-wake/` is deployed on services1 as `voice-wake.service` (loopback HTTP, model
pinned, proxied at `voice.int.leighhack.org/v1/wake/*`, selectable per tab from `whisper.int`)
— **detection only: it answers nothing and plays nothing**, and no camera feeds it yet.
The open items are the thresholds and the measurements, not the code: it scores WAVs with a streaming wake model
and prints peak score, detections per hour and CPU per audio-second. Architecture and engine
choice are in `docs/voice-assistant-2026-11-16.md` §"the wake tier"; its `README.md` owns the
numbers. Nothing is deployed, no unit exists, nothing listens. Open gates, in order:

1. **Real human recall.** piper is the only positive we have and piper is a bad proxy for a
   person (it is the voice Whisper returns as `K Jarvis`). Measured with piper: the isolated
   phrase **0.063** — a miss — even with real room audio around it, while the same phrase
   inside a sentence is **0.62**. A person saying "hey jarvis" alone, at camera distance, in
   a room at −44 dBFS, is the case that has to be scored before anyone believes a threshold.
   Re-verify: say it into cam8, `voice-wake score hey_jarvis.onnx clip.wav`.
2. **A busy hour, in this room, for the false-accept rate.** We have 240 s of a quiet-ish
   midday room: peak **0.031**, zero detections at 0.5. That is a baseline, not a rate, and
   the number that decides the feature is FA/hour while people are actually working (the
   upstream figure is 0.5–2.5/hour at threshold 0.5 on *conversational* speech — in a
   hackspace, tune it by measurement like Frigate's `min_area`, not by default).
   `just` recipe pending; the harness takes a directory of WAVs today.
3. **cam8's input level against the model's band.** Room captured at mean **−44.5 dBFS** /
   max −30.4; upstream's own positive fixture is mean −24.5 / max −6.0. Gain on a *clip* does
   not change the score (flat 0.061–0.071 across 42 dB), which is reassuring about the model
   and unhelpful about the mic — so if recall is poor, the answer is the camera's audio path
   (`rac`'s `agc`/`hpf`/`ns` are all `false`) or a server-side gain stage, chosen by
   measurement. Read `rac status` on cam8 first; it is the same interface that answered
   `aec: false` for the half-duplex decision.
4. **Concurrency.** ~**26 streams per core** is single-thread, single-stream
   (38.3 ms CPU per audio-second, identical at 1280- and 16000-sample calls, so it is
   inference and not per-call overhead). What N streams in one process actually cost — ONNX
   session-per-stream memory, thread contention, the ring buffers — is unmeasured, and that
   is the number "many more devices" hangs on.
5. **The backend trade, if we ever outgrow it.** We measure 38.3 ms/s where upstream
   publishes 16.5, because the Rust crate resolves to pure-Rust `tract` and gets a build with
   no ONNX Runtime blob. `nixpkgs#onnxruntime` is 1.27.1 and is the 2.3×-faster alternative;
   the price is a binary dependency in the derivation. Revisit only when stream count, not
   core count, is the limit.
6. **A second wake phrase (`computer`) needs a different route.** No published model for it,
   and upstream's training CLI wants CUDA/Apple-silicon plus their cloud TTS. The two local
   options are `nixpkgs#sherpa-onnx` (ships `sherpa-onnx-keyword-spotter`, takes a plain
   keywords file, no training, English model available) and `rustpotter` (trains locally from
   recordings). Nice-to-have; parked until the gates above say the first phrase works here.

---

## services3's Strata engine (the Pascal box) — measured 2026-11-22, `machines/services3/README.md` owns the table

Read off the live unit on 2026-11-22 (engine 0.1.63): `/proc/meminfo`, `sudo dmidecode -t
memory`, and four greps of `/var/lib/strata/strata.log`. **Nothing about the K/V tier crossed,
and nothing here claims it should**: `--kv-resident 32768` was written into the module on
2026-11-22 and reverted that day — `git log -- machines/services3/strata.nix` shows the two
commits with no deploy of this box between them, and the args the box runs are unchanged, so
that switch does not restart the engine. What is live is the evidence the greps gave: this box
already writes `1690 of the prompt path's 1690 lendable slots keep their experts in RAM too`
and `… (no file reads)`, which is why the two RAM dials services2 turns are inert here (see
the bullet below). **Blocks:** the K/V-tier prize stays unclaimed until someone runs a needle
and the answer key on this box and reads both lines back.

- **Two `MemTotal` readings sit 1 GiB apart and one cannot be true of this stick set** (found
  while answering "how much RAM is there to spend things in"). Today's read is `40999748 kB`
  (39.1 GiB) against `dmidecode -t memory`'s **2×4 GiB Kingston KHX2400C12D4/4GX + 2×16 GiB
  KTP9W1-MIE, four slots, all four full, `Maximum Capacity: 64 GiB`**. `machines/services3/README.md`'s
  Board/CPU row records `MemTotal: 41999736 kB` (40.05 GiB) on 2026-11-21 — which is *above* what
  40 GiB of DIMMs can report after reservation, so that figure is the suspect one, unless a stick
  was changed on the bench between the two reads and nobody wrote it down — **it was a stick
  swap, and this is now closed.** 2026-11-25, `dmidecode` on the box: A1 and B1 are
  **Corsair `CMK16GX4M4A2800C16`** 4 GiB (serials `00000000`), not the Kingston
  `KHX2400C12D4/4GX` this row used to claim, so the 4 GiB pair changed on the bench between
  2026-11-21 and 2026-11-22 and the 1 GiB `MemTotal` gap went with it. The same day,
  **DIMM_A2 (`KTP9W1-MIE` 16 GiB, serial `49216745`) was removed and manually tested: it is
  the proven-bad stick, and B2 (`54896945`) plus the 4 GiB pair are good.** A2 is
  deliberately **back in the box** with `machines/services3/bad-ram.nix` capping usable RAM
  with `mem=30G`, because every corruption this box has produced is at 31.338–38.316 GiB and
  the cap puts the whole window out of reach (the measurement, the 2026-11-23 boot that
  falsified a `memmap=` fence over the 272 measured pages, and the boot-check that proves
  the cap took are in `machines/services3/README.md`, "One stability fact…"). So the box
  reports about **29.5 GiB, not 40**, and every `MemTotal`/`MemAvailable` figure in these
  docs from before 2026-11-25 is an uncapped one.

  **Closed 2026-11-26 01:16:42 — the RAM is back.** The 272 measured pages are reserved out
  of the buddy allocator by a **patched kernel** (`machines/services3/bad-ram-kernel.patch.in`,
  generated from `bad-ram-pages.txt`), the `mem=30G` cap is gone (`infra.badRam.cap = null`),
  and the uncapped 2-hour run on that kernel came back `Status: PASS` —
  `130846592.00M in 7200.59s 18171.65MB/s, with 0 hardware incidents, 0 errors`, and 0
  `Hardware Error`/`Bad page`/MCE lines in that boot's kernel log. `MemTotal` went
  **29 840 212 → 40 998 676 kB (28.46 → 39.1 GiB: +10.6 GiB)**, and the engine's arena went
  **21.63 → 22.84 GiB with 1690 of 1690 lendable slots resident, "no file reads"** — the
  2026-11-21 full-40-GiB state, i.e. 1088 KiB of reservations is below what the arena
  measures in. **The cost that remains is compile time, not RAM:** a locally patched kernel
  has no binary cache, so every nixpkgs/kernel bump rebuilds `linux` and the NVIDIA module on
  this box (measured 92 min) and that build needs `strata` stopped to have the memory — the
  first attempt hard-stopped the box 30 minutes in with 1.5 GiB available. **Blocks:**
  nothing; receipts, the two-boot protocol and the arena ladder are in
  `machines/services3/README.md`, "The measured pages are now excluded by the kernel
  itself".
  **Why the capacity axis still matters:** 40 GiB → 64 GiB is a **purchase** of two 16s with
  the two 4s thrown away, not a rearrangement into free slots, and services2's board is
  *two* slots already holding 2×16 (`machines/services2/STRATA.md`) — so with no new RAM
  buyable, each box's only remaining capacity shape is which tier each thing lives in.

- **The RAM *dials* on services3 are at their ceiling, so two of services2's levers are inert
  there** (measured 2026-11-22: `1690 of the prompt path's 1690 lendable slots keep their
  experts in RAM too`, and the tier line ends `… (no file reads)`, `22.83 GiB` of arena page-locked,
  `MemAvailable` 10.8 GiB idle — **re-measured 2026-11-26 on the uncapped 39.1 GiB box: 22.84 GiB
  arena, 1690 of 1690, "no file reads", `MemAvailable` 11.1 GiB idle**, so the bad-RAM work cost
  the engine nothing it can measure). `--resident-budget-gib` and `STRATA_RESIDENT_HEADROOM_GIB` are
  **not set on services3 on purpose** — they trade file-read cost for headroom, and there are no
  file reads to trade. services2 needed both because its box is 30.7 GiB and the KV budget came
  *out of* the expert budget; the same dial there is a 2× prefill swing (562/725 → 1210/1204 tok/s
  at 8k). Do not copy services2's env line to this box "for parity"; the engine's own line is the
  proof, and the general rule is that a flag whose evidence is another box's numbers does not
  transfer (`docs/services.md`). **Blocks:** nothing. **To do:** nothing until the gate line stops
  reading N of N, which is the day they become real.

- **The conversation cache on services3 parks and never restores: `reused_kv_bytes = 0`**
  (measured 2026-11-22, `/var/lib/strata/strata.log`: `conversation cache sized from the free RAM:
  3584 MiB for up to 4 parked conversations` — README's 4096 MiB was the older start, so the number
  moves with the load — and `conversation cache: parked 4089 tokens in 240.1 ms; parked=2
  bytes=1139510732 evictions=0 snapshot_bytes=884619696 reused_kv_bytes=0`, at every start read).
  So ~1.09 GiB of host RAM is held for parked conversations and nothing has ever been read back
  from it, while the *other* mechanism does its job: `prompt 38569 tokens = 37990 reused + 579 read
  in 2022 ms … 4 checkpoints` is `--prompt-cache`. Same shape as services2's open item on prefix
  reuse existing on some path and not the one clients use (`docs/follow-ups.md`, "Unexplained, and
  it bounds what aibox's conversation-cache numbers mean") — **the two are probably one bug and
  should be read together before either is "fixed"**, and the fix is one line: `--pack` is not
  under `<data>/packs`, so the engine also prints `conversation cache: no SSD tier: no data folder
  (--conversation-spill-dir sets one)`. services3 is the only box in the network with both an
  SSD for that tier and 10.8 GiB of RAM to hold the bytes in; services2's answer there was "not
  with swap" (`machines/services2/STRATA.md`). **Blocks:** nothing. **To do:** read the reuse/restore
  path in `serve/server.py` and `include/strata/core/conversation_buffer.hpp` against the
  metrics card (`serve/server.py:834`), then decide between the spill dir, `--conversation-cache-mib
  0` (services2's choice, on principle: its RAM), and leaving it. Do not conclude "dead weight,
  delete it" from one start's counters — 2 parked conversations at a 4-chunk cadence is a small
  sample, and `parked=2 evictions=0` is as easily a cold cache as a broken restore.

- **`STRATA_PF_FUSED` / `STRATA_PF_SMALL` / the layer-major prefill walk: the engine says this
  very card is in the regime where the by-hand path wins, and nothing has been tried here**
  (measured 2026-11-22: `small expert cache: the default prompt path would read in chunks of 7936
  tokens (under 8192), so prompts are read the way that suits a card with little memory and, from
  two chunks, the layer-major walk (chunk 7936 tokens); STRATA_PF_SMALL=0 keeps the default path,
  STRATA_PF_FUSED / STRATA_PREFILL_LAYER_MAJOR set by hand win`). services2 deployed
  `STRATA_PF_FUSED=1` on 2026-11-14 on answer-key evidence (`machines/services2/STRATA-TUNE-2026-11-14.md`,
  8/8 before and after, revert one line + `just switch`). **Do not assume it is available on
  Pascal**: the *fused int8* prompt kernels are sm_80-gated and this card cannot have them
  (`docs/work-log-2026-11.md`), so whether the knob is honoured here at all is a measurement —
  the house example of a flag that is accepted and half-honoured is llama.cpp's flash-attention
  line on this same box (`machines/services3/README.md`, "both wordings this box prints").
  **Blocks:** nothing. **To do:** measure both arms as `env.*` keys in `./strata.nix` — they are
  read at start, no rebuild — with the harness at `machines/services2/strata-tune/` pointed at
  `/srv/strata` and `strata3.int` first (`follow-ups.md`'s note above is that its numbers mean
  nothing until), one arm at a time (that doc's noise floor is real: a control pass once read
  807 tok/s, so under 10 % is nothing), and gate quality on pi's usage rather than on
  `strata-answer-score.py` — nine greedy items cannot see KL 0.006.

## The streaming voice turn (built 2026-11-22): shipped in the tree, unmeasured on the network

`docs/voice-assistant-2026-11-16.md` §"Streaming end to end" and
`crates/voice-assistant/README.md` §"The turn is a pipeline" own what was built and how it
fails: rolling ASR (`asr.rs`), the engine prefill (`POST /v1/ask/prefill`, and the device-facing
`POST /v1/warm`), the streamed answer (`POST /v1/ask/stream` → `/v1/turn`), the sentence
splitter (`sentence.rs`), the resident piper (`tts.rs`) and the SSE plumbing in both directions
(`common-rs/web/src/stream.rs`, `common-rs/http/src/stream.rs`). The overlap is **tested and
falsifiable** (`turn::tests::the_first_audio_arrives_while_the_model_is_still_answering`,
verified to fail when the voice is slowed past the model). What is not done is everything that
needs a running box.

1. **The first-audio number is read** (2026-11-22, services1): first token 10 534 ms, **first
   audio 11 317 ms**, answer complete 13 320 ms, and **178 ms a sentence** from the resident
   piper — the table is in `docs/voice-assistant-2026-11-16.md`. Two bugs turned up in the same
   ten minutes, both fixed and pinned by tests verified to fail: the warm-up refusing its own
   turn (1b), and a fast-path answer that streamed no text and therefore spoke nothing
   (`turn.rs` hands the `answer` text to the same splitter when nothing streamed).
   **Still unread: `asr_tail_ms` on real audio.** Both measured turns arrived as text, so
   rolling transcription has run against a fake whisper at 250 ms a slice and never against
   aibox's real one. Someone has to speak to `cam8` and read `asr_tail_ms` out of `/v1/turn`'s
   `done` event or `satellite-status.json`: the audio half of this rests on the assumption that
   a slice of a bursty Wi-Fi window is short enough to matter, and that is a number, not a
   design.
1b. **The warm-up is off by default, with receipts.** Deployed switched-on, it logged
   `prefill: services2 (RTX 3060) cached 0/4079 prompt tokens in 4629 ms` and then failed the
   very turn it was warming (`another turn is in flight`), because `/v1/ask/prefill` and
   `/v1/ask/stream` share the listen socket's one turn lock. Fixed in three places
   (`turn.rs`'s `warm_skip_reason`, `turn_gate::acquire_agent`'s bounded wait, and
   `warmEngine = false`). The number that would justify switching it back on is
   `prefill_ever_hit: true` in the bot's `status.json` **and** utterances long enough that a
   prefill lands before the speaker stops; nobody has watched that pair yet.
2. **The browser page still calls `/v1/ask`.** `whisper.int`'s Assistant card would show the
   transcript growing and start playing mid-answer — the fastest path in the system, since its
   client-side wake means the question is already text (`{"text": …}`, no whisper at all). The
   blocker is mechanical, not architectural: `crates/whisper-ws/web/app` is a Vite/Solid build
   whose npm deps are not in the offline cache, so the edit needs the build to be reachable, and
   `build.rs`/`gen_assets.sh` mean an unbuilt `dist/` silently serves the *old* bundle
   (`AGENTS.md` rule 19). Contract: `docs/esp32-voice-satellites.md` §2.2b.
3. **The satellite's live path has never run on the camera.** `satellite.rs` now feeds the
   rolling ASR from `Gate::take_open_samples` and fires a warm-up on the first partial, and that
   whole path is unit-tested against fake HTTP servers only. The first time it runs against a
   real whisper it will produce a number nobody has seen: `asr_tail_ms` per window in
   `satellite-status.json`. Read it once before believing rolling transcription helps on a
   device whose audio arrives over Wi-Fi in bursts, and watch `warms` — a warm-up fired on a
   *noise* partial is a wasted engine request, which is harmless but not free.
4. **Concurrency is now the only wall.** One warm piper behind a mutex, and
   `slack-agent listen` still 429s a second caller (`crates/slack-agent/src/listen.rs`).
   Streaming makes a turn feel faster; it does not make the assistant multi-way. Before more
   than a couple of satellites exist, someone has to choose the queue policy (last one wins,
   first one wins, or say "one at a time" out loud) — `docs/esp32-voice-satellites.md` §2.2.
5. **`turns` counters and the warm-up receipt are new `status.json` fields, and nothing
   monitors them.** `common/monitoring-endpoints.nix`/gatus watch endpoints, not numbers inside
   JSON. A `voice_turn_first_audio_seconds` gauge from the gateway's own `/metrics` would be the
   honest version of "is the assistant getting slower", and would have caught the `cached 0`
   failure mode that the round-trip test caught only because someone wrote a test for a field
   nothing read.

---

## The space calendar (services1) — opened 2026-11-23, `docs/space-calendar.md` owns it

- **HA's long-lived token went into a transcript on 2026-11-23** while reading
  `/var/lib/hackspace-api/env` (the credential the Space API needs), and is therefore
  burned. What happened: the intent was "does this file have the key, without printing
  the key", and the idiom written for it — `sed 's/^HOMEASSISTANT_TOKEN=/*/p'` — masks
  only the **name**: the whole token body follows the mask and lands in the transcript.
  A long-lived Home Assistant credential is a write-capable one over HA's own API, so
  it is not "a token that only reads the space status". Blocking: the rotation itself,
  not any discovery. Rotate by minting a new HA long-lived token and rewriting that
  `0600` root file **in one action** host-to-host (`docs/space-calendar.md` step 5, and
  the same reason `machines/services1/services/hackspace-api.nix`'s header gives for the
  file existing at all: the unit refuses to start without it). Ask a *question* of the
  file rather than printing it (`grep -c '^HOMEASSISTANT_TOKEN='`, or
  `${#HA_TOKEN}` after taking it into a variable). Re-verify with:
  `/api/` on `127.0.0.1:8123` with the new token → `200`, and
  `curl -fsS http://127.0.0.1:8081/space.json` → still `api: "0.13"` with its seven
  temperatures, because a broken rewrite fails the Space API silently as a feed with
  missing sensors rather than a crash.

- **Nothing found can change an existing calendar row on this build** — **superseded
  2026-11-24: it can, and the probe below could not have found it.** Core registers
  `calendar/event/create|delete|update|subscribe` as **WebSocket commands**
  (`components/calendar/__init__.py:308/346/393/442`) and `get_services_for_target` does not
  see WebSocket commands at all; all three write commands are live on the running
  `2026.11.0` container (probed with deliberately bogus payloads, `.ics` md5 unchanged), and
  `local_calendar` implements create/update/delete. Receipts and payload keys:
  `docs/events-sync-plan-2026-11-24.md` §2 row 1. **Keep the habit, drop the conclusion: name
  which of HA's three doors a measurement used** — a service, a WebSocket command, or the
  entity's declared `supported_features` — because the first cannot answer for the other two.

  (measured 2026-11-23,
  `get_services_for_target` on `calendar.public_events` offers exactly
  `calendar.get_events` and `calendar.create_event`). `local_calendar`'s entity declares
  `CREATE_EVENT | DELETE_EVENT | UPDATE_EVENT`, but only the two above are registered
  **services**, and the Assist/dashboard routes that reach the entity methods
  (`/api/assist/debug/*`) answer `404: Not Found` on the pinned `2026.11.0` core. So
  "move this event to another date" currently means adding a second event and leaving the
  old row, or editing the file and restarting HA — which the same doc says not to do.
  Blocked on: the pin, not on us — `HA_IMAGE` in
  `machines/services1/services/home-assistant.nix` only moves when someone bumps it
  deliberately. **Probe `get_services_for_target` again after any bump before inheriting
  this sentence.**

- **Home Assistant's authentik OIDC client secret sits in plaintext in
  `/srv/homeassistant/configuration.yaml`** (`auth_oidc.client_secret`), a runtime file the
  flake only mounts, and it is now in a transcript: reading that file to ask *whether* it
  holds a credential printed the credential, because the idiom written for "print it masked"
  filtered on `api_key|token|password` and `client_secret` is none of those (rule 13, same
  shape as the 2026-11-23 HA-token item above — the asking-a-question idiom has to be
  `grep -c`, never `grep`). It can mint HA logins through authentik, so: rotate the
  `home-assistant` application's secret in authentik, and move the value out of
  `configuration.yaml` (a sops file read with `!env_file`, matching what
  `machines/services1/sops.nix` already does for everything else), leaving the OIDC provider,
  discovery URL and claims where they are. **Re-verify with:** a fresh `curl` of
  `ha.int.leighhack.org` → authentik login → a signed-in HA session, and
  `sudo grep -c 'client_secret' /srv/homeassistant/configuration.yaml` → `0`.

- **The events plan is open work, and this file is its index**: `crates/events-sync` (an
  Eventbrite → Home Assistant mirror), the HA automation that makes HA drive the Slack
  event notices it does not drive today, and the website's `/events` (an engaging public
  listing) vs `/calendar` (the combined view). Phase-by-phase, with the decisions that need
  an owner before anything is deleted: **`docs/events-sync-plan-2026-11-24.md`**. The two
  blockers are a credential with a named owner (§7.1) and permission to retire the
  hand-mirrored calendar rows (§7.3); nothing in it needs a front-door row until phase 5.

## The Thread border router and the Matter controller (services1) — planned 2026-11-26, `docs/thread-border-router-2026-11-26.md` owns the evidence

- **The radio does not exist yet**, and the dongle this network was offered is not
  the radio: a Sonoff ZBDongle-P is a Texas Instruments CC2652P, and upstream's own
  issue tracker calls the correct firmware on the correct dongle "not working when
  joining device" (`RadioSpinelNoResponse`). **Blocks:** `infra.thread.enable`, which
  the module's own assertion refuses while `common/radio-dongles.nix`'s thread-rcp
  row has no device. **The way forward is a purchase, not a patch:** an EFR32MG24-class
  dongle (256 KB RAM / 1536 KB flash against the CC2652P's 80 KB / 352 KB) and `just
  radio-plug` to name it once it is in a USB port. `radios/firmware-pins.json` carries
  its firmware, pinned by digest, and `just thread-fw-fetch` is the verb.
- **What the spare *is* good for, if the Thread radio has to be bought: the Zigbee mesh.**
  A CC2652P is a working 802.15.4 radio — it is Thread-certification and RAM it lacks, not
  radio-ness — so flashed with a **Zigbee Router** firmware (a repeater: no `pan_id`, no
  Z2M instance behind it) it joins the network the live SLZB-06M runs and extends it,
  which is the cheap move for a mesh with weak link quality. `common/radio-tables.nix`
  has the role (`zigbee-router`) and its rule (it answers on one of the mesh's own
  channels — it joins that network, it does not start one — and it has no `mountAt`, because
  no stack talks to it). `radios/firmware-pins.json` deliberately does **not** pin a
  Zigbee-Router artifact yet: the row would name something nobody has verified, and the
  measured point *against* the repeater is a 150-device house that had spare Sonoff sticks
  flashed and plugged in around the network and still had ~50 nodes at bad link quality,
  fixed by moving the coordinator to the middle of the house instead. So: a cheap option,
  not medicine, and this space is one room with one coordinator in it. Decide it against
  the mesh's own numbers (`crates/zigbee-network/` exists for exactly that read), and
  remember that a flash does not change a dongle's USB vendor/product ids — a flashed
  Sonoff still answers as a Sonoff Zigbee dongle in `/dev/serial/by-id`, which is why the
  table's **role**, not the device name, is the fact that survives a flash. **Blocks:**
  nothing; it is a decision about a spare dongle, and a flash is a one-way door per dongle.
- **This box's IPv6 is not Thread-ready, and the two upstream docs disagree about
  what to do about it.** Measured 2026-11-26: `net.ipv6.conf.vlan225.forwarding = 0`,
  `accept_ra = 0`, `accept_ra_rt_info_max_plen = 0`. The OpenThread guide says set the
  first; the Matter server's own OS-requirements page says do **not**, for a networkd
  setup, and pin `PRIMARY_INTERFACE` instead — which is what `matter-server.nix` does
  today. **Blocks:** a Thread mesh that commissions and is then unreachable. Nothing
  here has changed a sysctl, and the question belongs to **gw's NAT rules and the switch's
  VLAN configuration**, not to a container: Thread's RA advertisements arrive on the
  interface that carries the space's leases, and Matter's own caveat about "not across
  different LANs or VLANs" has never been measured against VLAN *tags* on this hub.
  A network shaped like ours answers the first half of it: a VLAN-aware setup on Proxmox
  and VyOS (`br100`, `br100.10`, `br100.20`) found Router Advertisement **disabled on the
  intermediate hosts** and set `accept_ra = 2`, `accept_ra_rt_info_max_plen = 64`,
  `forwarding = 1` on every interface **and** every VLAN-tagged pair before the Thread
  routes reached the clients (`vanwerkhoven.org`, 2026) — so forwarding belongs to the
  *middle* of the chain and RA acceptance to the *client* end, which is exactly the shape
  the two upstream docs seemed to disagree about, and 64 is the RIO prefix length every
  working recipe names. Ours differs in one way that matters: the sysctl keys on both
  boxes answer for `vlan225`, `vlan226`, `router-lan`, `router-uplink` … as **plain named
  interfaces** (`ls /proc/sys/net/ipv6/conf/`) with **no `iface/tag` key**, because our
  VLAN tags are `slide` links with their own names rather than kernel VLAN pairs — so that
  recipe lands here as one row per named interface, and the pair half of it does not
  apply. The same write-up names a second half of the question this repo has not
  considered: Thread's discovery is **IPv6 mDNS**, and "mDNS does not travel from zone to
  zone but instead to the router and is then reflected outward again" — their VLAN-aware
  network needed an explicit **mDNS reflector** plus firewall rules from every zone *into*
  the LOCAL zone, "two hours of my life I won't get back". Nothing on this network reflects
  mDNS (`gw` runs dnsmasq: DNS, DHCP, the NAT sets). So crossing our VLAN tags may be
  **two** questions, and neither is guessable from the container.
- **Commissioning has no Bluetooth path on this box.** `/sys/class/bluetooth/` is empty
  and `lsusb` has no Bluetooth device, so the Matter server's `BLUETOOTH_ADAPTER` option
  is not available; `BLE_PROXY` is, and needs a device — an ESPHome host for the `/ble`
  bridge. **Blocks:** Matter commissioning of anything, Thread or Wi-Fi.
- **The border-router image has no version tag** (measured through the registry on
  2026-11-26: `latest`, `edge`, `edge-<sha>`, three `sha-<hex>`), so the flake pins a
  moving tag and the digest that tag resolves to is off-flake. `just thread-pins` prints
  the tag list so the drift is at least a number someone can read. **Blocks:** nothing;
  it costs re-pinability. Home Assistant's own app image *does* have a pinnable tag
  (`3.4.0`, and `stable`), so switching to it is the way out if this one drifts.
- **`NAT64 = "0"` is a decision without a measurement behind it.** MQTT-over-Thread needs
  NAT64 and the space runs an MQTT broker; nothing has yet asked a Thread device to reach
  it. **Blocks:** nothing today; it is the next thing that will be wrong.

