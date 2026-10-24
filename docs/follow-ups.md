# Follow-ups — the live backlog

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps a short list of the ones
that an agent can trip over or fix in passing; this file keeps each item with the
context that makes it actionable. **Add an item here (with a date and what blocks
it) rather than growing `AGENTS.md`** — that is what keeps the rules file a rules
file.

Narrative of work already finished: `docs/work-log-2026-10.md`.

---

## Nix / build

- **Evaluation still reads a derivation: finish the crane half of "Evaluation must not
  build"** (found 2026-10-22, on gw, by accident: `just check` began failing with
  `Cannot build '/nix/store/7ac7180r…-Cargo.lock.drv'` — `local builds are disabled
  (max-jobs = 0)`). **Blocks:** any `just check` on gw, so the router currently has no
  flake-level gate; nothing else is blocked, because the four assertion evals and
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
  `common/build-farm.nix` writes `/etc/ssh/ssh_known_hosts`, and a trivial delegated
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

  **Its sibling is the item at the end of this section** — "`just check` is
  machine-dependent", where the read is `builtins.readFile (config.sopsSecretText …)` and
  what gets compiled is a *decrypted secret*. Same class, two doors, and they fail in a fixed
  order: gw dies on `Cargo.lock.drv` before the secret one is reached, and aibox passes
  only because those `sops-secret-*` outputs are already in its store — so that item's
  "the documented fallback has one leg" now reads "has one leg, and it is held up by store
  state". Whoever fixes either should fix the class: no `builtins.readFile` on a
  derivation in a module, which is the same sentence `common/crane.nix` was written to
  obey.

