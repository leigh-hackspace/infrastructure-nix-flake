# Work log — November 2026

The record of what changed and what the changes proved, for the month the fleet
became five machines. `AGENTS.md` is rules; `docs/network-upgrade-runbook.md` is the
procedure; this file is what happened, kept because three of its findings changed that
procedure. Still-open items live in `docs/follow-ups.md`.

## 2026-11-15 — a whole-network repin and deploy: what a five-box upgrade actually costs

First **full-fleet** run since `services3` joined (2026-11-11), and the first where the
repin moved an **engine**. Commit range `63e7015` → `00a85f5`; run from `services1`
(`hostname` read first, as rule 27 demands), all five boxes ended on one commit and one
lock with clean trees.

**Inputs.** `nix flake update` (every third-party input at once, which is what was asked
for — Phase 2's "decide what moves" was overridden by the request, so the engine move
below was accepted rather than avoided). Moved: `nixpkgs` `319e4cebe` (2026-10-14) →
**`2074cc672` (2026-11-12)**, `crane` → `79fd63528`, `nixos-utils` → `246843f70`,
`sops-nix` → `11e5885d2`, `llama-cpp-upstream` `7bcbc04fb` → **`e574a17b0`**.
Did **not** move: `strata` (both engines untouched, so the per-card `STRATA.md`
measurements still describe what is deployed), the `llama-cpp` fork pin,
`door-entry-management-system`, `nix-software-center`, `nixos-appstream-data`.

**Result.** All five boxes `26.11.20261014.319e4ce` → **`26.11.20261112.2074cc6`**, 0
failed units everywhere, `cur == good` everywhere, endpoint probe **byte-identical** to
the Phase 0 baseline, prometheus 15 up / 0 down, `front-door-cert-check` green with
`issuer=C=US, O=Let's Encrypt, CN=YE2`. Postgres 17.11 → 17.12 (same major, restart
only). Order used: `services1` → `aibox` → `services3` → `services2` → `gw` (the
runbook's default, with `services3` inserted as the leaf it is).

**Kernels and drivers — three boxes needed a reboot, and only two got one yet.**
`6.18.56 → 6.18.63` on `services1`/`services2`/`services3`/`gw`, `7.2.10 → 7.3.4` on
`aibox`. The NVIDIA **driver** also moved: `services2` `595.104.02 → 595.112.07`,
`services3` `580.178.04 → 580.196.07`. That second fact is the one that bites, and it is
the three new findings below.

### 1. A GPU box cannot be *switched*; it has to be switched **and rebooted**

The runbook already says a kernel bump with out-of-tree modules wants a reboot, and
frames it as "the module will not load on the running kernel". Measured 2026-11-15, the
shape is worse and more visible than that: activation **restarts the GPU unit into a
userspace that no longer matches the loaded kernel module**, so the endpoint dies on the
spot. On `services3`, `llama-server` went `active → activating` and never came up;
`nvidia-smi` said `Failed to initialize NVML: Driver/library version mismatch`; the
engine's `/health` was empty and the door 502'd. Same on `services2` for `strata` a few
minutes later. Nothing had gone wrong with the configuration — the generation was fine,
`cur == good`, zero failed units; the *running kernel* was the problem.

So on a box with `hardware.nvidia` (or any out-of-tree module its services actually
open), the deploy is one invocation: `switch && nixos-confirm && reboot`. And once the
engine is mismatched, **the reboot is not a risk to be weighed, it is the repair** — the
endpoint is already down, so there is no outage to avoid, only one to shorten. `sudo
reboot` after the confirm, poll for ssh, then poll the engine's own health: `services3`
was back at `{"status":"ok"}` 17 s into the boot (model load took ~1 min more),
`services2` ~1 min in. Both then passed the check the module itself documents, which is
the point: `services3`'s embedded UI still returns the built HTML at 1.0.0, so the
silent-empty-asset-table fault did not return with the version bump.

The three boxes that are **not** yet rebooted (`services1`, `aibox`, `gw`) are healthy
because nothing restarted into a mismatch there — `services1`'s Coral keeps working on
the already-loaded `gasket`, and `aibox`'s `amdgpu` is in-tree. They each carry a new
kernel in their generation and will take it at their next reboot; `gw`'s reboot is a
network-wide DNS-and-front-door outage, so it needs a window, not an agent.

### 2. `just check-build` is a disk hazard on `services1`, and a GC throws the gate away

Two separate traps, both hit in one run, both expensive.

`check-build` builds **all five** toplevels from the box you stand on, and nix copies
every one of those closures *back to that box*. `services1` went from 72 % to **99 %
full (3.6 GB free)** mid-run — on the box holding postgres, nginx, SSO and frigate. It
is not "a full build costs ~25 GB" any more; five machines' system closures on a nixpkgs
that has moved a month is more than that, and the failure mode is a live box with no
space rather than a slow build. Recovery was `nix-collect-garbage` (99 % → 54 %, ~50 GB
back; `aibox` needed the same twice, 55 GB then 30 GB more).

Then the trap inside the recovery: **`nix build --no-link` — which is exactly what
`check-build` runs — creates no GC root.** A `nix-collect-garbage` between the gate and
the deploy deletes every output the gate produced, so the deploy recompiles from
scratch. That is how a "Phase 3 finished, nothing to build" `services1` switch ended up
recompiling ~700 derivations, and why the same happened on `services2`. The builder has
the same property in a nastier form: outputs `aibox` built *for other machines* are
unreachable **there**, so its own nightly GC collects them.

What to do instead, and what worked here: after a reclaim, **finish the gate on the box
that will run the closure** (`nix build .#nixosConfigurations.<m>...toplevel` *on* `<m>`,
or on the builder for `gw`, which cannot compile), and treat "GC" and "deploy" as
mutually exclusive steps — if you must GC, re-run the gate afterwards and expect the cost
in compile minutes rather than being surprised by it mid-activation.

### 3. The thing holding the big lock was not mine: `nixd` did it, twice

The docs' recipe for a hung nix command is "find the survivor of your aborted call and
kill it", with `pgrep -f "nix eval|nix build"`. On both `services1` and `services2` that
pgrep found nothing, because the holder was **`nixd`** — Zed's remote server starts it,
it forks `nixd-attrset-eval`, and each of those keeps a `nix-daemon` child that holds
`/nix/var/nix/db/big-lock`. On `services1` it had held the lock **1 h 56 min** while
`nixos-rebuild switch` sat motionless on `nix-instantiate --find-file nixos-system`;
`services2` showed the identical shape 90 s after its deploy started. Both unblocked
*inside one poll* of killing them.

Two things worth keeping: the detector is a `/proc/*/fd` scan for `big-lock` **printed
with the holder's ppid** (the ppid is what names it as an editor server rather than a
build), and `SIGTERM` does nothing — the workers were blocked in `flock`, so the
`nix-daemon` children had to be `kill -9`'d. Killing them costs an editor its Nix
IntelliSense until it respawns `nixd`, which is not a service and not anyone's session;
waiting it out costs the deploy. Recipe is now in `docs/nix-dev-workflow.md`.

