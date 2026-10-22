# Full network upgrade — the macro

The procedure for moving the whole flake forward **once** and landing it on every
machine the machine-hop-key reaches, in an order that keeps the network working
while it happens. Read it before running one, and follow it in order: the steps
have gates, and a gate that fails is a stop, not a "continue and hope".

A full network upgrade is the largest blast radius this repo has. It touches the
one box every public request goes through (services1's nginx and the SSO in front
of it), the one box that decides whether anything is *reported* as broken
(services1's gatus, since 2026-10-17), the network's build machine (aibox, since
2026-10-17), the desk box that hosts the photo/video/AI apps, and — without being
touched — the router, the NAS, the Proxmox host and the appliance that is the IdP. Almost every rule in
`AGENTS.md` is a rule here because this is where it bites.

Read first: `AGENTS.md` (rules 4, 5, 11, 13, 20, 21), `docs/nix-dev-workflow.md`,
`docs/follow-ups.md`, and the per-box READMEs (`machines/services1/`,
`machines/services2/`, `machines/aibox/`).

---

## The macro, in fifteen steps

Skim this; the detail below is what each step means.

1. Read the state of every box. Change nothing.
2. Confirm every box is *settled* (`/run/current-system` = `system-good`).
3. Put the whole network on **one commit**: push local work, then reset each box to it.
4. Decide, per input, whether the upgrade moves it. Update `flake.lock` deliberately, commit it.
5. `just check` (evaluate everything, activate nothing). Re-read the baseline at the new lock.
6. `just check-build` (build all four machines' toplevels, activate nothing) — detached, polled, ~75 min for a nixpkgs bump.
7. Read the diff for **interface** changes (ports, addresses, APIs). They reorder step 10.
8. Decide per box whether the upgrade changes the kernel. A kernel change means a reboot.
9. Pick the order: services1 → aibox → services2, unless 7 said otherwise.
10. Land one box: `switch` **and** `nixos-confirm` in one shell invocation — chained with `&&`, so a failed switch is never confirmed.
11. Verify that box before starting the next one. Never two boxes mid-switch at once.
12. Repeat 10–11 for the next box.
13. Do the network's own after-effects: DNS records, netboot image, printers, VMs, router.
14. If a box failed twice, stop the run and put the remaining boxes back on the old commit.
15. Record what moved, what failed, what is now open — `docs/work-log-2026-10.md`, `docs/follow-ups.md`.

---

## What "the network" is, and what an upgrade means on each box

Reachability below was probed from services1 on 2026-10-15 with
`ssh -i ~/.ssh/agent-hop-key -o ConnectTimeout=5 -o BatchMode=yes`. It is a
statement about *that day*; re-probe it, do not inherit it.

| Box | Address | Reached as | What an upgrade means here | What must **not** happen here |
| --- | --- | --- | --- | --- |
| services1 | 10.3.1.20 | `leigh-admin` + hop key | full `switch`. The choke point: nginx terminates TLS for the whole network **and is the front door every `*.int` name answers at**, nginx-sso answers every `X-WEBAUTH-USER` app, postgres serves five DBs | a reboot that takes nginx/postgres/SSO down without knowing who is up |
| services2 | 10.3.1.21 | `leigh-admin` + hop key | full `switch`. The RTX 3060 / Strata **Coder** engine, its own status dashboard, a 16 GiB swap file | being switched while its 30 GiB is the engine's; being mistaken for the builder or monitor it no longer is |
| aibox | 10.3.1.32 | `leigh-admin` + hop key | full `switch`. Hosts immich, frigate, whisper, the netboot server, a status dashboard — and, since 2026-10-17, the **builder** | deploying from its **dirty** tree (see Phase 0) |
| gw (the router) | 10.3.1.1 is the **flake's own box** (cut over from the OPNsense appliance on 2026-10-21; the appliance is retired) | `leigh-admin` + hop key, root not keyed — like every other machine here | `just switch-gw`, built **off** the router (builds delegate to aibox and `nix.settings.max-jobs = 0`, so nothing compiles here) and confirmed on the box. It answers DNS and hands out leases for the whole network | deploying gw while services1 is mid-switch (the deploy that adds a vhost's record is this one); anything that takes the router down; a build on the box itself (`_build-host-guard`) |
| NAS | 10.3.1.6 | `leigh-admin` + hop key (root is **not** keyed) | nothing declarative. It must stay online: services1 and aibox NFS-mount it, borg runs on it | editing `/etc/sudoers` (middleware regenerates it); blaming a mount failure on the flake |
| monster1 | 10.3.1.11 | `root` + hop key | nothing declarative. Its VMs' disks live on the NAS | `qm start` on an HA-managed VM; treating `storage 'nas2-nfs' is not online` as a VM fault |
| authentik appliance | 10.3.1.36 | **not reachable** — the hop key was refused (`Permission denied (publickey,password)`) | nothing. It is the live IdP every app and every switch login goes through | un-parking `machines/services1/services/authentik.nix` (the shadow is **parked** for a reason) |
| 3d-lime | 10.3.14.61 | `hackspace` + hop key (probed ok) | nothing flake-managed; `moonraker-exporter` polls it at a fixed IPv4 | renaming it, or expecting the flake to change it |
| 3d-blue | 10.3.14.62 | ssh **timed out** from services1 (it is off-network — `docs/follow-ups.md`) | same | assuming its absence is caused by the upgrade |
| netboot clients | 10.3.14.101–110 | the justfile's netboot recipes use `root` + `StrictHostKeyChecking=no`; 101 timed out from services1 on 2026-10-15 | they pick the new image up **on their next boot**; `just switch-netboot` re-exports the squashfs and reboots all ten | rebooting ten clients unprompted |
| switch1 / switch2 / switch3 | 10.3.1.2 / .3 / .4 | ssh **refused** — Telnet only, via `crates/switch-gui/` | nothing. Their NVRAM holds the names 10.3.1.36 and 10.3.1.20 | any NVRAM write; a reboot is the only undo |

The upgrade is therefore **three `switch`es plus a set of things you must not do**.
Everything else in the network is either out-of-band or downstream, and the
runbook's job is to keep them alive while the three move.

---

## The three invariants the whole run protects

Everything else is detail. These three are what a network upgrade can quietly
break, and each one has already been broken once:

1. **One commit on every box.** Each box runs `nixos-rebuild --flake .` against
   *its own* checkout. Three checkouts on three commits is three different
   networks, and rule 20 (`git log -1` on a remote box before blaming a config
   change) exists because this happened. Phase 1 makes them identical and proves
   it.
2. **One lockfile.** `flake.lock` is what decides which nixpkgs, which
   `pi-room-sys`, which `strata` every box builds. A box that updates its lock
   alone is a box that will not build the same thing as the others.
3. **Every switch confirmed in the same shell invocation** (the GOLDEN RULE).
   `system.autoRollback.enable = true` on all three boxes: five minutes after a
   switch, one timer fires, compares `/run/current-system` with
   `/nix/var/nix/profiles/system-good`, and **reboots the box into the old
   generation** if they differ. A deferred confirm marks the *old* generation
   good; a rollback reboots the machine out from under whatever the next tool
   call is doing.

---

## Phase 0 — read the state. Change nothing.

Each of these is a cheap poll, and each one rules something out that would
otherwise cost a day later. Run them from services1 (the box you should be on:
`hostname` must say `services1` — it is the choke point, it has the headroom, and
it is where the hop-key probes and the push have to start. Since `8e0430c` it is
**no longer** the only box that can evaluate the flake: nothing is read out of
`~/Projects/` any more, so `just check` runs on services2 and aibox too).

```bash
hostname
readlink /run/current-system /nix/var/nix/profiles/system-good   # must be equal
systemctl --failed --no-pager --no-legend                        # must be empty
df -h / ; df -h /home                                            # build headroom
pgrep -a -f "nix eval|nix build|nixos-rebuild"                   # no survivors
```

Then the **imperative state the deploy itself depends on**, which no eval will
tell you about and which is not the same on all three boxes (each of these has
now cost a failed switch):

```bash
for h in 10.3.1.20 10.3.1.21 10.3.1.32; do
  timeout 20 ssh -i ~/.ssh/agent-hop-key -o ConnectTimeout=5 -o BatchMode=yes leigh-admin@$h \
    'echo "$(hostname): root_safe_directory=$(sudo git config --global --get-all safe.directory | paste -sd, -) tailscaled=$(systemctl is-enabled tailscaled 2>&1) swap=$(free -g | awk "NR==3{print $2}") esp_free=$(sudo df -h /boot/EFI | awk "NR==2{print $4}")"'
done
```

- **`root_safe_directory` empty** → a `sudo nixos-rebuild --flake .` on that box
  fails instantly with libgit2's "not owned by current user". Set it before you
  need it (`sudo git config --global --add safe.directory
  /home/leigh-admin/Projects/infrastructure-nix-flake`).
- **`tailscaled` is `not-found` on services2 and aibox** — only services1 runs
  tailscale (and headscale). "Check tailscale is up" is not a per-box check here;
  asking a box for a unit it has never had wastes a poll and reads as a regression.
- **`esp_free` matters when the kernel bumps**: three kernels' worth of a new
  generation lands in the ESP on the next boot, and a 400 MB-full ESP turns a
  reboot into a rescue job. 2026-10-15: 401-428 MB free on all three.

And a **baseline of what is already red**, because the monitor's own list contains
checks that have never passed and you will otherwise spend the run blaming
yourself: probe every URL in `common/monitoring-endpoints.nix` once, save the
output, and diff the post-switch runs against *that* rather than against "all
green". On 2026-10-15 four of them were already failing before anything moved:
`https://status.leighhack.org` (the vhost exists on services1's nginx, the
DigitalOcean record does not — and nothing in the repo can see it, because the
zone is not in the flake), and `http://3d-{1,2,3}.int.leighhack.org` (no such names;
the printers are `3d-blue` / `3d-lime` — see `docs/follow-ups.md`).

```bash
for u in $(grep -oE 'https?://[^"]+' common/monitoring-endpoints.nix | tr -d '"' | sort -u); do
  printf '%s %s\n' "$(curl -sk -o /dev/null -m 6 -w '%{http_code}' "$u" || echo ERR)" "$u"
done > /tmp/nu-endpoints-base.txt
```

Then the same four lines on the other two boxes, in one hop each:

```bash
for h in 10.3.1.21 10.3.1.32; do
  timeout 20 ssh -i ~/.ssh/agent-hop-key -o ConnectTimeout=5 -o BatchMode=yes \
    leigh-admin@$h 'echo host=$(hostname); echo gen=$(readlink /run/current-system); echo good=$(readlink /nix/var/nix/profiles/system-good); echo failed=$(systemctl --failed --no-legend | wc -l); echo df=$(df -h / | tail -1 | awk "{print \$5}")'
done
```

And the repo state on all three, which is the thing that bites most often:

```bash
git log -1 --format=%H_%s; git status --short | wc -l; git log --oneline -1 origin/main
for h in 10.3.1.21 10.3.1.32; do
  timeout 20 ssh -i ~/.ssh/agent-hop-key -o ConnectTimeout=5 -o BatchMode=yes \
    leigh-admin@$h 'cd ~/Projects/infrastructure-nix-flake && git log -1 --format=%H_%s && git status --short | wc -l'
done
```

What each answer rules out:

- **`/run/current-system` ≠ `system-good` on any box** → that box is mid-rollback
  or was never confirmed. Do not start the run: the auto-rollback timer is
  *armed* and will reboot that box into the old generation while you are working.
  Land it first (`sudo nixos-confirm`), or reboot it, and re-read.
- **A box with failed units** → you are upgrading on top of an already-broken
  baseline. Note what is failing *before* the change, so the post-upgrade
  failure list has something to diff against. A failure you did not record
  beforehand is indistinguishable from one the upgrade caused.
- **Different `git log -1` per box** → Phase 1 is mandatory, not optional. (On
  2026-10-15 this was true: services1 was two commits ahead of `origin/main`,
  services2 and aibox were one behind services1.)
- **A dirty tree on a box** → `nixos-rebuild --flake .` on a dirty git repo
  includes the working-tree changes in the flake source. Deploying that box
  ships someone else's half-finished work to a live network. (aibox had 71
  staged files on 2026-10-15 — a `crates/whisper-ws/` crate mid-write.) Commit
  it, or stash it, before Phase 1's reset — **`git reset --hard` on a dirty box
  deletes that work and there is no undo.**
