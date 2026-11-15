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