### The repin moved `services3`'s engine, and two hand-copied facts had not followed

`llama-cpp-upstream` is tracked on its default branch on purpose, so a repin moving it is
correct — but `machines/services3/llama.nix` holds **copies** of two facts about that
tree rather than derivations of it, and both were stale: `npmDepsHash` (upstream changed
one UI dependency's `resolved` URL from a `codeload` tarball to a `git+ssh` ref, so the
lock hash moved and the build refused with `ERROR: npmDepsHash is out of date`, *inside*
a toplevel build — nowhere near the thing that caused it) and `version = "0.9.0"`, while
the tree's own `CMakeLists.txt` had moved to `1.0.0`. Same class as rule 31: a fact about
a moving thing, hand-copied, drifting silently.

The repin also **falsified the reason the input exists**, in four places at once. Every
copy of the sentence said nixpkgs' `llama-cpp` is 0.6.0 and has no `lfm3`; at the new
lock `pkgs.llama-cpp` is **0.9.0** (tag `v0.9.0`) and that tag ships `lfm3.cpp` *and*
`qwen35.cpp` — both arches this box serves. So the input now buys a *pin*, not a
capability, and `docs/follow-ups.md` has the collapse-onto-nixpkgs question open as the
engine decision it is (0.9.0 is a downgrade of a live endpoint whose flags are measured at
this pin). Corrected in `flake.nix`, `machines/services3/llama.nix`, its `README.md` and
`docs/services.md`, with the three curl-and-eval probes that re-verify it in seconds
without building.

### Two things the runbook said that this run proved better

- **The netboot dance is a pre-step, not a retry.** Phase 7 tells you to expect `aibox`'s
  switch to exit 4 on `exports-netboot-squashfs.mount` and to fix it with `umount -f -l` +
  `mount -a` + switch again. This run did the `umount -f -l` **first** (after checking
  `ss -tn sport = :2049` was empty), and activation then *started* the mount unit on the
  new store path and exited 0 — no exit 4, no armed rollback countdown, no retry. Same
  conclusion the doc reaches, but the cheap order is the one that never arms the timer.
- **`just deploy <machine>` is the right verb everywhere, including for the router.**
  All five went through it (`services1` via `just switch`, being the box in question), it
  chained every confirm correctly, and it degenerated to the local path exactly when
  appropriate. `gw`'s deploy still evaluated on itself and compiled nothing
  (`max-jobs = 0`, zero `rustc` on the box during its own deploy — re-measured).

### Checked, unchanged, and deliberately not touched

- `nix flake check --no-build` fails at the new lock on
  `…-sops-secret-pg_pass.drv is not valid` and at the **old** lock on
  `…-filestore-web-src.drv is not valid`. Same documented class (an eval reading a
  derivation, rule 22), verified against clean HEAD before being attributed to anything
  (rule 9). `just check-eval` is green at both locks.
- **`ai.int.leighhack.org` / `ai.leighhack.org` 502, and that is the designed steady
  state.** A 502 with no baseline looked like the run's fault; it is not.
  `machines/aibox/strata.nix` clears `llama-server`'s `wantedBy` because llama-server's
  ~33 GB of GTT and strata's 35.5 GiB arena cannot coexist, and its own comment says
  those two vhosts "502 while strata has the box". `llama-server.service` was stopped
  **2026-10-28** and has been `linked`/`inactive` ever since — 18 days before this run.
  The real finding is the gap around it: **no gatus check covers `ai.*`**, so a
  permanently-502'd origin row is invisible to every gate the repo has. Worth a row in
  `common/monitoring-endpoints.nix` that says *which* of the two engines is expected to
  own the box, so the check can be green for the right reason.
- `services1`'s pre-run failed unit `borgbackup-job-backup-srv` was cleared by the
  switch's unit reset; the borg timer itself is unchanged. Pre-existing known-bad
  endpoints (`3d-{1,2,3}.int`, `woodwork.int`, `status.leighhack.org`) were still
  known-bad and identical — the diff is against the baseline, not against green.