- **`df -h /` over 90 % on a box** → a build there will fail partway, possibly
  after activation has started. Build off-box (`just build-remote`) or GC first
  (`common/build-farm.nix` owns the policy; `just gc` is interactive — do not
  run it expecting a non-tty to answer).
- **A surviving `nix eval` / `nix build`** → kill it before issuing another nix
  command. A nix client holds `/nix/var/nix/db/big-lock` for its whole
  evaluation, so an aborted survivor makes every later nix command look hung
  while doing nothing (`docs/nix-dev-workflow.md`).
- **The NAS and the router** — confirm both are answering, because half the
  network's units wait on them:

  ```bash
  timeout 10 ssh -i ~/.ssh/agent-hop-key -o ConnectTimeout=5 root@10.3.1.1 'grep -c . /var/etc/dnsmasq-hosts'
  ls /mnt/filestore /mnt/backups 2>&1 | head          # services1's automounts
  mountpoint -q /mnt/filestore && echo mounted || echo "not mounted (automount idle)"
  ```

  A NAS that is still coming up after a power cut is not a flake fault — see
  `docs/nas-and-borg.md` and `docs/monster-powercut-runbook.md`. `wait-for-nas`
  and `wait-for-network` have `TimeoutStartSec = 0` on purpose: they never give
  up, so a service waiting on them is *supposed* to sit there.

