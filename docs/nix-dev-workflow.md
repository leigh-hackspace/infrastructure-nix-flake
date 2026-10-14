# Nix dev workflow — commands, how to read the result, and the ways it goes wrong

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: run
`just check` before deploying, never evaluate this repo through a path literal,
`nix flake check --no-build` must stay warning-free, always read back the rendered
unit after editing `ExecStart`, never put a real account in `nixbld`, and kill the
survivor of an aborted nix tool call before issuing another nix command. This file
has the commands and the full gotchas.

The flake is pure and its sources come from the **git tree**, so everything below
runs from this repo root, and untracked files are invisible to it (`git add` new
`.nix` files first).

## Test a change without applying

The standard, preferred way:

```bash
nixos-rebuild dry-run --flake .            # services1 (the default)
nixos-rebuild dry-run --flake .#aibox      # another machine
nixos-rebuild build --flake .#aibox        # build the toplevel locally, print the store path
```

`dry-run` alone may not build all dependencies, so for anything touching
packages/bins run `build` and inspect the result under `/nix/store/` (the path is
printed on success). `--target-host` targets a remote host without deploying, e.g.
`nixos-rebuild dry-run --target-host leigh-admin@aibox ...` (log in as
`leigh-admin` — root is not keyed — with its passwordless sudo), but local `build`
is enough to validate config.

`just check` = flake evaluation + the config assertions in
`machines/services1/lib/check-*.nix` and `common/check-printer-states.nix` +
`alejandra --check` over every tracked `.nix`. `just check-build` =
`nixos-rebuild dry-build` for all three machines: compiles everything, activates
nothing. Use the latter whenever anything is built rather than just evaluated.

## Inspect the generated output (the reliable way to know what a service runs)

Once you have a system path `$SYS`:

```bash
cat $SYS/etc/systemd/system/<unit>.service   # exact rendered unit
grep -rn something $SYS/etc/                 # search the built config
```

**Editing a service's `ExecStart`/flags: always re-run `nixos-rebuild build` and
read back the rendered unit.** NixOS normalises quoting and expands `${...}` and
line-continuations differently from what you typed, so the file you wrote is not
evidence of what runs. This is how you confirm what a service actually runs, what
port it binds and what paths/flags end up in `ExecStart`.

## Extract a single value or package

```bash
nix build --no-link --out-link /tmp/aibox-sys .#aibox
ls /tmp/aibox-sys/bin            # symlinks into the system
```

Gotcha: `flake.outputs` is a **function** (args `{ nixpkgs, nixos-utils,
llama-cpp, ... }`) and `flake.nix` itself is a plain set — import it with no args,
then call `outputs flake.inputs`. But evaluating
`nixosConfigurations.aibox.config` that way fails with `attribute 'lib' missing`,
because `specialArgs`/`nixpkgs.lib` need the flake-resolved inputs that
`nixos-rebuild` provides. So use the flake reference form (`nix build .#aibox`)
for building.

## Common checks

```bash
nix flake show               # what the flake exposes (aibox, services1, services2)
git status --short           # untracked files are EXCLUDED from the flake!
nix flake metadata           # locked inputs / rev
```

## Gotchas

- **Never evaluate this repo through a path literal.** `import
  /home/leigh-admin/Projects/infrastructure-nix-flake/common/crane.nix` resolves
  the `../crates` inside it against the **real working tree**, and
  `lib.cleanSourceFilter` drops only VCS directories and `~`/`.o`/`.so`/`result*`
  names — it does **not** read `.gitignore` — so
  `cleanSourceWith { src = ../crates; }` copies `crates/target/` (**3.1 GB** of
  cargo artifacts) into the store while holding the big lock (next section). The
  flake path is unaffected: through `self` the same directory arrives already
  filtered to git-tracked files (the store copy is 132K), which is why nothing in a
  deploy hints at this and it only bites a throwaway `nix eval --file` that
  measures something. Read values out of the flake instead — `nix flake check
  --no-build`, `nixos-rebuild dry-build`, then
  `$SYS/etc/systemd/system/<unit>.service` — or, for a value with no flake output,
  `nix eval --impure --expr` against `builtins.getFlake` (flake sources are
  git-filtered; path literals are not).
- **`nix eval`/`nix build` on a bare path** (e.g. `nix build nixpkgs#foo`) from the
  repo root resolves against the **repo** flake, not nixpkgs; use `--file <(...)`
  with an explicit `import` for standalone nixpkgs lookups.
- **Untracked files are invisible to the flake** — `git add` new/changed `.nix`
  files first.