- Nothing imperative on the off-flake side moved: `monster1` untouched, the authentik
  appliance VM still stopped, switches untouched (no NVRAM write, rule 16), the Pi Room
  **clients not rebooted** — `aibox` re-exported `/exports/netboot-squashfs` from the old
  store path to the new one with zero NFS clients on `:2049`, so the ten clients pick the
  new image up at their own next boot.

## Part two — the inference outages, the safety net that worked, and what the checks missed

Written after the fleet had landed, because three things the morning's write-up could not
see turned out to be the part worth keeping.

### Both inference endpoints went down, and the reason was not the repin

`strata.int` (services2) and `llama.int` (services3) were both 502 from the front door while
every unit on both boxes read `active`, `just check-eval` was green, and each box's
generation read `cur == good`. The cause was neither the repin nor the front door:
**the switch had installed new NVIDIA user-space over the kernel module the old generation
loaded.** The repin moved both driver packages, and each box's running kernel was still
loaded with the previous module, so the CUDA/HIP stacks could not talk to the cards:

```
nvidia-smi: Failed to initialize NVML: Driver/library version mismatch
NVML library version: 595.112      ← user-space, from the new generation
/sys/module/nvidia/version: 595.112.01  ← kernel module, from the running kernel
config.hardware.nvidia.package: 595.112.07  ← what the new toplevel belongs to
```

The mistake in the reasoning was checking `strata.service` and finding it `active`. With
`INFRA.mkNeverGiveUp` a unit in this state does not crash — it starts, cannot initialise the
card, and the engine answers 502 or never finishes loading. **Unit state is not proof an
engine can see its GPU.** The fix was the reboot the runbook already told me to defer, and
it is exactly why Phase 9 exists — the procedure was sound, and I talked myself out of it at
02:55 to save a few minutes of inference downtime.

The same trap exists for a kernel bump that is not a driver bump. On the box with no failed
units, no disk problem and a green check, `uname -r` is the only thing that says whether the
generation you switched to is the generation you are running: services1 and gw are both on
`6.18.63` generations and were still running `6.18.56`.

### The fleet's own safety net fired, and I nearly walked into it

The first `just deploy services2` did not get as far as the switch. Its remote half died at
status 4, and the answer was not in the service logs but in `journalctl -u auto-rollback`:

```
Nov 15 03:41:01 aibox auto-rollback[472794]: aibox has been switched to a new generation
Nov 15 03:41:01 aibox auto-rollback[472794]: but the operator has not confirmed ...
Nov 15 03:42:31 aibox auto-rollback[472794]: REBOOTING into /nix/var/nix/profiles/system-good
```

aibox had been switched minutes earlier and left unconfirmed, so the five-minute countdown
expired and the box rebooted itself into `system-good` — correctly, exactly as designed
(`AGENTS.md` rule 4). Two consequences worth knowing: `just deploy aibox` then **fast-
forwarded its checkout while the box was mid-boot**, which is what made the following
services2 switch fail while its checkout was at `00a85f5` with no generation built from it;
and the deploy of whatever you are standing on can reboot under you without you having typed
anything. **The rule this teaches is not "be careful with `deploy`" — it is that a fleet-wide
run must land and confirm every box it switches, including its own, and confirm it in the
same invocation.**

### Three findings about how this repo is actually built

1. **The big lock was held for 1 h 56 min by an editor, not a build.** `just check-build`
   and `just deploy` both hung with no CPU and no output. The command this repo recommends
   for that symptom (`pgrep -f "nix eval|nix build"`) found nothing, because the holder was
   `nixd` (a Zed remote-server language server from a session opened earlier) driving
   `nix-daemon --stdio` children — the same shape on services1 and services2. `/proc/locks`
   names it directly: resolve the `big-lock` inode, match the `->  FLOCK` line, read the pid
   **and its parent**. `SIGTERM` did not release the lock (the daemon retried); `kill -9` on
   the daemon children did. `just store-lock` is that lookup.
2. **`check-build` is not disk-safe on a live services box.** Five fleet closures on
   services1 took `/:` 72 % → 99 % (3.6 GB free) while it served postgres, nginx, SSO and
   frigate. Recovery needs a GC, and a GC at that moment deletes the gate's own outputs —
   `nix build --no-link` writes no GC root, so "run the gate, reclaim space, deploy" is a
   trap: every later deploy recompiles. (Measured the hard way: after one GC, services1
   rebuilt ~700 derivations it had already built hours earlier, and services2 rebuilt its
   CUDA engine and a few hundred npm tarballs.)
3. **The remote-builder failure mode has changed, and the old workaround is obsolete.** The
   runbook's Phase 7 note described the hop dying with `kex_exchange_identification:
   Connection reset by peer` seconds in and needing a hand-written `/root/.ssh/config`. That
   fault is fixed — `common/ssh-known-hosts.nix` is now the pin and
   `common/build-farm.nix` writes `/etc/ssh/ssh_known_hosts` (rule 25), and this run's
   remote builds were clean. The failure mode now is *mid-build* (`builder dies during a
   long crate compile` → `error: cannot build ... on the remote machine`) which is harmless:
   `nix.distributedBuilds` falls back to local and the rebuild continues. So the note about
   root's ssh config is history and says so.

### What the checks did not cover

The gate proved it catches the class it was built for, and its limits are now written down:

- `just check-eval` is green while a **runtime** mismatch between kernel module and
  user-space is certain. No eval can see it; only a run-time check can (see the guard below).
- `nix flake check --no-build` cannot run on a *new* source path — the first build-shaped
  command after any tracked edit fails on `…-sops-secret-<name>.drv is not valid`, because
  every build-time secret drv is keyed to the flake source. So the flake step answers
  "has this source been built here yet?", not "does this lock evaluate?" — worth knowing
  before chasing it.
- `check-doc-pointers` §2 (every tracked file in a machine dir must be reachable from an
  import list) fired on the parked module and passed when it was imported, proving the
  `# PARKED` marker is the only thing that keeps such a module inert — a new parked module
  must go through the same two steps (import it so §2 is satisfied; mark its own header).