---

## Phase 1 — freeze the source: one commit, everywhere

Do this before anything is built. A build of the wrong tree is a wasted build,
and a switch of the wrong tree is a wrong network.

1. **Land the local work.** `git add` the new files (rule 5 — untracked files are
   invisible to the flake), `just check`, then commit and push. If you are on a
   box with no GitHub key of its own (services2), push through services1 with
   agent forwarding — the exact incantation and its two traps are in
   `machines/services2/README.md`. Guard the push so it can only fast-forward:

   ```bash
   git merge-base --is-ancestor "$(git ls-remote origin refs/heads/main | cut -f1)" HEAD && git push origin HEAD:main
   ```

2. **Put every box on that rev.** Boxes get reset imperatively and periodically;
   that is house style. But check for uncommitted work on each one first (Phase
   0's `git status --short | wc -l`), and stash rather than discard:

   ```bash
   # on each of services2, aibox — after confirming their tree is clean
   git fetch origin && git reset --hard origin/main
   ```

   If a box cannot fetch (no GitHub key of its own), fetch the *rev* from
   services1 into that box's checkout, or `git bundle` it — do not rsync a tree
   as a side effect of a deploy.

3. **Prove it.** All three must print the same hash and a clean tree:

   ```bash
   for h in 10.3.1.20 10.3.1.21 10.3.1.32; do
     timeout 20 ssh -i ~/.ssh/agent-hop-key -o ConnectTimeout=5 -o BatchMode=yes \
       leigh-admin@$h 'cd ~/Projects/infrastructure-nix-flake && echo $(git log -1 --format=%H) $(git status --short | wc -l)'
   done
   ```

   Stop if they differ. A deploy that starts from three trees is not an upgrade,
   it is a lottery.

---

## Phase 2 — move the inputs on purpose

An "upgrade" is only an upgrade if you decide what moves. `nix flake update`
moves **everything**. As of 2026-10-15 all eight inputs are GitHub: commit
`8e0430c` ("flake: move whisper-ws and pi-room-sys in; nothing is read out of
~/Projects") removed the `git+file://` inputs, so `~/Projects/` no longer feeds
the flake and the two things that used to move invisibly with a local checkout
— `crates/whisper-ws/` and `machines/pi-room-sys/` — now move with the **git
tree**, i.e. they are Phase 1's decision, not Phase 2's. Re-check that this is
still true before trusting the table: `grep -c 'file:///' flake.lock`. If it is
non-zero, an `update` moves those to whatever that checkout happens to be, which
is the classic way a network upgrade turns into "and here is what was on my
desk".

| Input | How it is pinned | Moving it means | Verdict for a normal upgrade |
| --- | --- | --- | --- |
| `nixpkgs` (`nixpkgs-unstable`) | floating, in `flake.lock` | every package on every box: the kernel, postgres, nginx, immich, frigate, podman, the Dioxus toolchain | **yes** — this is what an upgrade is |
| `crane`, `sops-nix`, `nixos-utils` | floating, follow `nixpkgs` | the build helpers and the `autoRollback` / `updateContainers` modules this repo depends on | yes, implicitly |
| `strata` (`github:Niko1221/Strata`) | **tracked on `main` on purpose** (`docs/services.md`) | both engines' upstream source; each `strata-package.nix` pins the vendored llama.cpp revision, and the model packs live outside the store | yes, but read the box's `STRATA.md` first — the engine flags were measured per card and do not transfer |
| `door-entry-management-system` | floating GitHub | the door system's own nix | yes |
| `llama-cpp` | **pinned to a model branch** (`github:ifm-ai/llama.cpp/model/K2Horizon`) | nothing, unless you re-pin | **leave pinned.** A branch pin is a fact about which model was built |
| *(nothing)* | in-tree since `8e0430c` | `crates/whisper-ws/` and `machines/pi-room-sys/` are flake **source** now, so they move with the commit, not the lock: a change there is a change to the netboot image and the whisper gateway, and it is rebuilt by every box that builds, not only aibox | this is Phase 1's territory — do not let a half-finished crate in the tree reach a switch |

Then commit `flake.lock` and re-run Phase 1's proof. The lock is the network's
input set; a lock that differs per box is invariant 2 broken.

Sanity-check what actually moved, without evaluating:

```bash
git diff --stat -- flake.lock
grep -n '"rev"' flake.lock | wc -l
```

Note that the lock keeps stale nodes (there is a dead `nixpkgs` node from before
the input rename alongside the live `nixpkgs_2`). Read `root.inputs` to know
which node is live before reading a `rev` — a stale node's date is not the
network's date.

---

## Phase 3 — build everything, activate nothing

`just check-build` builds all four machines' system toplevels and activates
nothing. It is the gate that catches "evaluates but will not build", which
`nix flake check` does not. The fourth is `gw`, the replacement router: it builds
here because it is deployed, and it is *not* in the deploy order below — it runs in
co-existence mode beside the live router and `machines/gw/README.md` owns when it
may be switched over.

```bash
just check          # nix flake check + the assertion evals + alejandra --check
just check-build    # all four machines, nothing activated
```

**`just check-build` used to be `nixos-rebuild dry-build`, and it never built
anything.** This box's `nixos-rebuild` is nixos-rebuild-ng, whose man page
defines `dry-build` as "Show what store paths would be built or downloaded by any
of the operations above, but otherwise do nothing". The first run of this runbook
(2026-10-15) measured it: all three machines, a 786-derivation "these derivations
will be built" list for services1, **exit 0 in 57 seconds, nothing compiled, the
builder at load 0.00**. The gate that was supposed to catch "evaluates but will
not build" caught nothing. It is now `nix build` on
`nixosConfigurations.<m>.config.system.build.toplevel` — the same derivation,
with neither of the two bad alternatives: `nixos-rebuild build`/`boot` do compile,
but they write a boot loader entry, so the next reboot comes up on a configuration
nobody ever activated. A toplevel build touches no profile, no boot entry, no
`/run/current-system`, so it cannot arm the auto-rollback timer either.

**Budget it, do not guess it.** The 2026-10-15 run (one nixpkgs bump, 2026-10-08 →
2026-10-14, plus crane/nixos-utils/sops-nix) took **77 minutes** and produced
2 310 built derivations: services1 ~10 min (786 drvs), services2 ~4 min (121),
aibox ~63 min (1 404, of which the strata CUDA engine and the pi-room-sys
`mksquashfs` were the long poles). Run it detached to a log and poll; a tool call
is not a place to sit through that.

**The baseline is not green, and you must know it before you read a failure as
yours.** As of the 2026-10-08 lock, from services1, `nix flake check --no-build`
died on `services2` with `error: path '...-sops-secret-grafana_secret_key.drv' is
not valid` (from `services.grafana.settings.security.secret_key` in
`machines/services2/monitoring.nix`) having checked `services1` only —
`nix flake check` stops at the first failing output, so a "passing-looking" check
was never a network-wide check. **On the 2026-10-14 lock that error did not
reproduce: `nix flake check` reached all five configurations and exited 0** (the
Grafana plaintext-`secret_key` *warning* is still there, and is open in
`docs/follow-ups.md`). Re-read the baseline at the new lock instead of inheriting
the sentence, and verify against clean HEAD (`git stash`) before hunting any error
you think your change caused (rule 9). `gw` is deliberately in neither gate
(`machines/gw/README.md`).

Build discipline for the long part:

- **Offload it, but expect half of it local anyway.** `nix.distributedBuilds = true`
  and nix 2.34 *prefers* the external builder, so a plain build on services1 does
  run much of itself on aibox and copies the result back — but the
  fixed-output download derivations (`fetchurl`, every npm `.tgz`, every crate)
  are `preferLocalBuild` and run on the client: the 2026-10-15 run (when the
  builder was services2) measured 754 local vs 162 remote for services1's
  toplevel, and 411 local vs 20 remote for aibox's, with the builder sitting at
  load 0.0 while services1 sat at load 20-37.
  `just build-remote` (`--max-jobs 0`) removes the *local fallback*; it does not
  force a derivation that nixpkgs marked `preferLocalBuild` onto the builder.
  **Re-read the headroom numbers; they are a fact about a day.** On 2026-10-15
  services1's root measured 52 % before the run and 77 % after (a full
  three-machine build costs services1 ~25 GB of store), services2 45 %, aibox
  82 %. Aibox is the builder now, so its store is the one that grows, and strata's
  parked model data sits outside it. `df -h` on every box rather than inheriting
  a comment written months ago (including the 211 GB claim in
  `machines/services2/README.md`).
- **Judge health by latency, not load.** services1's load average reached 37 during
  the aibox build and every SSO vhost still answered in 8-50 ms (`curl -w
  %{time_total}`). Load on a box whose baseline is frigate's ffmpeg workers is not
  a signal; before you decide the box is unhappy, probe the endpoints and read the
  time.
- **Watch the builder's memory, not just its load.** Since 2026-10-17 the builder
  is aibox: 59 GiB and 6 cores, with strata parked, but also frigate, immich,
  whisper and llama-server on the same box.  `free -g` on aibox belongs in the poll
  loop next to the log tail; a build storm there degrades the apps rather than the
  services box, and llama-server's GTT is what to stop if memory gets tight.  The
  old builder (services2) was the worse case, kept here as the reason not to hand
  the role back: it ran strata at 24-26 GiB RSS against 30 GiB of RAM, and a full
  build pushed it to 0 MiB free and 6 GiB of swap, with the peak `mksquashfs`
  (4.4 GB RSS, 7 cores) building the pi-room-sys netboot image.  Nothing OOMed,
  but that was a coin-toss; if it had started thrashing, the first thing to lose
  was strata (26 GiB, unmonitored by gatus) and the second the monitor itself.
- **Poll, never sleep** (rule 11). Run it detached to a log
  (the `systemd-run` recipe is in `docs/nix-dev-workflow.md`; `setsid nohup … &`
  also survives an aborted tool call), then per tool call: is the process still
  running? read the log tail; stop the moment it exits. A poll that returns early
  on "log line count changed" or "error: in the log" is worth more than a fixed
  sleep, because the failure you are hunting usually lands in the first seconds.
- **The first build after an input bump recompiles the shared crane workspace
  deps** (`crates-workspace-deps-0.1.0` and friends), the strata CUDA engine on
  **both** GPU boxes, and the netboot squashfs. That is expected cost, not a fault.
- Do not run `nix eval --impure --expr` with `builtins.getFlake` to "see what
  will change" (rule 21): it is a cold evaluation of every machine and crate,
  slower than the switch that answers the same question, and its aborted
  survivor holds the big lock. `nixos-rebuild dry-run` is the same evaluation the
  deploy does, so it stays cache-warm — but remember it **builds nothing**.

If `check-build` fails on one machine, **stop**: fix it, or revert the change,
before any box moves. A half-landed network is worse than an un-landed one.

Cheap, cache-warm things worth knowing before you switch (all of these are one
eval, and they answer questions that otherwise get asked at 20:00 during an
activation):

```bash
for m in services1 services2 aibox; do
  echo -n "$m new-kernel: "
  nix eval --raw ".#nixosConfigurations.$m.config.boot.kernelPackages.kernel.version"
done; uname -r
# and, from the built toplevel, whether a database major moved:
nix path-info -r <toplevel> | grep -oE 'postgresql-[0-9.]+' | sort -u
```

The 2026-10-15 run: services1/services2 6.18.55 → **6.18.56**, aibox 7.2.9 →
**7.2.10** — three patch bumps, so `switch` and no reboot; postgresql stayed
**17.11** on services1, so no migration. immich is a pinned *container* image
(v3.0.3), so a nixpkgs bump cannot move it and the mobile-client check does not
apply.

---

## Phase 4 — the order, and why that order

The network is not three independent boxes:

```
router 10.3.1.1  ── DNS for every *.int name + the DHCP reservation that gives
  │                 every box its address. Never touched by this run.
  ├── NAS 10.3.1.6 ── NFS exports; services1 and aibox mount it; borg lives here
  │        └── monster1 10.3.1.11 ── VM disks are on the NAS (nas2-nfs)
  ├── services1 10.3.1.20 ── nginx (TLS terminator for the whole network),
  │     └── proxies → services2 (grafana/prometheus/gatus/status)
  │     └── proxies → aibox (immich/frigate/whisper/status)
  ├── services2 10.3.1.21 ── the monitor (gatus → Slack) AND the build farm
  └── aibox 10.3.1.32 ── the netboot server → clients 10.3.14.101–110
```

**Default order: services1 → aibox → services2.**

- **services1 first.** It is the choke point: every public app, every OIDC
  redirect, every `*.int` vhost and the ACME challenge live there. Landing it
  first means the new nginx/vhost/cert set exists before the leaf boxes change
  the ports they serve. If it fails, auto-rollback restores it and nothing else
  has moved — the run stops at the cheapest possible point.
- **aibox second.** Its apps are proxied by services1, whose new nginx is
  already up. It is a leaf: nothing else waits on it except the netboot clients,
  which only re-read the export on their next boot.
- **services2 last**, for two reasons that are the same box. It hosts the only
  live **Strata** engine, whose ~28 GiB RSS is most of the box's RAM, so its
  switch is the one most likely to need memory that is already spoken for; doing
  it last means a failure there leaves the other boxes landed and working. And it
  is a leaf: nothing else waits on it now that gatus, Prometheus and Grafana are
  on services1 — the witness role it used to have is gone, not deferred.
  (When services2 *was* the builder, the extra reason was that switching it
  stopped the daemon every delegated build used. That is aibox now, and aibox is
  second, so Phase 3 must be finished before it moves. It always is.)

**The exception that overrides the order:** if the upgrade changes an *interface*
— a port, an address in `common/hosts.nix`, an API path, a VLAN — then the box
that **provides** it must land before the box that **consumes** it, or there is
a window in which nginx proxies a port nothing serves (404s and a gatus incident
for every endpoint in that group). Read the diff before choosing:

```bash
git diff <old-commit>..<new-commit> -- common/hosts.nix common/exporters.nix \
  common/monitoring-endpoints.nix machines/services1/services/monitoring.nix
```

If a port moved on services2, services2 goes before services1's nginx change. If an
address moved, the table that holds it (`common/dns-records.nix` for a device,
`common/int-dns.nix` for a `*.int` name) and `just switch-gw` go first too, or every
client resolves the old address while the box answers the new one
(`docs/router-and-dns.md`).

**Never two boxes mid-switch at once.** Not "for speed": a rollback reboots a
box out from under the other one's activation, and a shared store copy through a
box that is itself switching is a race you cannot read afterwards.

---

## Phase 5 — land one box

The command, with the GOLDEN RULE chained (`--flake .` resolves against the cwd,
so `cd` first over ssh):

```bash
# services1 (you are on it)
sudo nixos-rebuild switch --flake .#services1 && sudo nixos-confirm

# services2, aibox — one ssh invocation, chained
ssh -i ~/.ssh/agent-hop-key -o ConnectTimeout=5 leigh-admin@10.3.1.21 \
  'cd ~/Projects/infrastructure-nix-flake && sudo nixos-rebuild switch --flake .#services2 && sudo nixos-confirm'
```

**`&&` is not decoration: never confirm a switch that failed.** A chained `;` (or
the `systemd-run` recipe in `docs/nix-dev-workflow.md`, which ran `nixos-confirm`
unconditionally until 2026-10-15) marks the generation *currently in
`/run/current-system`* good even when activation aborted halfway — and that is
exactly the state auto-rollback exists to catch. aibox hit this on the first run:
activation died on a mount at status 4, `/run/current-system` had already been
flipped to the new toplevel, and the chained confirm marked it good, disarming the
only safety net. Use `&&`, or in a script `if [ "$SWITCH_EXIT" -eq 0 ]; then
nixos-confirm; fi`, and treat `SWITCH_EXIT!=0` as "this box is half-activated,
read the log before anything else".

**A switch that outlives the tool call should be a detached unit, not a long ssh.**
The 2026-10-15 run used the `systemd-run --no-block` recipe (in
`docs/nix-dev-workflow.md`) with the absolute repo path and `HOME=/root`, logging
to `/tmp/nu-switch-<machine>.log`, and polled `systemctl is-active <unit>` plus the
log. Two things that recipe does not tell you:

- `--setenv=HOME=/root` + `cd ~/Projects/…` resolves to `/root/Projects/…`. Use
  the absolute path.
- **Running the deploy as root needs `safe.directory` for the repo in root's git
  config**, or nix refuses the checkout with `repository path … is not owned by
  current user (libgit2 error code = 7)`. services1 and services2 have it in
  `/root/.gitconfig`; **aibox did not**, and its first switch died in the first
  second on exactly that. It is imperative, undeclared, per-host state that the
  deploy depends on — check it in Phase 0 (below) and see `docs/follow-ups.md`
  about declaring it.

Notes:

- **`switch`, not `boot`, unless the kernel changed.** `switch` activates now and
  links the profile; `boot` only writes the boot entry. A nixpkgs bump that moves
  the kernel leaves the *running* kernel alone until a reboot, and anything
  compiled against the new one — the out-of-tree modules, i.e. services2's
  NVIDIA driver for the RTX 3060 — will not load on the running kernel. Ask
  before switching, cheaply, by comparing the newest kernel in the ESP with the
  running one:

  ```bash
  uname -r
  sudo ls -1 /boot/EFI/nixos | grep -o 'linux-[0-9.]*-' | sort -V | tail -1
  ```

  Different → this box needs `switch` **then a reboot** (or `boot` + reboot).
  Rebooting services1 takes nginx, postgres, SSO and every app down for the
  window of the boot — say which, and to whom (gatus will fire a Slack incident
  for every endpoint; that is the noise, not an incident). Rebooting aibox does
  not reboot its netboot clients.
- **Do not add anything to `nixbld`, ever** (rule 7 / `docs/nix-build-sigkill-2026-10-11.md`): the group is the pool of UIDs builds run as and nix SIGKILLs every process of the build UID before and after each build. `common/nixbld.nix` fails the eval if you ask, and `nixbld-guard.timer` undoes an imperative `usermod` within fifteen minutes.
- **Interrupting a build is harmless** now — the sweep lands on a throwaway `nixbldN` uid.
- If a switch is aborted by the tool-call timeout, **the switch is still running**
  on that box. Find it (`pgrep -a -f nixos-rebuild`), and re-read
  `/run/current-system` before doing anything else: a half-activated generation
  with no `nixos-confirm` is a box that will roll itself back five minutes later.

---

## Phase 6 — verify the box before moving to the next

Per box, after the switch, in one bounded poll each:

```bash
readlink /run/current-system /nix/var/nix/profiles/system-good   # equal = confirmed
readlink /run/current-system | grep -o '26\.11\.[0-9]*\.[0-9a-f]*'   # the new version string
systemctl --failed --no-pager --no-legend
journalctl -b -1 -p err -n 50 --no-pager       # errors from the boot you are in
```

Then the things this network specifically needs:

- **The NAS rule** (rule 12): every service that needs the NAS declares `after`
  **and** `requires` `wait-for-nas.service` and keeps restarting forever. If a
  NAS-dependent service is failed, read `Result=` and the second attempt — the
  never-give-up policy (`Restart=always`, `StartLimitIntervalSec=0` in
  `common/containers.nix`) means a first-start race with a setup unit is
  legitimate (`docs/work-log-2026-10.md`).
- **Rendered units, not typed ones** (rule 10): if the upgrade changed an
  `ExecStart` or a flag, read it back from a real build —
  `/etc/systemd/system/<unit>.service` — because NixOS normalises quoting and
  `${...}` differently from what you wrote.
- **New/renamed vhosts need the table** (rule 14): a `*.int` vhost is its `server_name`
  in `machines/services1/services/…` **and** its label in `common/int-dns.nix`; the
  second is checked against the first at eval (`machines/services1/int-dns.nix`), and
  the router gets it from `just switch-gw`. Public `*.leighhack.org` names are
  hand-added in the DigitalOcean zone, which is outside every gate this repo has.
  Without a record the vhost is NXDOMAIN, not 404 — a different failure.
- **Postgres**: if the bump moved a **major** version, the existing DBs are on
  the old one and activation refuses — the fix is
  `postgresql-alt-install` / `postgresql-update-services` before the switch, and
  the collation-version trap (`ALTER DATABASE template1 REFRESH COLLATION VERSION`)
  after a glibc upgrade. `machines/services1/services/postgres.nix` pins
  `pkgs.postgresql_17` deliberately; check the diff before assuming it moved.
- **Containers**: `system.updateContainers` runs nightly on both boxes
  (`machines/services1/containers.nix`); a bump does not re-pull an image by
  itself. Check the `podman-*` units are the ones you expect, and that the
  restart policy survived.
- **The monitor's view**: after the last box, read what gatus saw during the run
  — `journalctl -u gatus` on services2, or the Slack thread. It is the network's
  own record of the sequence, and it is the only witness to services1's outages.
- **The monitor's own health, queried correctly** (measured 2026-10-15, because
  every obvious guess was wrong): prometheus listens on **`0.0.0.0:9091`**, not
  9090; gatus binds **`10.3.1.21:8080`** and not loopback, so `curl
  127.0.0.1:8080` from services2 fails while the box is healthy; and gatus's
  `/api/v1/endpoints/status` returns **404** on this build. The query that
  actually answers "is anything down" is prometheus's:

  ```bash
  ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.21 \
    'curl -s -m 10 "http://127.0.0.1:9091/api/v1/query?query=count(up)";      \
     curl -s -m 10 "http://127.0.0.1:9091/api/v1/query?query=count(up==0)"'
  ```

  2026-10-15: 12 targets, empty `up==0` before and after all three switches.