- **The door entry package cannot be built by the builder, and it is not reproducible**
  (measured 2026-10-22 while proving `inputs.door-entry-management-system`'s new
  `nixpkgs.follows` edge; **both facts are older than that change** — the same
  attributes are on the pre-change drv in services1's store and the post-change one on
  aibox, because they come from the package, not from nixpkgs).
  `door-entry-management-system` sets **`__noChroot`**, so aibox's daemon — whose
  `sandbox` is `true` — refuses the derivation outright:
  `error: derivation '…-door-entry-management-system-1.1.9.drv' has '__noChroot' set,
  but that's not allowed when 'sandbox' is 'true'`. services1 and services2 run
  `nix.settings.sandbox = "relaxed"` (`machines/<m>/configuration.nix`) and so build it
  locally, which is why it has always worked and why `just check-build` **on aibox**
  stops on it: that failure is this item, not your change. Reproduce it with
  `nix build --no-link ".#nixosConfigurations.services1.pkgs.door-entry-management-system"`
  (it needs no system build) and prove it builds with `--option sandbox relaxed` — that
  succeeded in 37 s at the current lock. Second fact: `--rebuild` on the already-valid
  drv prints `may not be deterministic: output … differs`, so this package's output path
  moves every time it is built and "identical drvPath / identical output" is not a proof
  that includes it. Two options, neither taken here: find the non-determinism upstream
  and fix it, or stop `just check-build` from promising more on aibox than aibox can
  build (and say so in the recipe's own comment).

- **gw now delegates its builds to aibox and has no local fallback (added
  2026-10-21, deployed to gw the same evening).** Until today gw was the one machine
  that imported
  neither half of `common/build-farm.nix`: `/etc/nix/machines` did not exist, so every
  build — including the accidental kind an agent reaches for — compiled on the router.
  `machines/gw/default.nix` now sets `infra.buildFarm.builderHost = HOSTS.aibox` **and
  `nix.settings.max-jobs = 0`**, and imports the module with `infra.buildFarm.gc = false` (the
  router delegates; it does not collect garbage on a timer).

  The `maxJobs = 0` is the half that makes the rule mechanical. `nix.distributedBuilds`
  only makes nix *prefer* the external builder and falls back to local silently — a
  builder alone leaves the failure mode in place, just rarer. With zero local jobs there
  is nothing to fall back to, and substitution is unaffected (copying a path in is not
  building one), so `just switch-gw`, the closure a switch needs, and every auto-rollback
  keep working with aibox unreachable. `_build-host-guard` still refuses the build-shaped
  recipes on gw: **evaluation is still paid on the box that answers DNS for the network**,
  and a guard is cheaper than an explanation afterwards.

  Three things to do or decide once it is deployed:
  1. **Prove the delegation, and prove the failure.** From gw, build something
     cache.nixos.org does not have (`nix build .#whisper-ws`) and watch aibox's
     `journalctl -u nix-daemon`; then stop aibox and repeat — it must fail, not compile.
     Nothing about this change is testable from the config alone.
  2. **The emergency path is now narrower, and its escape hatch is untested.** If aibox
     *and* services1 are both down, `nixos-rebuild switch` on gw cannot build. The
     intended override is a trusted-client `--option max-jobs N` (leigh-admin is in
     `trusted-users`), but nobody has confirmed the daemon honours it for that setting —
     find out on a good day, not during an outage.
  3. Revisit whether the guard should now allow `just switch` on gw. The argument for: it
     cannot compile anything any more. The argument against: an evaluation storm on the
     resolver, and `INFRA_ALLOW_BUILD_ON_ROUTER=1` is one environment variable away
     from whatever the next person needs.

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
  aibox, and locally use the four standalone assertion evals
  (`machines/gw/lib/check-network-tables.nix`, `common/check-printer-states.nix`,
  `machines/services1/lib/check-config-to-gitlab.nix`) plus `nix eval` of the option
  you changed. The durable fix is the one the crane header already prescribes — hand
  the SPA builder the source by path and never let it read a derivation.

- **`nix flake check` is not warning-free: two findings, both reachable now**
  (noticed 2026-10-14 while checking the `gw` machine; re-read on 2026-10-15 during
  the network upgrade). Checking `nixosConfigurations.services2` emits
  `evaluation warning: Grafana passwords will be stored as plaintext in the Nix
  store! Use file provider or an env-var instead.`, which by rule 9 is a finding:
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

- **aibox is the builder now; prove the hop and watch its disk** (2026-10-17, with
  the move off services2). Two things are open, neither of them a switch:

  1. **The delegated build itself.** `common/build-farm.nix`'s hop was last proven
     against services2 on 2026-10-12 (a delegated build and a `--max-jobs 0` build,
     no SIGKILL sweep). The key, `sshUser = "leigh-admin"` and the
     `trusted-users` entry are box-agnostic, and aibox already has
     `~/.ssh/agent-hop-key`, but run `just build-remote` once from services1 and
     read the builder name in the log — a silent fallback to local is the failure
     mode the rule about polling exists for.
  2. **aibox's root and the parked model data.** aibox measured 82 % full on
     2026-10-15, while services2 had 211 GB free; the builder is the box that now
     grows `/nix/store`. Strata's data is deliberately outside the store and is
     **not** removed by disabling the service: the IQ2_XS GGUFs under
     `/home/leigh-admin/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF` (~68 GB) and the
     pack/MTP under `/home/leigh-admin/Strata`. Deleting them is a manual decision
     — do not automate it from the flake — but until then the builder's headroom
     is smaller than the role assumes. `common/build-farm.nix`'s GC policy is what
     keeps it from being a surprise.

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
  change — say so, and say what you verified against a clean HEAD.


## Strata (services2)

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

## Machines / access

- **services2 cannot hop out.** `machines/services2/README.md` used to claim the
  shared machine-hop key was installed there; it is not. Only `authorized_keys` is
  (by `common/users.nix`), so you can hop *in* to 10.3.1.21 but it cannot ssh to
  services1, and it has no GitHub key of its own. Fetch there with `ssh -A` (agent
  forwarding — `AGENTS.md`, "SSH between machines") rather than copying a private
  key around. Install `~/.ssh/agent-hop-key` deliberately if it should hop out;
  never make it a side effect of a deploy.

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

## DNS / monitoring

- **The router's DNS is three generated files now, so `common/int-dns.nix`'s `legacy`
  set is a to-do list, not a design** (added 2026-10-22). `nginx.int`, `mqtt.int`,
  `tailscale.int`, `gitlab.int` and `ldap.int` answer with the front door and are served
  by no enabled vhost; each carries its reason in the table. `gitlab.int` disappears by
  itself when the gitlab module is re-enabled (the guard moves it from `legacy` to
  `names` by refusing the other way). `mqtt.int` and `tailscale.int` want someone to
  check what dials them and then delete the records. `nginx.int` stays — it is the CNAME
  target of every public `*.int` record in DigitalOcean. And `ldap.int` belongs with the
  authentik decision below, not with DNS.

- **`nginx.int`'s AAAA record in DigitalOcean is a ULA** (`fd99:dead:beef:225d::20`,
  while `nginx.leighhack.org`'s AAAA is the public `2001:8b0:1d14:225::20`). So the
  public `*.int` CNAMEs hand any client outside the building one private IPv4 and one
  private IPv6, which is coherent for a LAN zone and useless as a remote entry point —
  if anything is expected to reach the internal apps off-LAN, it should be through the
  tailnet (`int.leighhack.org` split-DNS → 10.3.1.1) and this record should point at the
  public address or go away. Decide before pruning the `*.int` CNAMEs above.

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

- **`firewall.int.leighhack.org` proxies a port nothing listens on** (found 2026-10-24
  while planning the front door). `machines/services1/http.nix` sends it to
  `https://10.3.1.1:60443`; `ss -lntp` on gw answers 53, 22, 8091 and tailscaled, and
  `60443` appears in the whole tree exactly once — as that `proxyPass`. The retired
  appliance's admin UI was deliberately never reopened (`machines/gw/README.md`, and
  `firewall.nix`'s ssh-rule comment), so the vhost is a 502 today and the row in
  `common/int-dns.nix` is a name the front door answers for an app that does not exist.
  Either something is meant to be listening on 60443 — in which case say what, and on
  which interface, because "the router's admin panel" is exactly what its firewall
  audit concluded it should not have — or the vhost, the `firewall` label and the
  `*.int` record all go. Not carried into `machines/gw/http.nix` in the meantime, which
  is the difference between inheriting a dead upstream and choosing one.

- **The front door, phases 2 and 3: everything the migration doc now commits to.** Read
  `docs/router-front-door-migration.md`'s decision and its phase 2/3 lists; the short
  version is that `gw` ends up terminating *all* of it — the full certificate (services1
  with no `security.acme` at all), the public names, `nginx-sso`, `anubis`, the
  `LOCAL_NETWORK` ACLs — and services1 keeps an **origin** nginx: no cert, no gate, no ACL,
  reachable from `10.3.1.1` alone. The five pieces that must land together, and the two the
  order of them exists to protect:

  * `common/front-door.nix` + `common/nginx/front-door.nix`: one row per name the door
    serves (`origin` | an address | `local`), with the per-name policy (`sso`, `anubis`,
    body sizes, timeouts, aliases). Vhosts leave the service modules because a vhost is now
    one box's business; leaving them in place is how two boxes keep opinions about one name.
  * `common/public-names.nix` and its guard. The public names have **no table today** — that
    is why `docs/router-and-dns.md` can narrate "two live vhosts and no DNS record" — and
    the guard must check only what an evaluator may know: uniqueness, one upstream per name,
    the `*.int` subset agreeing with `common/int-dns.nix`. "Does the DigitalOcean record
    exist" is a gatus check; making it an eval-time fetch would put the network in the
    evaluator (AGENTS.md: evaluation must not build, and here it must not talk either).
  * The **origin-reachability rule** on `gw`'s forward chain, plus the origin's own
    `allow 10.3.1.1; deny all;`. This is the piece phase 3's safety rests on: three apps
    assert *who you are* from `X-WEBAUTH-USER` because loopback means no request can arrive
    without passing through nginx (`crates/switch-gui/src/routes.rs` says so about its own
    field), and the same reasoning refuses `--no-auth` on a routable bind. It ships before
    or with the first moved vhost, never after.
  * The gate and `anubis` moving, with `listen.addr = "127.0.0.1"` on the gate once it is
    next to the nginx that calls it (it answers `*:8082` on services1 today),
    `users.users.nginx.extraGroups = [ anubis ]` on `gw` or its unix socket is a 502, and
    `anubis`'s `TARGET` retargeted from `127.0.0.1:8004` to `10.3.1.20:8004`.
  * Phase 3 step 5's deletions on services1 — `security.acme`, `useACMEHost`s, the sso unit,
    `machines/services1/int-dns.nix`, the `*.int` vhosts — **and the gatus endpoint guard
    with them**, since `services/monitoring.nix` derives the internal endpoints from the box
    own `virtualHosts`.

  Two audit items fall out of the same change, both owed before the flip rather than after:
  every app that logs, rate-limits or ACLs on the peer address (after the flip its peer *is*
  the router, and `X-Forwarded-For` is only evidence if `set_real_ip_from` names the router
  and nothing else — the module option does not exist in this nixpkgs, so it is
  `extraConfig`), and every container whose `oci-containers.ports` entry publishes on
  `0.0.0.0` while its only intended client is nginx (`127.0.0.1:PORT:PORT` fixes the ones
  that are nginx-only; the UniFi controller's and mosquitto's ports have real consumers on
  the LAN, so read the service's doc before narrowing anything).

- **Deployed 2026-10-21 21:19** — the state to compare anything against. gw is at
  `zb8s89v5w8ir5nv86i6kb3ghlsa4qh9y` (public IPv6 inbound with its counter, the tailnet
  /10 in every rule, dnsmasq's four upstreams, the aibox build delegation and
  `max-jobs = 0`) and services1 at `38zsm0pnmb9bwd7i58wqp2rlrnp0wh59` (the same ACL in
  nginx and `pg_hba`), both switched *and confirmed in the same shell invocation*, both
  `is-system-running = running`, `filestore.int` 200 / `id` 302 / `network-info.int` 200
  afterwards. The new v6 rule read `counter packets 3` minutes after activation and 12 a
  few minutes later — which proves the rule matches traffic and **not** that the internet
  can arrive, because LAN clients hit it too; the off-network probe is still owed. Still
  pending by choice: `--advertise-routes` (a boot, plus the services1 overlap settled),
  the VPN, the rotations, and services2/aibox picking the ACL up.

- **`just switch-gw`'s `sync-gw` rsyncs the router's `.git` as well, and `--delete`
  means it.** Seen on the deploy above: 29 deletions, every one of them gw-only git
  metadata — a local branch and the records of three stale worktrees. The excludes are
  `target/`, `node_modules/`, `/result*` and `/.git/config`, so the rest of `.git` is
  reconciled against services1's copy. Two consequences: **a commit or branch made in
  gw's checkout is not merely unsynced, it is on a collision course**, and the router's
  repo can be left referencing objects a later sync deleted. Nobody has hit the
  corruption; the branch it ate was this audit's and the commits were safe on services1,
  which is the only reason this is a paragraph and not an incident. Treat the mirror as
  files (`--exclude /.git`, and fetch on the rare occasion the router needs to be a repo)
  or stop describing it as a checkout.

- **Public IPv6 inbound was never in the flake — fixed 2026-10-21, but unverified
  from outside.** The old box let the internet dial services1 over IPv6
  (`pass in quick on l2tp0 reply-to (…) inet6 proto tcp from any to
  <SERVICES1_IPV6> port = {http, https, 8522}`, read out of the archived `pf.txt`
  on 2026-10-21) and every `*.leighhack.org` vhost publishes a AAAA — `nginx.leighhack.org`
  is the CNAME they all share and it answers `2001:8b0:1d14:225::20`. `firewall.nix`
  had no v6 service rule at all: its forward chain is `policy drop` with one
  `ip6 saddr <ACL>` accept, so an internet v6 client was dropped for four days while
  every IPv4 test passed, and the only v6 traffic that got through was Chris's
  `2a02:8010:6680::/48` — which passes *because it is in the shared ACL*, not because
  a rule allows the service. `publicV6ForwardRules` is the ported rule; the two
  addresses are `constants.nix`'s `PUBLIC_SERVICES1_V6`.  The post-cutover audit on
  2026-10-21 narrowed it from the old box's {80, 443, 8522} to **80 and 443**,
  matching the v4 policy (web plus tailscale only).

  What this still owes someone: **a probe from a v6 address that is not in the ACL.**
  A LAN `curl -6` proves nothing (its source matches `2001:8b0:1d14::/48`), and the
  external-v6 hits in services1's nginx access log stop at 08:46 on 2026-10-21, so
  the pre-cutover behaviour is inferred from pf rules rather than observed. Read
  `nft list chain ip6 router6 forward` and its counters after a probe from, say, a
  phone on a mobile network. One deliberate non-change: 3478 stays v4-only because
  the old box never let it in over v6 (so headscale's DERP has a published IPv6
  with a v4-only STUN socket — worth a look if DERP ever needs to be reachable over
  v6); 50443 and 51820 were removed entirely on 2026-10-21 (see `firewall.nix`), and
  `yunohost.leighhack.org` has no AAAA so the appliance has no v6 row.

- **The tailnet's advertised routes are not applied, and a hand-typed `tailscale up`
  outranks the flake** (measured 2026-10-21). `machines/gw/uplink.nix` advertises
  `10.3.0.0/16,192.168.2.0/24,2001:8b0:1d14::/48,fd99:dead:beef::/48` and
  `--advertise-exit-node`; `sudo tailscale debug prefs | jq .AdvertiseRoutes` on the
  box answers `null`. `extraUpFlags` reach the daemon only through
  `tailscaled-autoconnect.service`, which runs **at boot** — and at the cutover boot
  that unit timed out and failed (`16:36:22 State = NoState` → `16:37:53 Failed`), the
  join was done by hand, and a manual join is never re-asserted by a later `switch`.
  It also registered as **`gw-1`** (see the stale-`gw`-node item below, now confirmed
  rather than suspected). Before applying it: services1 advertises
  `10.3.0.0/16, 2001:8b0:1d14::/48, fd99:dead:beef:225::/64` *imperatively* (the
  `tailscale set` comment at the top of `machines/services1/services/headscale.nix`),
  which is unequal to this box's `/48` — pick one advertiser or make the two lists
  identical, then reboot and check the prefs, not the unit's state. Whether headscale
  approves the routes at all is still unknown from this side.

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
  single-target, so `firewall.nix`'s static marks are parity, not a regression); the
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
  name that answers with services1's three addresses, `machines/gw/dns.nix` renders it to
  `/etc/dnsmasq-int-vhosts.hosts`, and that store path is a dnsmasq `restartTriggers`
  entry, so record and restart arrive in one activation. The gate that makes "forgot the
  DNS" impossible to land is `machines/services1/int-dns.nix`: it derives the `*.int`
  names from `services.nginx.virtualHosts` and refuses to evaluate services1 unless that
  set equals the table (it caught three names of its own on its first build, which is the
  argument for it). The crate, its workspace membership, `machines/services1/dns-sync.nix`
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
  used to run Windows; `machines/gw/README.md` has the install and the layout, and
  `docs/gw-cutover-attempt-2026-10-21.md` has the post-cutover findings (missing
  `*.int` records, no inbound NAT, dnsmasq's RA timer stopping after a LAN link flap)
  and the last four open items below. The `10.3.1.254` co-existence address is history.

- **dnsmasq stops sending *unsolicited* RAs after a LAN link flap** (found
  2026-10-21, on the cut-over box after the trunk was re-cabled; the trap and its
  checks are in `machines/gw/README.md`, "The third way dnsmasq breaks the LAN
  silently"). Solicited RAs keep working, so a probe hides it; clients lose their IPv6
  default when the 1200 s router lifetime expires, and a host that forwards (services1)
  never takes the RA default at all and so cannot reply to VLANs 226/227. The fix is a
  `systemctl restart dnsmasq` after the flap — there is no config change that restarts
  the timer. **Open**: a link-flap hook (or moving RAs to radvd, which re-advertises on
  link-up) so this cannot be missed by whoever is not watching at the time.

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

- **A stale headscale node called `gw` may own the name** (noticed 2026-10-17;
  **confirmed at the cutover**). `sudo headscale nodes list` on services1 listed a `gw`
  at 100.64.0.11, registered 2025-08-25 and offline since 2026-04-13 — some earlier life
  of this box, or of the old router's tailnet client. Nothing in the flake references
  that address, but headscale's node names are what ACLs and humans read, so a second
  `gw` is a finding either way: rename or expire the old node at the cutover, and
  check `tailscale status --json` on the new box says it advertises no routes. **It
  happened as predicted**: the live box registered on 2026-10-21 as `gw-1` (id 56,
  100.64.0.28) because id 15 still holds `gw`, and the check —
  `sudo tailscale debug prefs | jq .AdvertiseRoutes` — is the one that found the
  advertise-routes item above. Expire id 15, rename id 56, and read the prefs rather
  than `status`'s `AllowedIPs`, which hides an unapproved advertisement.

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
  1. `common/config.nix`'s `LOCAL_NETWORK` contains five public ranges
     (`51.148.168.145/32`, `217.155.231.1/32`, `2a02:8010:6680::/48`,
     `2a0a:ef40:154a::/48`, `2001:4860:7::/48`) and both the SSH input rule and the
     forward chain trust the whole ACL, so those addresses get **port 22 and any
     forwarded destination**.  Moving them to the tailnet (or splitting the ACL into
     service clients vs routable admins) is the next decision; it was not made in the
     audit because it cuts off access someone may still intend.
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


## Postgres

- **The application databases still record collation version 2.42** while the box
  runs 2.44, so each still logs `database "…" has a collation version mismatch` on
  connect. `template1`/`postgres` were refreshed on 2026-10-13, which is what
  unblocked `CREATE DATABASE` (the full story, including why `template0` refuses, is
  in `docs/work-log-2026-10.md`). Refreshing an application database is a one-liner
  each — `ALTER DATABASE <name> REFRESH COLLATION VERSION;` — but it is a *statement
  about their indexes*, so it wants a decision per database rather than a loop. The
  strict alternative is `REINDEX` of the collated indexes.

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

## Authentik

- **`RadiusProvider` / `groups` ScopeMapping tidy.** The `switch-gui` provider's
  `groups` scope reuses gocardless-dashboard's ScopeMapping row rather than owning
  one, because creating a fresh child row through the ORM trips a unique-violation
  on `propertymapping.name`. Fine for now; the proper fix is the web UI or the real
  API. `docs/authentik-oidc-notes.md`.

- **The authentik cutover is not done — and the shadow is now parked.** The
  container stack was deployed, migrated from the live database and verified
  against the appliance (`docs/authentik-container-migration.md`), then switched
  off on 2026-10-15 by un-importing `machines/services1/services/authentik.nix`;
  10.3.1.36 is the IdP. **Step nought is uncommenting that import and
  `sudo authentik-migrate all --force`** (the state under `/srv/authentik` is
  still there but stale). Then the four things the checklist names: the outpost
  containers (needs the two `AUTHENTIK_TOKEN` sops secrets, then
  `enableOutposts = true`), the RADIUS clients that name 10.3.1.36 (switch NVRAM
  — ask first), `authentik.int`'s router override to 10.3.1.36 (aibox's sssd),
  and flipping `shadow = false` while deleting the `proxyPass` vhost from
  `machines/services1/http.nix` in the same change.

- **Rotate the appliance's postgres password (2026-10-15).** The first 40
  characters of `PG_PASS` and of `AUTHENTIK_SECRET_KEY` reached a transcript
  through a verification command whose redaction did not work. The database
  password is cheap: it is only read by the appliance's own compose file, so
  change `PG_PASS` in `/srv/authentik/.env`, `docker compose down && up -d`.
  `AUTHENTIK_SECRET_KEY` is not cheap — authentik cannot re-encrypt what it has
  already encrypted, so rotating it means regenerating every stored provider key
  and bind password. Both are now also in `secrets/secrets.yaml`
  (`authentik_env_file`), which is where they should have been.

- **`id-shadow.int.leighhack.org` has no DNS record.** The vhost works and was
  tested with `curl --resolve`; `sudo dns-sync sync` adds the record, and
  `just dns-sync prune` should remove it when the shadow goes away.

- **The nightly authentik dump timer is off** while `shadow = true` (a dump of a
  copy of the live database would only confuse a restore). The unit itself is
  tested; it turns on with the flag. `machines/services1/services/authentik.nix`.