- gatus watched nothing in the inference group, so neither outage was visible on the board.

## What was made structural out of it

Written the same day, from the measurements above rather than from the narrative — each one
exists because this upgrade paid for it.

- **`common/gpu-driver-match.nix`** — `infra.nvidiaDriverMatchGuard.units = [ "strata" ]`
  (services2) and `[ "llama-server" ]` (services3) prepend an `ExecStartPre` comparing the
  version `config.hardware.nvidia.package.version` this system was **built** for against
  `/sys/module/nvidia/version` the running kernel **has**, and refuses to start with the
  reason and the fix. It fails loudly rather than silently because `INFRA.mkNeverGiveUp` is
  already on both units, so a refusal becomes a legible `activating` retry state that
  self-heals on reboot — and it replaces the "`strata.service` is active" reasoning that
  fooled the morning. It exits 0 when no module is loaded at all, because on a box where GPU
  passthrough is in flight that is not this fault. Both branches were exercised on the box
  before shipping: the match, and a copy of the rendered script with the expected version
  perturbed.
- **`just pending-reboot [machine]`** — running kernel vs the current generation's kernel,
  plus the NVIDIA module and whether `nvidia-smi` agrees. It answers "the deploy said
  Success and the inference is down", and it found three boxes (services1, aibox, gw) still
  running the previous kernel because the upgrade did not reboot them — a fact no unit
  state, profile listing, or `just check` expresses.
- **`just store-lock`** — resolves the `big-lock` inode through `/proc/locks` and prints the
  holding pid **and its parent**, which is the only way to see that an editor's `nixd` owns
  the store.
- **`just check-build` takes a machine and roots its output.** It was a hard loop over all
  five toplevels; it is now `check-build [machine|all]`, because the 2026-11-15 shape of the
  job was one box's service config, and there is no reason to assemble five fleet closures
  on the live services box to answer it. It refuses below a disk floor (`_disk-headroom`,
  overridable), prints free space as it goes, and symlinks each toplevel into
  `/nix/var/nix/gcroots/check-build/<m>`. Measured while writing it: an unrooted
  `nix build --no-link` output is deleted by the next GC, a symlink-rooted one is not
  (`nix store delete` refuses with *still alive*), and **a text gcroot file listing store
  paths is silently ignored** — the path was deleted anyway — which is worth knowing because
  the nix docs make you expect it to work. `just check-build-clean` drops the roots.
- **`just deploy` does the `aibox` netboot dance itself** (lazy-unmount the squashfs export
  before the switch, since a nixpkgs bump moves its store path and activation cannot restart
  a boot-generated mount onto the new one), prints the target's disk first, never confirms a
  failed switch on the local path, and finishes by printing `just pending-reboot <machine>`
  so "switched but not rebooted" is said when it becomes true rather than discovered at the
  endpoint.
- **Three inference endpoints are now watched.** `strata.int`, `strata2.int` and `llama.int`
  `/health` joined `common/monitoring-endpoints.nix` as an **Inference** group: both failures
  of this upgrade were engines 502-ing with zero failed units, and gatus had no row for any
  of them, so the board could not have shown either. The public `ai.*` names are deliberately
  absent — they answer 502 as the *designed* state while `strata` owns aibox's GTT, and a
  check that is red by design trains people to ignore the board; giving it a stated
  expectation is open in `docs/follow-ups.md`.
- **The builder's GC cost is specified rather than suffered** — `docs/follow-ups.md` carries
  the design (one symlink per machine under `/nix/var/nix/gcroots/deployed` on aibox,
  `ln -sfn` so the previous generation ages out automatically), the measurements that say why
  it is not in tree yet (aibox holds **zero** paths of the services3 generation running on
  services3, so there is nothing to root; a client→builder `nix copy` dies on `Too many
  authentication failures`, the `IdentitiesOnly` trap that same item lists), and the order
  the fix has to go in. It was written, tested, and **removed** rather than shipped
  half-working: a GC root pointing at a path the machine does not have protects nothing and
  reads like protection.

### End state, read off the boxes rather than asserted

All five deployable machines on `331f45a` with clean checkouts (services2, services3 and gw
needed the peer-fetch fallback — they have no GitHub key of their own, and `git fetch origin`
quietly failing is indistinguishable from "already up to date" unless the recipe says so,
which `just deploy` does). `cur == good` on all five; `systemctl --failed` empty on all five;
gatus reporting **37 of 37 endpoints UP**, which includes the three Inference rows answering
200 through the front door at 2–30 ms — a better end state than the pre-upgrade baseline, on
which `3d-{1,2,3}.int`, `woodwork.int` and `status.leighhack.org` were red. Three boxes are
**deliberately left pending a reboot** because the kernel moved and none of them can afford
the window yet: `services1` (`6.18.56` running, `6.18.63` wanted — it hosts postgres and the
IdP), `aibox` (`7.2.10` → `7.3.4` — the builder, whose netboot export the Pi Room clients are
booting from), and `gw` (`6.18.56` → `6.18.63` — the router, DNS and front door: rebooting it
drops every tunnel and every vhost for a minute). `just pending-reboot` names exactly those
three, which is the point of having it: the fleet now carries a list of what is not finished,
instead of that fact living only in whoever read `uname -r`.