- **`nix flake check --no-build` is expected to be warning-free**, so a warning is
  a finding rather than noise. The four classes that were live until 2026-10-14:
  crane guessing `pname`/`version` from the workspace root (it is a *virtual*
  manifest — pass both to `buildDepsOnly`, see `common/crane.nix`), the renamed
  `nix.trustedUsers` (now `nix.settings.trusted-users`), and a unit `after`
  `network-online.target` without `wants`/`requires` it. It still ends with
  `error: path '...sops-secret-grafana_secret_key.drv' is not valid`, which
  predates all of that and is not yours to chase — **verify against clean HEAD
  (`git stash`) before hunting any error you think your change caused.**

## The nix big lock, and aborted tool calls

A nix client holds the global store lock (`/nix/var/nix/db/big-lock`, nix's
exclusive NixDB lock) for its **whole evaluation**, and **an aborted tool call does
not stop what it started**. Seen 2026-10-14: a `nix eval` whose tool call reported
`Command aborted` went on running for minutes holding that lock. Every nix command
after it queued behind the lock and did *nothing* — no CPU, no output, no timeout —
which is indistinguishable from "this eval is just slow", and it read as several
dead evals on top of the one that was actually stuck.

So: **after aborting a nix tool call, find the survivor and kill it before issuing
another nix command**; and when a nix command hangs, look for the lock holder
before believing it is evaluating. None of these run nix, so they work while it is
wedged:

```bash
pgrep -a -f "nix eval|nix build"     # survivors of aborted calls — kill these
ps -o pid,stat,etimes -p <pid>       # D = blocked in I/O, not computing
sudo ls -l /proc/<pid>/fd | grep big-lock   # the holder is a nix-daemon child
```

Killing the client takes the daemon's child, and the lock, with it.
`nix eval --raw --expr '"ok"'` returning instantly is the proof the store is free
again.

## Building in a detached unit (for builds that outlive the tool call)

`systemd-run` without `--no-block` waits for a oneshot to *finish*, so a harness
timeout interrupts the build. Start it detached and poll:

```bash
sudo systemd-run --no-block --unit=deploy-build --collect --service-type=oneshot \
  --property=DynamicUser=no --property=TimeoutStartSec=3600 \
  --setenv=PATH=/run/current-system/sw/bin --setenv=HOME=/root \
  /run/current-system/sw/bin/bash -c \
    'cd ~/Projects/infrastructure-nix-flake && nixos-rebuild switch --flake .#services1 \
       > /tmp/deploy.log 2>&1; echo SWITCH_EXIT=$? >> /tmp/deploy.log; \
       nixos-confirm >> /tmp/deploy.log 2>&1; echo CONFIRM_EXIT=$? >> /tmp/deploy.log; \
       readlink -f /run/current-system /nix/var/nix/profiles/system >> /tmp/deploy.log'
```

Poll `/tmp/deploy.log` and the unit state (`systemctl is-active deploy-build`,
`journalctl -u deploy-build`); do not sleep to the expected finish time. Chaining
`nixos-confirm` inside the unit keeps the GOLDEN RULE intact. `nixos-rebuild
switch` does link `/nix/var/nix/profiles/system`, so the boot target follows the
switch; that only needs the manual `nix-env --profile ... --set` fix when
activating a closure by hand with `switch-to-configuration switch` (see
`docs/nix-build-sigkill-2026-10-11.md`).

## Remote building (services2 is the builder)

`common/build-farm.nix` sets `nix.distributedBuilds = true`, and nix 2.34 then
*prefers* the external builder for every build, falling back to local only when no
builder is available. **`--max-jobs 0` does not enable delegation, it removes the
local fallback.** So a plain `nix build` / `just switch` on services1 builds on
services2 and copies the result back; `just build-remote` offloads deliberately.
Flip `nix.distributedBuilds = false` in `common/build-farm.nix` to go back to
local-only.

Verified working 2026-10-12 (delegated build and a `--max-jobs 0` build both
completed on services2 and copied back, with no SIGKILL sweep on either box) — the
fix that made that true is `common/nixbld.nix`, and the incident behind it is
`docs/nix-build-sigkill-2026-10-11.md`. Poll a delegated build rather than trusting
it: **a remote builder that silently falls back to local is one of the failure
modes only polling catches.**

## The `nixbld` rule, in one paragraph

`build-users-group` is not a permissions group, it is the **pool of UIDs nix runs
builds as**: nix takes a member for each build and, before and after that build,
runs `setuid(uid); kill(-1, SIGKILL)` (`killUser()`) to sweep the build's
leftovers. A real account in that group loses every process it owns to every build
on the machine. **Never put a real account in `nixbld`** — `common/nixbld.nix`
fails the eval if the config asks for it, and `nixbld-guard.timer` undoes an
imperative `usermod -aG nixbld` / `gpasswd -a` within fifteen minutes and writes
the reason to the journal (`journalctl -u nixbld-guard` is the first thing to read
if a session ever dies during a build). Full story, nix 2.34.7 source references
and evidence: `docs/nix-build-sigkill-2026-10-11.md`.