A box that switched but whose endpoints you have not re-checked is not upgraded.
`curl -fsS -m 5` the handful of URLs that box serves (the list is
`common/monitoring-endpoints.nix`), and the OIDC apps that matter to the run —
then **diff that probe against Phase 0's baseline file, not against green**. On
2026-10-15 the probe set was byte-identical after services1, after services2 and
after aibox, which is the cheapest proof of "no interface moved" the run has.

---

## Phase 7 — the network's own after-effects

These are not part of the three switches, and skipping them is what makes the
run "finished but not upgraded":

- **DNS**: nothing to run. A `*.int` name exists because
  `common/int-dns.nix` says so and `just switch-gw` deployed it, and you verify against
  what the router answers rather than against the file:

  ```bash
  dig +short @10.3.1.1 <name>.int.leighhack.org A AAAA     # there is a dig on gw
  sudo -n grep <name> /etc/dnsmasq-int-vhosts.hosts        # generated; never edited
  getent ahosts <name>.int.leighhack.org                   # from any LAN box
  ```

  If the name is a vhost and the answer is empty, services1 should not have evaluated
  (the guard compares the two lists) — so look for a deploy of gw that did not happen.
  **The public `*.leighhack.org` half is still hand-made in DigitalOcean and still
  outside every gate**: a public vhost with no record passes every check this repo has
  and stays NXDOMAIN forever (that is how `status.leighhack.org` was found, with a gatus
  check red since the day it was written).