## 2026-11-16 — a voice assistant, and the two things the camera cannot do

Asked for a smart speaker out of `cam8`, with the parsed request going to the bot that
already answers in Slack rather than a second brain, plus a web page so any phone or laptop
can be the speaker instead. Shipped: `crates/voice-assistant` (a loopback gateway — whisper,
the bot, piper — and a camera satellite), `slack-agent listen` (the bot's answer on a
loopback socket, no Slack, no token in its environment), and an Assistant card on
`whisper.int.leighhack.org` that does push-to-talk and hands-free on the wake word the page
already runs in WASM. **The camera's speaker is off**, and the page answers out loud.
Design in `crates/voice-assistant/README.md`, receipts in `docs/voice-assistant-2026-11-16.md`.

Four things were only knowable by measuring, and two of them killed a plan:

* **`cam8` has no ALSA.** No `/dev/snd`, no `/proc/asound` — SigmaStar MI driven by `rad`, and
  `rac record`/`rac play` are the only way in and out. One core, 36 MB. So nixpkgs'
  `services.wyoming-satellite` — the obvious "don't write it yourself" answer, sitting right
  there in the same pinned tree as `wyoming-piper` and `wyoming-openwakeword` — **cannot run
  on it at all**: its `arecord`/`aplay` have no device. The whole Wyoming/Home-Assistant route
  died on that one row, and with it the option of not writing this code.
* **No echo canceller, on either side** (`rac status`: `aec: false`, `ns: false`, `agc:
  false`). Not a tuning problem: a device that listens while it speaks transcribes its own
  answer into a new question. So the satellite is half-duplex by rule — audio captured during
  playback is dropped and counted — and the numbers a future server-side AEC needs
  (`audio_ms`, `played_at`) are recorded now so that work is not a guess.
* **The mic works and hears the wrong thing.** `rac record -d 3` → whisper in **2.7 s** →
  `' (soft music)
'`. The pipeline is right; the space has a stereo. That single transcript
  is why the gate is three features rather than an energy threshold, and why the release
  includes a verb whose whole job is to sit in the room and count
  (`just voice-probe` → `probe_false_positives`).
* **The turn's budget is the model.** whisper 2.7 s (on **CPU** — see below), piper 2.5 s,
  one-tool answer 7.7 s: ~13 s spoken end to end, and minutes if the model decides to be
  chatty. Hence `MAX_SPEECH_TOKENS = 200` against Slack's 700, a spoken system prompt that
  forbids bullets and URLs, `spoken.rs` as the backstop when it ignores them anyway, and the
  observation that the biggest available win is the deterministic `events_window` path that
  calls no engine at all.

Two documentation corrections fell out, both of the AGENTS kind (a comment believed longer
than the box): `machines/aibox/whisper.nix` asserted since 2026-09-02 that the unit "cannot
(re)start" because `--device 1` no longer exists. The live unit is `active`, `NRestarts=0`,
started 2026-11-15, and answering — it does not crash, it **silently falls back to CPU** (no
`/dev/dri` fd, its own `system_info:` line names CPU and no Vulkan, `--help` reports exactly
one Vulkan device and it is llama-server's). A stale "this is broken" comment had been
pointing at a service that had been quietly running on the other backend for two months,
which is the same failure shape as the `mqtt.int` row: the doc, not the config, was wrong.
The follow-up is now honestly stated as *decide the backend*, not *make it start*.

## 2026-11-21 — services3 becomes the network's third Strata engine (Pascal, CUDA 12)

The box on the bench had a GTX 1080 Ti doing 63 tok/s of a 9B model under llama.cpp and
40 GB of RAM doing nothing. It now runs **Qwen3.8-Flash-Next Coder IQ1_M** — the same pack
services2 serves — across the card, the RAM and the 476 GB NVMe that arrived holding a
previous owner's ZFS pool. `strata3.int.leighhack.org`, in place of `llama.int`.

What made it more than "copy services2's module", in the order it bit:

- **It is a different CUDA, and that is not a preference.** CUDA 13 dropped Pascal: `nvcc`
  fails CMake's compiler test with `compute_70`. GP102 is compute capability 6.1, so the
  build is **CUDA 12.8** (12.9's math headers clash with glibc >= 2.43 and this box runs
  2.44) with `-DSTRATA_EXPERIMENTAL_SM60=ON`, upstream's opt-in path for cards below
  sm_75, one arch (`61`). `machines/services3/strata-package.nix` is the third
  `strata-package.nix` in the tree for exactly this reason, and `.#strata-cuda12` is its
  flake output — building it is also the only way to know the toolkit choice is right,
  because the failure is in CMake's first test compile, not in the engine.
- **The card is not a small RTX 3060, it is a fast fp32 card with no half precision.**
  Consumer Pascal runs FP16 at 1/64 of FP32, so cuBLAS's FP16-operand GEMMs — the dense
  projection path — are the slowest thing on it. `STRATA_DENSE_F32=1` sends those through
  `cublasSgemm` on FP32 copies instead (upstream measured that on a Tesla P40, the other
  sm_61 card: 430-474 tok/s of prompt against 360-432). The fused int8 prompt kernels
  services2 deployed are **not** an option here: they need sm_80.
- **`--resident-experts`, not `--mmap-experts`.** The Coder arena is ~23.4 GiB and the
  card holds ~9 of them; the rest are copied into RAM once. 40 GB of RAM is what makes
  that the right shape, and it is why this box needed no swap (a swap file on the same
  NVMe that serves the 27 GiB PLE table would fight the one tier it is short of).
- **The name moved with the engine.** `llama.int.leighhack.org` was a row in
  `common/front-door.nix` with no injected key (that row's own note argued, correctly,
  that pointing an unrelated engine at `strata_api_key` widens a secret's audience).
  Strata *is* that engine, so the row became `strata3.int.leighhack.org` with
  `strataBearer = true`, the `llama` entry in `common/int-dns.nix` became `strata3`, and
  the gatus `/health` row and pi's `llama-services3` provider moved with them — five
  files, one fact, and the two evaluation-time guards are what say whether they agree.

What was **not** carried over: `llama-server` (parked, with its measurements, in
`machines/services3/llama.nix` — one GPU serves one engine), the four things it had
loaded into `/srv/models` (still there; nothing reads them), and any claim that this box
is quick. Its RAM is the engine's and its CPU pool is four Skylake cores, so it is the
network's slowest endpoint by construction and `docs/services.md` says so rather than
quoting the P40's numbers.

**It works, and the numbers are the point:** 18.2 tok/s decode and 464–539 tok/s
prefill on the live unit at **128K context** (Coder IQ1_M), with a needle at 60 % depth
of a 77,612-token prompt answered correctly in 152.8 s of prefill. At 32K the same
engine did 17.6–21.0 tok/s — the 128K KV moves ~1.35 GiB of expert cache to KV, which
costs about 13 % of decode and nothing measurable of prefill. 418 MiB of VRAM free after
the 77K request is the card's edge, and the engine says so; the two dials are in the
module. The 3060 does ~32 tok/s on the same pack, so this is the card's ceiling
being reached, not the box being misconfigured. One flag was learned by refusal: a
native (IQ) pack will not start without `--spec T (T >= 2)` — the suffix drafter's
verify window, which needs no MTP weights — and `--prefill auto` resolves after that
check, so the error named three things and wanted one.

**The box also crashed once, and it is not resolved.** 17:50, ~30 min into the
download: three Oopses at the same address, `kmem_cache_alloc_noprof` inside
`alloc_buffer_head`, a faulting pointer one bit from canonical, filesystem wedged,
journald watchdog, down. It did not reproduce in the 90 minutes of downloading, pack
prep and model load that followed. No ECC on this board, no NVMe error, no MCE — "bad
RAM" and "a kernel bug that overwrote its own freelist" are both still open, and the
discriminators (slub_debug=F, then the pinned nixpkgs' 6.12 LTS) are written down in
`machines/services3/README.md` rather than guessed at here.

The bring-up, with what the first boot actually measured, is
`machines/services3/README.md`; the flags and the VRAM budget are
`machines/services3/strata.nix`'s header.

### 2026-11-22 — merging that branch back, and the two things the branch could not have known

`services3`'s commit arrived on a box that had kept working, so `main` had to take it without
having been able to test it against anything. Two findings, both of the "the branch was true
when it was written" kind:

* **`main` had moved the one input all three Strata engines share.** The box was built on
  `13eedf552`; `main` is on `3142b66b9`, past upstream's 0.2.16 layout move and with the input
  declared `flake = false`. So the third `strata-package.nix` arrived configuring a root CMake
  project that no longer exists, copying `serve/` from a path that is now `terminal/serve/`,
  and carrying its own `fetchFromGitHub` of llama.cpp next to a tree that vendors it — and
  `strata.outPath` on an input that is now a bare source path. Reconciled with services2's
  working package (`cmakeDir = "../engine"`, the `terminal/` copy list, the vendored gguf-py,
  the second pin deleted). The Pascal half was checked against the pinned tree rather than
  assumed: `STRATA_EXPERIMENTAL_SM60` and the sub-sm_75 refusal it lifts are still there in
  `engine/cmake/backends/cuda.cmake`, and `docs/OLDER_GPUS.md` still gives the CUDA-12 recipe.
  **What is not proven is the build itself**: nothing has compiled the sm_61 arm at the new
  pin, and the box is live on engine 0.1.59.1 (read out of `strata.log`, and its
  `ExecStart` names `…-strata-unstable-sm61`, which lives in **aibox**'s store — services2,
  warm for CUDA 13, does not have it). `docs/follow-ups.md` owns the order.
* **the commit's own last claim is false, and measuring it took one `curl`.** It said
  "`llama.int` is dead either way; that name is retired, not broken" — but Strata took
  llama.cpp's port 8080, and neither the door table nor dnsmasq had moved yet, so from `gw`:
  `https://llama.int.leighhack.org/health` answers **200** with
  `{"service":"strata","model":"qwen3.8-flash-next-coder-iq1m","api_key":true}` and `/v1/*`
  answers **401**, because that row deliberately injects no key. A retired name that resolves
  is worse than one that does not: the gatus `Inference` row for "Llama (services3, Vulkan)"
  is green, and it is green about a different engine. `strata3.int` does not resolve yet
  (the merge is not deployed), so the network is in the interval where both names point at one
  engine and only one of them says so. That interval is the item to close, and it is written
  up as one — `docs/follow-ups.md`, "`strata3.int` is declared and not deployed".

## 2026-11-22 — one commit on every box, and the fleet on the current Strata pin

The follow-up from the morning: land `services3`'s engine work, then everything the merge
touched, on every box. What that turned out to involve, in the order it happened.

**Two boxes held work nobody had pushed.** Phase 0's `git log -1` per box showed `aibox` on a
commit `main` had never seen; fetching each box's `main` found **four** `paper printers`
commits there and services3's one engine commit (already merged). A reset-to-one-rev, which
is what the runbook's Phase 1 step 2 tells you to do, would have **deleted** aibox's four.
That probe is now in Phase 0 as its own bullet, with the loop, because nothing else in the
runbook asks the question. Both stacks rebased onto `main` — aibox's with zero conflicts,
services3's with five, all of them append-mostly rows in `AGENTS.md`, the month's log,
`docs/services.md`, the new `common/llm-endpoints.nix` and the box's own README.

**The push was rejected, and correctly.** `origin/main` had moved to the same four printers
commits aibox was holding, so my rebased copies were duplicates by content and divergent by
SHA. Collapsing them (`git rebase --onto origin/main`, then proving the tip trees identical
with `git diff tmp/collapse main` empty) is what `git cherry` is for: it said every aibox
commit was already upstream (`-`) and services3's **was not** (`+`) — because I had edited
inside it during the merge. Patch-id equality is not the same question as tree equality, and
the two boxes needed different answers (`just deploy` could fast-forward aibox; services3 got
a safety ref and a reset).

**The gate refused to run where the runbook says to run it.** `just check-build` on aibox died
on `door-entry-management-system`'s `__noChroot` — which is not a new finding, it is
`docs/follow-ups.md`'s own item, quoted in advance ("that failure is this item, not your
change"). services1 is the box that may build it, so the gate ran there, per machine, so each
machine got its own exit code: services1 0, services2 0, services3 0, and it spent real time
compiling CUDA 13 from source on aibox for services2's engine because the 2026-11-21 bump had
since GC'd it — the lesson in `machines/services2/STRATA.md` ("build the CUDA half where the
toolchain is warm") is about *timing*, not just toolchain choice.

**A detached unit's `sudo` is a separate program from an interactive shell's.** Three launches
died with `sudo: /run/current-system/sw/bin/sudo must be owned by uid 0 and have the setuid
bit set`, and `NoNewPrivileges=no` proved it was not the usual cause: NixOS's setuid sudo is
the **wrapper** at `/run/wrappers/bin/sudo`, and my `--setenv=PATH=/run/current-system/sw/bin:…`
resolved the bare, un-setuid binary in `sw/bin`. It bit exactly once per shape — a `just`
recipe that calls `sudo`, run as a unit — and it is worth noting what it did *not* do: the
`just deploy gw` attempt died **before** activation, so nothing was switched, nothing was
confirmed, and `auto-rollback` was never armed. Documented in `docs/nix-dev-workflow.md`, in
the same bullet list that already warns about shebangs and `HOME=/root`.

**Landed, in this order, each one read back:** `gw` first, because it owns the interval — the
rendered `nginx.conf` gained the `strata3.int` block and lost every `llama.int` byte,
dnsmasq answers the name on all three door addresses, `getent hosts llama.int…` returns
nothing, and from the router through the door `/v1/models` answers **200 with no client key**,
which is the only test that distinguishes an injected `strataBearer` from a configured one.
Then `services1` (gatus's API on `:8999` now lists `Strata3 (services3, CUDA sm_61)` and no
Llama row), `services2`, `services3` after the sm_61 engine finally built at the new pin, and
`aibox` **last on purpose**: it is the builder, and switching it mid-build is how you kill
someone else's compile. Five boxes, one commit `0bb5495`, `current == system-good` on all
five, `systemctl --failed` empty on all five.

Engines, through the door, after the run: `strata` Coder IQ1_M loaded, `strata2` IQ2_XS
loaded, `strata3` Coder IQ1_M loaded — the last two having gone 0.1.59.1 → **0.1.63**, so the
one `strata` input finally means one engine version on all three cards. `just pi-agent-status`
discovers `strata-services3 … ctx=131072` from the live endpoint, which is the rename working
end to end — and it also reported `not managed here: llama-services3`, because renaming a
provider id strands the old one in every box's `models.json` by design; that is
`docs/follow-ups.md`'s problem now, not this run's.

Left open, deliberately, by this run: **`gw` is running kernel `6.18.56` against a generation
built for `6.18.63`** (`just pending-reboot` says so), which is a reboot of the thing that
answers DNS for the network and is therefore a decision with a time of day, not a step; the
ten Pi Room clients have the new image exported at them and will take it **on their own next
boot**; and services3's 18.2-tok/s band in its README is 0.1.59.1's measurement, now a prior
rather than a number.

## 2026-11-22, later the same day — the fleet on a second commit, a reverted flag, and the gate that had to change shape

Same day as `fe19e16` (the GitHub hop, written up in `docs/nix-dev-workflow.md`), so this
entry is short and its job is the three things that cost time. The commit landed here is
**`8cd7594`**: it reverts the `--kv-resident 32768` the morning's read of services3's
strata log wrote into `machines/services3/strata.nix`, and rewrites the sentences that
promised it — the box already writes `1690 of the prompt path's 1690 lendable slots
keep their experts in RAM too`, its tier line ends `(no file reads)`, and the evidence
for the flag was services2's numbers on the one box whose own log says the thing is
already paid. `git log -- machines/services3/strata.nix` now reads add-then-delete on one
day with **no deploy of that box between the two**, so the engine's args — the config
file's `args` array, which is what `--config <store json>` actually carries — never moved
and services3's switch of that day did not restart its engine.

### What the gate had to become, and what it cost

`docs/network-upgrade-runbook.md`'s Phase 3 now owns the mechanics (a root unit cannot
gate this flake — `just` is not in a unit's PATH, and uid 0 will not fetch a `git+file:`
input the operator owns; the per-machine shape is the shape near full; the outer
`_disk-headroom "60"` is not per-machine). The numbers, because a runbook that says "is
near full" without them invites the next reader to try:

* **`services1` went `/` 76 G → 18 G inside one `check-build all`**, then its own guard
  refused the next machine. The reclaim that got it back was the documented decision —
  `just check-build-clean` to drop the interrupted gate's roots, `--delete-generations`
  on 30 system generations, `nix-collect-garbage -d` — **85.5 GiB freed**, and `/` back to
  75 G. That GC was only safe because the gate's outputs were unrooted and *no* gate for
  the new commit had run yet; the docs' warning about GC between gate and deploy is what
  made the order matter rather than the size.
* **aibox ran out of RAM, not disk**: 59 GB total, `MemAvailable: 2.1 GB`,
  `SwapFree: 173 MB`, with its `strata-host` at **31.6 GB RSS** and the netboot
  `squashfs.drv`'s `mksquashfs` at 9.4 GB. Stopping `strata` for the squashfs half and
  starting it afterwards is the documented "first thing to lose", and this run paid its
  cost: `strata.int.leighhack.org` was dark for the minutes in between, and read
  `/health` 200 only after the restart — the honest version of "the builder is the
  inference box" is that the pair that has to fit is 41 GB.

### What landed, and what was seen on the boxes rather than asserted

Five boxes on `8cd7594`, `dirty=0`, `main...origin/main` on all five (`services3` pushed
GitHub itself again — the scratch-branch proof of the *write* grant is in
`machines/services3/README.md` and did not need repeating for this commit; `aibox`'s and
`gw`'s stale `origin/main` refs were corrected to GitHub's own answer, read from a box
that has a key, since those two refuse the fetch). `current == system-good` on all five
after a chained `nixos-confirm`. `just pending-reboot` clean on all five, and both GPU
boxes' gates agree with their engine (`services2` module 595.112.07, `services3` 580.196.07)
— no reboot owed there. **`gw` owed one and took it**: kernel `6.18.56` running against a
generation built for `6.18.63`, so `switch && nixos-confirm` then `reboot`, and after the
reboot `uname -r` is 6.18.63, `front-door-cert-check.service` reads
`issuer=C=US, O=Let's Encrypt` (not the minica fallback), and the leases page is a live
table — `total 58, expired 0, stale: null, source_age 154` — not the frozen-zeroes shape
`machines/gw/lease-reader.nix` exists to catch.

Through the door after the run, read with `curl`: `id.int` 200 (302 at `/`, authentik
redirecting), `api.leighhack.org/space.json` 200, `strata` / `strata2` / `strata3`
`/health` 200, `voice` / `filestore` / `gatus` / `frigate` / `ha` / the three
`*-status` dashboards 200, `switch.int` 302 (OIDC, so the gate is in front of it), and
`network-info.int/api/connections` 200. `mqtt.int` answers nothing over HTTP — it is the
`answeredBy` socket name (`via = "services1"` in `common/int-dns.nix`), and its proof is
the broker's unit (`podman-mqtt.service` active on services1), not a `curl`; the 000
there is the expected shape and is why the doc says "ask what dials a name".

`just check-eval` on the router with the new source: printer tables ok, network tables ok,
doc pointers ok, voice platform boundary ok, source sizes ok, alejandra ok.

### The finding that is not about this change

**The UPS on the router went dark on that reboot and has not come back** —
`upsdrv.service` fails `No matching HID UPS found` at boot and on a retry an hour later,
`upsc apc` says `Driver not connected`, and `sudo lsusb` on `gw` lists eight devices,
none of them a UPS, while `/etc/nut/ups.conf` still names the bus port and serial that
used to be there. That is the whole power-cut chapter's foundation, so it is written up
in `docs/ups-and-fleet-shutdown.md` (§7a) and open in `docs/follow-ups.md`, and it is
aggravating rather than caused: `machines/gw/ups.nix` was not touched here. Two receipts
came with it: the notice unit also failed that boot with three recipients unreachable
(the router's own network was flapping), and `sudo systemctl reset-failed` — run to clear
the transient gate units — erased both from `list-units --failed`, which is why an empty
failed-list on `gw` is not proof the UPS is alive.

### Left open by this run

* **The docs commit that follows** (this file, the runbook's Phase 3, the UPS chapter, the
  follow-ups' two items) adds no `.nix` file, so no machine's content moves with it — that
  is the reason the five boxes were *not* re-switched for it, and `services1` was left
  running `8cd7594`'s toplevel rather than the docs commit's, which is the half a later
  reader must not read as drift.
* **The four evaluation-warning classes at this lock** (`slack-agent-digest`'s ordering,
  Grafana's plaintext `secret_key`, `ups-watchdog-0.1.0` missing `meta.mainProgram`, and
  the `stdenv.is*` deprecations) are a re-read of rule 9's baseline in
  `docs/follow-ups.md`; the `ups-watchdog` half is ours to close and is the half the
  router's two UPS units are currently trusting `getExe` for.
* `gw` and `services2` still have no GitHub credential of their own — `just repo-status`
  says `NO-CREDENTIAL` for exactly those two, and this run moved commits to them with a
  one-commit bundle over the LAN rather than by copying a key onto a bench box.