- **Netboot**: the image is `machines/pi-room-sys/`, an **in-tree machine** —
  edit it and rebuild, there is no pin to move any more (it used to be a
  `git+file://` input with an `update-netboot-input` recipe to drag `flake.lock`
  along, and the pin going stale in silence is why that is gone). `just switch-netboot`
  does a full `nixos-rebuild switch --flake .` (no machine target — run it on aibox),
  remounts `/exports/netboot-squashfs`, and **reboots all ten clients**. Do it
  only if the upgrade changed the image and someone asked for the clients to move
  now; otherwise let them pick it up on their next boot. Boot flow and what aibox
  exports: `machines/pi-room-sys/README.md`.
- **The netboot export makes every aibox switch fail, and the fix is the umount,
  not the reboot.** `/exports/netboot-squashfs` is a `fileSystems` bind mount of a
  **store path baked at build time**, so a nixpkgs bump changes the mount's source
  on every single upgrade. The unit is boot-generated from `/etc/fstab` (it has no
  file in the toplevel's `etc/systemd/system`), so activation's `systemctl restart`
  of it fails and `switch-to-configuration` exits **4** — on 2026-10-15 that was
  the only thing that failed aibox's switch, and it failed *after* `/run/current-system`
  had been flipped and after nginx/sshd/immich/strata had restarted. The dance
  `just switch-netboot` already documents is the fix, and it does **not** need the
  client reboot when you are only re-pointing the export:

  ```bash
  sudo ss -tn sport = :2049          # who is mounting it right now; empty = nobody
  sudo umount -f -l /exports/netboot-squashfs && sudo mount -a
  findmnt -no SOURCE /exports/netboot-squashfs   # must now be the NEW store path
  sudo nixos-rebuild switch --flake .#aibox && sudo nixos-confirm   # now exits 0
  ```

  Lazy-umounting is safe for running clients (their open handles survive; new
  mounts get the new image), and the clients pick the new image up on their next
  boot. Check `ss -tn sport = :2049` first so you are not doing it under a client
  that is mid-boot. A client that mounts the *previous* image boots the previous
  generation rather than panicking (`init=/nix/store/.netboot-closure/init`, see
  `machines/pi-room-sys/README.md`), but one whose mount fails because the export
  is left unmounted cannot boot at all. Declaring the export so it stops moving is
  open in `docs/follow-ups.md`.
- **monster1**: nothing declarative. If the NAS was offline at any point in the
  run, expect `storage 'nas2-nfs' is not online` on VM starts and check
  `pvesm status` / `mount | grep nas2-nfs` **before** blaming a VM. HA-managed
  VMs are started with `ha-manager set vm:<id> --state started`, never `qm start`;
  clearing `error` state is disable → wait → start, not both in one loop.
  Full procedure: `docs/monster-powercut-runbook.md`.
- **Printers**: nothing flake-managed; `moonraker-exporter` polls fixed IPv4s on
  purpose. If 3d-blue is still off-network, say so and leave it.
- **Switches**: nothing. No NVRAM writes. Their RADIUS login goes to
  authentik at 10.3.1.36 and falls back to the switch's local account database,
  which is why `Can't reach RADIUS server.` prints **on a successful login**
  (`docs/procurve-switches.md`) — noise, not an incident. Do not "fix" it.
- **The authentik appliance (10.3.1.36)**: out of band, not hop-key reachable, and
  the live IdP. The flake's second authentik (`machines/services1/services/authentik.nix`)
  is a **parked shadow** — do not un-park it as part of an upgrade. Two
  authentik instances sharing a database's secrets is how the switch logins break
  (`docs/authentik-container-migration.md`).
- **The UniFi controller** (`docs/unifi-os-server.md`) if the upgrade touched it:
  the appliance is a *downgrade* of the running controller and its `.unf` backups
  are encrypted with a key that is not on disk.

---

## Failure modes, and what to do about each

| What can go wrong | How you see it | What to do |
| --- | --- | --- |
| **A rollback fires mid-run** | `/run/current-system` is the *old* generation; the box rebooted | Prevent it: `nixos-confirm` in the same invocation as every switch. If it happened, the switch's changes are gone — re-run, do not patch on top |
| **A switch confirmed that should not have been** | `SWITCH_EXIT=4` (or 1) in the log and `cur == good` anyway | The `&&` was a `;`. The half-activated generation is now marked good and the timer is disarmed. Read the log, fix the cause, switch again — and never confirm a non-zero exit |
| **`sudo nixos-rebuild --flake .` dies in one second** | `repository path … is not owned by current user (libgit2 error code = 7)` | Root has no `safe.directory` for the repo on that box (aibox, 2026-10-15). `sudo git config --global --add safe.directory <path>`; Phase 0 checks all three |
| **aibox's switch exits 4 after restarting everything** | `Failed to restart exports-netboot\x2dsquashfs.mount` | The netboot bind mount's source is a store path that moved. `umount -f -l` + `mount -a`, then switch again (Phase 7). No client reboot needed to re-point the export |
| **The switch looks hung on aibox** | the unit stays `activating`, `building '…strata…'` / `mksquashfs` in the log | aibox builds its own toplevel locally (it is the builder for others, not for itself), so it compiles what it does not have **during activation** (237+ derivations, ~15 min on 2026-10-15). Pre-seed it instead: `nix copy --to ssh://leigh-admin@10.3.1.32 <toplevel>` |
| **Boxes on different commits** | Phase 1's proof fails | Stop. Fix Phase 1 before building. Deploying three trees is three networks (rule 20) |
| **A dirty tree on a box** | `git status --short` non-empty | Commit or stash **before** any `reset --hard`. A reset on a dirty box deletes in-progress work with no undo |
| **A stale `origin/main` ref on a box with no GitHub key** | `git status -sb` says `ahead 1` forever | That is a stale ref, not an unpushed commit (`machines/services2/README.md`); push through services1 |
| **`git push` dies with `Permission denied (publickey)` on the box that has a key** | an exported `GIT_SSH_COMMAND` leaked onto the GitHub remote | Set `GIT_SSH_COMMAND` inline on the LAN `git fetch` only; the LAN fetch also needs `IdentitiesOnly=yes` (services1 offers several identities → `Too many authentication failures`) |
| **A nix command hangs, doing nothing** | no CPU, no output, no timeout | A survivor of an aborted nix call holds `/nix/var/nix/db/big-lock`. `pgrep -a -f "nix eval|nix build"` and kill it before the next nix command |
| **A build copies 3 GB of `crates/target/` into the store** | you evaluated through a path literal | Never evaluate this repo through a path literal (rule 8). Read values out of the flake |
| **A delegated build silently fell back to local** | the wrong disk fills; the builder idles | Poll for the build, not the clock; `nix flake`/`nixos-rebuild` log lines name the builder. Flip `nix.distributedBuilds = false` only deliberately |
| **Disk full mid-build** | `df -h /` climbing; build dies at a copy | Build off-box (`just build-remote`); GC per `common/build-farm.nix` (interactive — `just gc` will not answer for a non-tty) |
| **The builder is switched while a build is delegated to it** | the build dies with a daemon/ssh error | Finish every build (Phase 3) before any switch; the builder is aibox (second in the order), so services1 → aibox → services2 still leaves the phase boundary where it belongs |
| **A storm of transient failures after services1's switch** | dozens of units restarting | Expected: activation restarts the whole stack, and the never-give-up policy retries. Read `Result=` and the *second* attempt; a first-start race with a setup unit is legitimate |
| **A postgres major bump** | activation fails on the DBs; `psql` says the DBs are on the old version | `postgresql-alt-install` / `postgresql-update-services` **before** the switch; then the `template1` collation refresh. Do not `CREATE DATABASE` and hope |
| **A kernel bump with out-of-tree modules** | the NVIDIA driver on services2 will not load; `uname -r` ≠ newest `/boot/EFI/nixos` kernel | `switch` then reboot that box, in the same chained invocation, and say what goes down for the boot window |
| **A new vhost is NXDOMAIN** | `curl` fails at name resolution, not HTTP | The vhost and `common/int-dns.nix` are one change and gw is the other deploy (rule 14); public names are hand-added in DigitalOcean. The wildcard ACME cert needs no work |
| **ACME renews and rate-limits** | cert files change mtime; a renewal storm | Do not force renewals. The wildcard covers `*.{,int.}leighhack.org`; only touch certs if the diff demands it |
| **immich server moved, mobile clients did not** | the app refuses to connect (version mismatch) | `newVersionCheck.enable = false` is set in `machines/aibox/immich.nix`; a *major* bump still needs the clients. Say so before switching aibox |
| **A strata engine fails to start** | `ConditionPathExists` on the pack index is false | Model data is deliberately outside the store and prepared once by hand (`STRATA.md` per box). A missing pack is not a build failure — the unit is *supposed* not to start |
| **A Dioxus SPA looks unchanged** | the UI is the old one | The four Dioxus `dist/` are git-ignored and built by the flake; `network-status/frontend/dist/` is **committed** (rule 19). Rebuild and commit the dist |
| **A switch login prints `Can't reach RADIUS server.`** | on an otherwise successful login | Firmware's generic "authentication failed" message. Noise, not an incident (`docs/procurve-switches.md`) |
| **A NAS mount is empty under a service** | the service sees an empty `/mnt/...` | `x-systemd.automount` in a unit-file mount's `Options=` is dead config — `common/nas.nix` defines the `.automount` units explicitly. Check the automount unit, then the NAS, before the flake |
| **VMs fail to start on monster1** | `storage 'nas2-nfs' is not online` | The NAS is still booting. Check `pvesm status` before blaming VMs (`docs/monster-powercut-runbook.md`) |
| **A secret you did not mean to print** | it is in the transcript | Rule 13: reference secret *paths*, never contents. A leaked RADIUS secret and a leaked DO token are already open items in `docs/follow-ups.md` — rotate, do not re-print |
| **The whole run is half-landed** | some boxes new, some old, and a diff you cannot attribute | Stop conditions below. Put the untouched boxes back on the old commit, or land them; never leave the network split with no note |

---

## Stop conditions

Abort the run, in this order of preference:

1. **A box failed activation twice in a row** → stop. Do not switch the next box
   against a network that is half-moved. Revert the commit (or the input bump) if
   the failure is in the change, and re-run Phase 3.
2. **A rollback fired on a box you already landed** → the network is now
   inconsistent by construction. Land that box again, or roll the others back to
   the previous commit. Do not continue from a box whose generation you cannot
   name.
3. **The router, the NAS, or the appliance went down** at any point → the run is
   over. Those three are not in the flake's deploy set; if one of them moved,
   something else moved it, and you need to know what before you switch anything
   else.
4. **You cannot name the generation every box is on** (`readlink` + `git log -1`
   on all three) → stop and re-read Phase 0. Half of this runbook's value is
   that those six numbers are known at every point.

---

## Record it

A network upgrade is the thing a later agent most wants a narrative for.
Append to `docs/work-log-2026-10.md`: the commit range, which inputs moved, the
order actually used (and why, if it was not the default), what each box's
version string went from/to, what needed a reboot, what failed and how it was
fixed, and what the monitor saw. Open anything you did not finish in
`docs/follow-ups.md`. If a rule below in `AGENTS.md` turned out to be wrong or
missing, fix the doc that owns the topic and leave a one-line pointer in
`AGENTS.md` — do not grow the rules file.

---

## Appendix — the state on 2026-10-15, and the per-box quirks

The snapshot Phase 0 would have produced that day, kept because it is the
baseline the *next* run will diff against:

- All three boxes ran `26.11.20261008.8edc0c7`, and the lock's live nixpkgs node
  (`root.inputs.nixpkgs` → `nixpkgs_2`, rev `8edc0c72e3…`) matches — **the
  network was internally consistent on inputs**. The drift that day was in the
  git tree, not the lock.
- services1's HEAD was one commit ahead of a **stale** `origin/main` ref, and the
  remote had in fact moved to `8e0430c` — a commit no local checkout had. So the
  push needed a fetch and a rebase, not a fast-forward, and the stale-ref trap
  (`machines/services2/README.md`) bites on push as well as on `git status -sb`.
  services2 and aibox checkouts were at `1009a61`, one behind services1; aibox's
  tree had 71 staged files, `crates/whisper-ws/` mid-write — it was mid-way through
  the same vendoring move that `8e0430c` then landed on `main`.
- Disk: services1 52 % (100 GB free), services2 45 %, aibox 82 %. `/nix/store`
  27 GB; 219 `system-*` profiles on services1 (the GC policy in
  `common/build-farm.nix` is what keeps that from being a surprise).
- services1 had no failed units; `uname -r` 6.18.55 matched the newest kernel in
  `/boot/EFI/nixos`, so no reboot was pending there.
- `nix flake check --no-build` died at services2's grafana drv (above), having
  checked services1 only.

Per-box quirks that the run has to respect, in one line each — the box's own
README owns the detail:

- **services1** — since `8e0430c` nothing is read out of `~/Projects/`, so this
  is no longer the only box that can evaluate the flake; it is still the box you
  start from, because it is the choke point and the one with the headroom. Also:
  the tailscale client, the secrets habits (`machines/services1/README.md`).
- **services2** — the RTX 3060 driver, a 16 GiB **swap file** (no swap partition;
  strata alone measured 28.3 GiB RSS against 30 GiB RAM), and the only live Strata
  engine. It was the builder and the monitor until 2026-10-17; both roles have
  moved (builder → aibox, monitor → services1). `machines/services2/README.md`
  still says `just check` cannot run here because it evaluates aibox through the
  local `pi-room-sys` input — **that is stale since `8e0430c`**; re-run `just
  check` on the box rather than inheriting the sentence. No GitHub key of its own
  → push through services1.
- **aibox** — `/etc/hosts` maps `aibox` to `127.0.0.2`, so use the IPs.  The
  NVIDIA `nvidia.nix` + `legacy_580` patch on this box is **dead config** (the
  GTX 1060 was removed — `docs/gtx1060-followup.md`); the live GPU box for that
  driver is services2.  Since 2026-10-17 it is the network's **builder** and strata
  is parked here (6c/12t, 59 GiB).  The netboot export and the ten clients live
  here, and that export is what makes a plain switch exit 4 (Phase 7). No
  tailscale unit at all.

---

## What the 2026-10-15 run left behind — the baseline the next run diffs against

First execution of this runbook, start to finish. Commit range `e2b248d` →
`192058d` (plus `2e93a4b` tools, `7a1dc29` justfile); inputs moved: nixpkgs
`8edc0c72e3a` (2026-10-08) → `319e4cebef1` (2026-10-14), crane → `bd63bf1d0a0`,
nixos-utils → `cc0052bc235`, sops-nix → `44931708c62`; **strata,
nix-software-center and door-entry did not move** (offered to the same `nix flake
update`, unchanged — strata's `main` had not advanced since 2026-10-13, so neither
engine's flags changed and the per-card measurements in the `STRATA.md`s still
describe what is deployed); llama-cpp left pinned on purpose.

All three boxes: `26.11.20261008.8edc0c7` → **`26.11.20261014.319e4ce`**, 0 failed
units, `cur == good`, `auto-rollback.timer Result=success`, **no reboots** (kernel
bumps were patches: 6.18.55→6.18.56 on services1/services2, 7.2.9→7.2.10 on
aibox). **Order used: services1 → services2 → aibox, which deviates from the
stated default** (services1 → aibox → services2). It was benign here — Phase 3 had
finished so nothing was delegated to the builder, and the only thing services2
provided to the other two was the monitoring that aibox's switch would have wanted
as a witness — but it is a deviation, and the reason it is the *wrong* way to deviate
is in Phase 4: switching the monitor early blinds the run for every box after it.
(That was the arrangement on 2026-10-15; since 2026-10-17 the monitor and the
builder are both elsewhere — services1 and aibox — so re-read the order rationale
above rather than this paragraph before the next run.)
Post-run endpoint probe identical to the pre-run baseline; prometheus `count(up)`
12, `count(up==0)` empty.

Four things the runbook said that turned out to be false, all corrected above:
`dry-build` builds nothing (Phase 3); `check-build` is a ~75-minute job, not a
"few minutes" one (Phase 3); aibox's switch fails on the netboot mount until you
re-point it (Phase 7); and the deploy depends on per-host imperative state
(root's `safe.directory`) that only two of the three boxes had (Phase 0).
