# Nix dev workflow — commands, how to read the result, and the ways it goes wrong

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: run
`just check` before deploying, never evaluate this repo through a path literal,
`nix flake check --no-build` must stay warning-free, always read back the rendered
unit after editing `ExecStart`, never put a real account in `nixbld`, and kill the
survivor of an aborted nix tool call before issuing another nix command. This file
has the commands and the full gotchas.

The flake is pure and its sources come from the **git tree**, so everything below
runs from this repo root, and untracked files are invisible to it (`git add` new
`.nix` files first). **Which repo root is not settled either** — a session runs on
whichever box it was started on, so read `hostname` before anything that sshs. That
section is first for that reason.

## Which box am I on? — read `hostname` before anything sshs

A session is started on whichever box the operator was standing at. Every flake
machine has **the same repo checkout at the same path** (`~/Projects/infrastructure-nix-flake`)
and **the same `~/.ssh/agent-hop-key`**, so nothing about the prompt, the cwd, or the
git prompt says which box ran it. `hostname` is the first fact of any job that sshs or
deploys, not an assumption carried over from the last session.

```bash
hostname                       # gw | services1 | services2 | aibox | monster1 | …
git rev-parse --short HEAD     # which commit this box's copy of the repo is at
git status --porcelain | wc -l # and how dirty it is (rule 5's untracked-file trap)
readlink /run/current-system   # the running generation — differs per box, and is
                               # the thing the GOLDEN RULE confirms
```

**The trap: a hop to the box you are already on succeeds, silently.** The hop key is
authorized for `leigh-admin` on every flake machine *including the one you are standing on*,
so the ssh that was meant for another box lands back here, exits 0, and prints output that
looks like it came from over there. Measured on `gw` on 2026-10-28, as two halves of one
tool call:

```text
local:    host=gw gen=/nix/store/si3hsraw…-nixos-system-gw-26.11.20261014.319e4ce
              head=dcedf96 dirty=0
self-hop: hop-host=gw hop-gen=/nix/store/si3hsraw…-nixos-system-gw-26.11.20261014.319e4ce
              hop-dir=/home/leigh-admin/Projects/infrastructure-nix-flake
```

Same hostname, same store path, same repo directory. What changed is the *shell*: the
nested ssh carries none of the cwd, the `git log -1`, the `readlink /run/current-system`,
the dirty-tree state or the tty that the outer shell had just read, so every one of those
facts stops being what the transcript says it is — and `cd ~/Projects/infrastructure-nix-flake`
in the outer half is now a claim about a box the inner half is not on. **The GOLDEN RULE
cannot hold across two ssh layers** (rule 4: confirm in the *same* shell invocation as the
switch), which is what makes a self-hop turn a "deployed and confirmed" transcript into
nothing at all. Read `hostname` before issuing the hop; if it equals the target, run the
job locally.

Everything machine-shaped in this repo already keys to the local hostname, which is the
other half of why it matters — those commands answer *"did you type this here?"*, not
*"did you mean to build here?"*:

| What | Reads the hostname how |
| --- | --- |
| `just switch` (`nixos-rebuild switch --flake .`) | nixos-rebuild-ng's `Flake.parse` builds `nixosConfigurations."{attr or uname -n(target_host) or default}"`. With no `#<machine>` and no `--target-host`, `attr` is empty and `_get_hostname(None)` is `platform.node()`: **`--flake .` means the box you are standing on**, and `just switch` can only ever deploy *this* machine — a mistaken machine name is not what bites, a mistaken idea of where you are is. With `--target-host` it runs `uname -n` on the far side instead, so a push that named no `#<machine>` asked the *remote*'s hostname what to build — the same fact, read on the other side of the hop. |
| `just deploy <machine>` | `me=$(hostname)`; when it equals the machine it prints `=== <m> is this box: switching here` and runs the switch locally. **The recipe is correct typed on any box, self-hop included** — hand-written ssh chains are where the mistake lives. |
| `_build-host-guard`, hence `just check-build` / `just test` / `just clippy` / `just build-frontend` / the `just filestore-*` family / `just switch-gui-test` / `just build-remote` | `[ "$(hostname)" = "gw" ]` — a refusal is a question about where you typed, so "run it on aibox" is a *move*, not an override (rule 22 says why it is also not a workaround for the flake step of `just check`). |
| `just check` | skips its flake step on `gw` and prints why — the skip is a fact about this box, so quoting a green `just check` names the box it ran on. |

Two name collisions to keep apart while mapping a hostname to an address:

- `hostname` says **`gw`** where `common/hosts.nix` calls the same box **`router`** (the
  table names an address after its job). `just deploy gw` maps the two (`case "$machine" in
  gw) hkey=router ;; esac`) and `just known-hosts` reads the same table; a hand-written
  hop has to do the mapping by hand, which is what AGENTS.md's ssh block spells out as
  `leigh-admin@10.3.1.1`.
- **aibox maps its own name to `127.0.0.2` in `/etc/hosts`**, so `ssh aibox` from aibox is
  a self-hop behind a second indirection — use the IP in every hop.

Re-verify the rebuild half instead of inheriting this text: nixos-rebuild-ng has moved
once already (it is now `nixos-rebuild-ng-26.11`, a Python entry point rather than
`switch-to-configuration`'s bash), and the default-machine rule lives in its own source:

```bash
n=$(readlink -f "$(which nixos-rebuild)")
grep -n 'nixosConfigurations' "${n%/bin/*}"/lib/*/site-packages/nixos_rebuild/models.py
```

That prints the one line that decides the machine — `nixosConfigurations."{attr or
cls._get_hostname(target_host) or "default"}"` — plus `_get_hostname`'s two branches:
`uname -n` over `--target-host` when one was given, and `platform.node()` (this box's
hostname) when none was.

**The box also decides the rest of the session, not only the ssh:** `docs/network-upgrade-runbook.md`
begins by printing `hostname` on each box before touching it, rule 20 (`git log -1` on a
remote box before blaming a config change) only means anything because each box holds its
own checkout at its own commit, and `docs/router-ha.md` names the three things keyed to the
literal `gw` — a second router under that name would be indistinguishable from the first
by every one of them.

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

## Never evaluate to find out what a change does

The slow way, which looks like the careful way:

```bash
# minutes, and it is the thing that wedges the session
nix eval --impure --expr 'let f = builtins.getFlake "/home/leigh-admin/Projects/infrastructure-nix-flake"; in f.nixosConfigurations.services1.config.services.nginx.virtualHosts."x".locations."/".proxyPass'

# the same value, seconds (rule 3 below)
nix eval '.#nixosConfigurations.services1.config.services.nginx.virtualHosts."x".locations."/".proxyPass'
```

Why the first one is *minutes* rather than slower: `builtins.getFlake "/abs/path"`
is a **`path:` flake reference, and those are copied whole** — everything except
`.git`, with `.gitignore` **not** consulted (measured with a toy repo: a 20 MB
gitignored directory lands in the store, and *one changed byte inside it* is enough
to make the next call copy it again under a new name). Here that is the cargo and
dioxus `target/` trees plus `.git`, so the eval starts by copying **7.7 GB** into a
fresh store path — 7.2 GB of build trees and 516 MB of `.git` against the **26 MB**
of tracked source the flake actually reads, i.e. it ships 0.34% useful bytes.

Measured on services1 on 2026-10-18, same expression, four calls:

| call | wall | what it cost |
| --- | --- | --- |
| first ever (cold copy) | **108 s** | 7.7 GB written to the store |
| identical call, tree untouched | **303 s** | re-read and re-hashed all 7.7 GB into a temp dir, then threw it away: the daemon has no record of the temp copy, so **nothing is ever reused, not even with zero edits** |
| one nginx value out of the same flake | **331 s** | the copy again plus a cold evaluation of every machine |
| after `touch crates/target/junk` | **370 s** | a *second* 7.7 GB store path |

That last row is why the store fills up: cargo writes into `crates/target/`
constantly, so each call hashes to a new path and keeps its own copy. Six fat
`-source` paths from 2026-10-11/12 (2.9 GB each, the pre-`crates/` layout) and
three from 2026-10-18 (7.7 GB each) were still on this box, ~40 GB, kept alive by
old `.drv` files that name them — `nix store gc` is what reclaims those; `nix store
delete` only works on the unreferenced ones, and note the chicken-and-egg: a client
whose own argv/environ names the path keeps it alive (`/proc/<pid>/environ` is a GC
root), so build the path string inside the script rather than passing it in.

`nix flake metadata . --json | jq -r .path` shows what the *normal* commands use
instead: the same tree git-filtered, **26 MB**, in **1 s**. The git filter is what
makes the ignored build trees invisible, so they cost nothing here — and a dirty
tree costs the same as a clean one (measured: 1 s and 0 s), because the hash covers
tracked files only. Full set of sanctioned timings, same machine, same moment:
metadata 1 s, one value 1 s, `services1` toplevel `drvPath` 17 s, `gw` 7 s,
`aibox` 56 s (see [Evaluation must not build](#evaluation-must-not-build)), and the
`?dirty=1` deploy ref 1 s.

`builtins.getFlake` starts a **cold evaluation of the whole flake** — every
machine, every crate's source filter, every image pin — so it reuses none of the
eval and build cache the last deploy left behind. For a change confined to one
service module it is therefore *slower than deploying it*, and it answers a
question the running system can answer for free. It also has a second cost: an
aborted `nix eval` keeps running and holds `/nix/var/nix/db/big-lock`, so every
nix command after it — including `just switch` — queues behind it and looks
identical to "this eval is just slow" ([the big lock](#the-nix-big-lock-and-aborted-tool-calls)).

The rule, in order of preference:

1. **Apply it and read the file.** `just switch`, then
   `/etc/nginx/nginx.conf`, `/etc/systemd/system/<unit>.service`,
   `systemctl show -p …`, `curl localhost:<port>`. This is the only method that
   also proves NixOS's own normalisation, and on a warm machine it is usually
   faster than a cold eval of one attribute.
2. **Check without applying:** `nixos-rebuild dry-run`/`dry-build`, or
   `just check` / `just check-build`. Same evaluation the deploy performs, so it
   is cache-warm and its result is reusable by the deploy.
3. **One value, once:** `nix eval .#…` (the flake-reference form, not
   `--impure --expr`), under `timeout`, and kill the survivor before any other nix
   command. Never in a loop, and never to ask "did my setting land". If the
   expression really has to be `--impure`, `builtins.getFlake` **a store path**
   (`$(nix flake metadata . --json | jq -r .path)`, already copied, so instant),
   never the working directory.

The same trick deploys a dirty tree without the copy: `nixos-rebuild switch
--flake "git+file://$PWD?dirty=1#services1"` archives the **tracked** files as they
stand in the working tree, so it costs 6 s on services1 against the ~5 min the
`path:`-ref equivalent takes. `dirty=1` is what includes the uncommitted edits;
nix refuses the ref without it when the tree is dirty. Use it for a throwaway
check, `just switch` for anything you want to keep — the deploy itself is the same
either way.

## Evaluation must not build

**Resolved for the crate `-src` stubs on 2026-10-18 — reopened 2026-10-22 for the lock
file,** at the bottom of this section. Read the 2026-10-18 story first; the 2026-10-22
finding is the same mechanism through the other door, and the reason it was missed is the
warning that ends the old part.

`nix eval` was slow even in its *sanctioned* form, and not
only because of the copy above: evaluating a machine's configuration **compiled
things**. `nix flake check --no-build` — i.e. `just check` — died on

```
error: path 'wnjjz700yi95dbw9002ir5hm0hqmrhay-frigate-monitor-src.drv' is not valid
```

on a store that had never built that crate, and a cold eval of aibox's config
printed `building '…-frigate-monitor-src.drv' on 'ssh://leigh-admin@10.3.1.32'`.
An *evaluation* was paying a remote builder, per crate, and `--no-build` could not
stop it — the flag governs the command's goal, not what a forced store read does.

The mechanism is crane reading the build inputs **at eval time**:
`vendorCargoDeps` does `builtins.readFile cargoLock` and `findCargoFiles src`, both
harmless on a plain path and both a **realisation** when handed a derivation.
`common/crane.nix` hands it derivations by design — `lockFile` re-emits each
`Cargo.lock` as a content-keyed `writeText` so a comment in a `.rs` file cannot
invalidate the dependency cache, and `memberSrc`/`withSharedCrates` are
`runCommand`s. So every crate's `-src` stub and every `Cargo.lock` got built the
moment anything asked for a value out of a configuration that installs one.

The fix is in `common/crane.nix` (`vendorDirFor`): compute the vendor tree from the
*plain* source directory and the lock's contents, and hand `buildPackage` a
`cargoVendorDir`, which crane's `args.cargoVendorDir or (vendorCargoDeps args)`
short-circuits. `wasmSpa` therefore takes `cargoLockPath` rather than `cargoLock`.

Verified rather than argued, because "it should not change anything" is what a
rebuild costs:

- **No rebuilds.** `frigate-monitor`, `switch-gui` and `filestore` keep byte-identical
  store paths across the change (`54v84yci…`, `08hmnn4j…`, `2mf8baix…`), and so do
  `services1`, `services2` and `gw`'s toplevel `drvPath`s. Identical drvPath means an
  identical build recipe, so this is proof, not reassurance.
- **The realisation is gone.** Delete one `-src` output, then evaluate: with the fix
  `nix flake check --no-build` passes with zero `building` lines; with the fix
  stashed it rebuilds that crate's source tree on aibox and copies it back. That is
  the A/B, and it is repeatable — so `just check` is now a genuine guard: if a future
  change makes evaluation read a derivation again, `--no-build` is what says so.

Two things it is *not*: aibox's and pi-room-sys's toplevel `drvPath`s do move on
this change, but they move on **any** tracked edit — one HTML comment in
`docs/follow-ups.md` produced a third distinct pi-room-sys drvPath — which is a
separate, pre-existing cache-invalidation problem written up in
[`follow-ups.md`](follow-ups.md). And `nix flake check --no-build` passing is not
evidence the eval is clean: once a `-src` output exists, nothing looks different.
Delete one, or evaluate a tree that has never been built, before believing it.

Observed 2026-10-14 on services1: two `nix eval --impure --expr` calls to read one
nginx location's `proxyPass` — the first aborted and left a lock-holding survivor,
the second slower than the `just switch` that finally answered it, after which
`grep proxy_pass /etc/nginx/nginx.conf` was instant. The rendered config is the
source of truth; the expression is a guess about it.

### Reopened 2026-10-22: the lock file still gets read, and the router pays for it

`just check` on gw started failing with

```
error: Cannot build '/nix/store/7ac7180r3lg3rgk31ry47r88s4c8djpx-Cargo.lock.drv'.
Reason: local builds are disabled (max-jobs = 0)
```

**It is not a regression.** The same call fails at `f923c6e` — the commit this repo's
audit-pass branched from — run from a `git worktree`, so nothing in the restructure
caused it. What changed is the store.

The mechanism is the one above, through the door `vendorDirFor` did not close: `lockFile`
is still handed to crane as `cargoLock`, **without** the `cargoLockContents` that makes
the read free, at three sites — `workspaceArtifacts` (`buildDepsOnly`), the native crate
arguments, and `wasmSpa`'s `buildPackage`. The trace lands on
`machines/services1/services/filestore.nix:82`, `ExecStart = lib.concatStringsSep " " [
"${filestore}/bin/filestore" …` — a module that interpolates its own package into a unit
forces that package's derivation whenever anything evaluates the system, and
`nix flake check` evaluates the system.

Why it stayed hidden: `…-Cargo.lock` **was in gw's store**, so the read cost nothing and
`--no-build` had nothing to complain about. It is gone now — most plausibly a `nix store
gc` of a path no root ever pointed at — and it cannot be restored: `nix copy --from
ssh://leigh-admin@10.3.1.32` answers `cannot add path … because it lacks a signature by a
trusted key`. It is an unsigned locally-built path with no cache, and gw does not build,
by design. So a read that had been silently free for four years turned into a hard
failure with no commit in between. This is exactly the warning at the top of this
section, demonstrated rather than believed: **a passing `--no-build` is not evidence the
eval is clean.**

What was tried, and why it is not in the tree. Adding `cargoLockContents =
builtins.readFile <lock>` at those three sites — the pairing `vendorDirFor` already uses,
so the value is the one crane computes — makes the `Cargo.lock` error go away and uncover
a **second** eval-time read: `error: path
'xc2p67qsy0500f2z38krv1kkq7lwjfin-filestore-web-src.drv' is not valid`, i.e. the SPA's
`withSharedCrates` source. `crateName = pname` at both `buildPackage` sites (the value
crane's discovery returns — `pname` is the manifest's package name at every call site)
did not remove it, so some other crane scan of `src` is the reader and it has not been
identified. Both experiments were reverted: the pairing is very likely correct, but it
cannot be proved drv-neutral from a box that cannot build, and being wrong about the
shared dependency wiring costs every crate on the fleet a rebuild. Whoever finishes it
does it on aibox, with the before/after `drvPath` comparison this section has always
insisted on, and the *second* read is the real work.

**What still works on the router, and what does not.** The assertion evals
(`nix eval --impure --raw -f <file>`) and `just fmt` / `just fmt-check` do not evaluate the
machines, so they run on gw exactly as before; together they are `just check-eval`, which is
the router's gate. `nix flake check --no-build` — and so the flake half of `just check` — does
evaluate them, and is skipped on gw with the reason printed. Treat that skip as a workaround
for an open bug, not as a licence to skip the check: rule 22's reason for refusing builds on
gw is untouched, and the skip is rule 22 *working*. See the next section for the decision and
for why "run it on aibox" is a warm store rather than an answer.

### The hop itself was broken, separately — and it is not the operator's ssh

Same day, a different symptom: every delegated build from gw died seconds in with

```
cannot build on 'ssh://leigh-admin@10.3.1.32': error: failed to start SSH connection
to '10.3.1.32': Host key verification failed.
Failed to find a machine for remote build!
```

while `ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes -o ConnectTimeout=5
leigh-admin@10.3.1.32` from the same box answered `aibox` and returned 0. The two
commands are not the same ssh. **nix runs the hop as the daemon — uid 0, `HOME=/root`
— and that root has no `known_hosts` on gw**, so ssh has no record of `10.3.1.32`,
nix's ssh has no tty to be asked on, and it refuses. Read on gw, 2026-10-22:

- `sudo ls -la /root/.ssh/` → `.` (4096) and **`agent`** (the systemd-ssh-proxy socket
dir) and nothing else: no `known_hosts`, no key. The operator's
`~leigh-admin/.ssh/known_hosts` is what the manual hop reads.
- `sudo cat /etc/static/ssh/ssh_known_hosts` (the target of the
`/etc/ssh/ssh_known_hosts` symlink, i.e. NixOS's system-wide file, the one every
ssh — root's included — gets to through the `GlobalKnownHostsFile` line NixOS writes
in `/etc/ssh/ssh_config`) → **one blank line**. Nothing in the flake declared a
host key, so the file the daemon could have used was empty.
- the failure is reproducible without nix in one line: `sudo env -i HOME=/root … ssh -i
/home/leigh-admin/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.32 true` →
`Host key verification failed`, and adding a `known_hosts` that names that address
makes it return 0.

Why services1 and services2 never showed this: their **root** has a hand-made
`/root/.ssh/known_hosts` (services1's has 15 lines — `10.3.1.21`, `10.3.1.32`,
`10.3.1.36`, the cams, `[nas2.int.leighhack.org]:3022`), so the hop works *there*
because someone ssh'ed from a root shell at some point. That is the same shape of
debt as every other mutable file no deploy can reproduce: the box that works is the
box somebody visited, and nothing in the tree says so.

The fix is in the tree: `common/ssh-known-hosts.nix` (DATA — the four flake machines,
rsa + ed25519 each, addresses from `common/hosts.nix`) handed to
`programs.ssh.knownHosts` from `common/build-farm.nix`, so it lands in
`/etc/ssh/ssh_known_hosts` on every box that imports `commonModules`, and the
daemon's ssh is the pin whether or not its root ever visited anything. `just
known-hosts` prints the lines to paste in, for the day a box is reinstalled or
rotates its keys — pinning rather than `accept-new` is deliberate, and is the same
call `docs/repo-audit-2026-10-07.md` §3.4 made for the router's keys.

Two things worth not rediscovering:

- **`-o KnownHostsFile=…` does not exist on this fleet's ssh.** OpenSSH 10.5p1
(`ssh -V`) removed the keyword from the command line: `ssh -G 10.3.1.1 -o
KnownHostsFile=/tmp/kh` is a hard error — `command-line: line 0: Bad configuration
option: knownhostsfile` — so there is no way to point one ssh at another file, and
the generated config file is the only lever. `strings` on the binary shows why:
the surviving keyword is `KnownHostsCommand`.
- gw was left working *before* the deploy: on 2026-10-22 its `/root/.ssh/known_hosts`
was written with exactly the eight pinned lines, so the hop is live on a box whose
checkout predates the flake change (rule 20: check `git log -1` before blaming a
config change for a box's behaviour — this is the reverse of that, and it will be
silently superseded by the next switch of any box, which writes the store version).

With the hop fixed, gw builds elsewhere: `nix build --max-jobs 0 --expr 'derivation {
name = "gw-hop-test"; … }'` printed `building '…gw-hop-test.drv' on
'ssh://leigh-admin@10.3.1.32'… copying 1 paths…` and `cat` of the output path in **gw's**
store returned `hi`. It also changes where `just check` on gw dies: the `Cargo.lock`
read above now *resolves* (the daemon can build that drv on aibox and copy it back),
so the call fails one read later, at `error: path
'xc2p67qsy0500f2z38krv1kkq7lwjfin-filestore-web-src.drv' is not valid` — the
**same** store path the section above predicts once the `cargoLock`/`cargoLockContents`
pairing is in. Two independent passes agreeing on one hash is corroboration that the
pairing is the right next step and that the SPA source read is the real work.

**Observed on the second pass, 2026-10-22, after the new files were `git add`ed**: the
untracked file cost its own error first — `nix flake check` died at
`Path 'common/ssh-known-hosts.nix' in the repository "…" is not tracked by Git`, with
the trace going *through* `nixos/modules/programs/ssh.nix` evaluating
`programs.ssh.knownHosts` — which is the proof the option name and its
`attrsOf (submodule …)` shape are accepted in the real system context, and the
module is in nixpkgs' module list unconditionally (`grep -n programs/ssh
<nixpkgs>/nixos/modules/module-list.nix` → line 337; the module has no
`programs.ssh.enable` gate, so `/etc/ssh/ssh_known_hosts` is written on every
system — that blank line is the module, running). Then, tracked, the call died at
the filestore read above. Both halves of this section are now measurements, not
predictions.

### Closed 2026-10-25, one read at a time: the pairing shipped, the source read is a decision

The `cargoLock`/`cargoLockContents` pairing landed on 2026-10-25 in `common/crane.nix` as
**`lockArgs`**, applied to the three places that hand crane a lock file (`crate`,
`wasmSpa`, `workspaceArtifacts`), and `just check` on gw died **one read later** — at
`…-filestore-web-src.drv`, the store path this section predicted before anybody ran it.
So the lock is no longer read; what remains is the second of the two reads, and it is not
a pairing.

It had landed *partly* before that: `vendorDirFor` (2026-10-22, from the work above)
pairs internally and `crate`'s member branch already stated a vendor dir, which is why the
read survived at all — `workspaceArtifacts` had neither half, and a crate handed `cargoLock`
bare is read by crane whichever way round it is stated, because `vendorCargoDeps` does a
`pathExists`/`readFile` on it. Hence the rule `lockArgs` encodes: **a crate may state both
halves or neither**, and `cargoVendorDir` is what lets the contents be stated without the
lock being read back. The value is the one crane would have computed — no crate keeps a
`.cargo/config.toml`, so `cargoConfigs` is empty either way — which is why this is
drv-neutral: the toplevel drvPaths of the two machines that install **no** SPA, `services2`
and `gw`, are **bit-identical** across the change, measured on services1 with a warm store
(`yqs5l3n6…`, `hjnirr2d2…`, before and after). `services1` and `aibox` do move, and only
because the builder text of the four hand-written SPA source derivations moved into
`withSharedCrates` — same files, same copies, different whitespace, so a different hash. Prove it for
yourself on a box that has never seen the source:

```bash
# the honest detector: --no-build alone still lets an evaluation *realise* a source path,
# and a warm store hides the bug entirely, so take away its ability to build first
nix flake check --no-build --option builders '' --option max-jobs 0
```

**What the remaining read is.** `craneLib.mkDummySrc` — which every crate's dependency
build goes through — walks `src` with `builtins.readDir` *while evaluating*, to find the
manifests to stub; its own comment calls this "laundering the data at evaluation time".
Any `src` that is a derivation is therefore **realised by an evaluation**, and four SPA
sources are derivations because a SPA's manifest reads path dependencies that live outside
its directory (`common-rs/preview` for filestore, a sibling `dto` for switch-gui and
gocardless-dashboard) and so have to be copied around it. Two ways out were tried:

- **An attrset source.** `builtins.readDir` reads an attrset as a file tree, and an attrset
  is realised by the store rather than by a builder, so it is the same bytes with nothing
  to build. It does not work: `src` is stringified into the build environment, so stdenv
  answers `cannot coerce a set to a string: { "Cargo.lock" = «thunk»; … }`. Measured, so
  the next person does not spend an hour on it.
- **Copy in a build phase** (`postPatch`) instead of assembling a source tree. Not landed,
  for a reason worth writing down rather than a result: `buildDepsOnly` runs cargo on the
  same arguments as the crate build, and cargo cannot resolve a path dependency that is
  not in the tree it is pointed at — so the path dependencies would move *into* the
  dependency derivation, which changes what invalidates what. That is a decision about the
  shared wiring, not a simplification, and `common/crane.nix`'s comment on
  `withSharedCrates` says so where the code is.

The probe that finds it in a tree you inherited: set the SPA's crate list to `[]` and
re-run the detector. Plain directory, nothing to add, detector passes — which is how this
was localised on 2026-10-25 without reading crane first.

**And it is not router-specific, which the repo had wrong.** The read is keyed to the *flake
source*, not to the box: in a fresh `git worktree` on aibox — the network's builder — the
detector failed with the same `jfv7x3pz…-filestore-web-src.drv` gw reports, and `--no-build`
refuses the stub whether or not aibox would build it in seconds. One
`nix eval .#nixosConfigurations.services1.config.system.build.toplevel.drvPath` in that same
worktree, and `nix flake check --no-build` printed `all checks passed!`. So every sentence in
this repo that said "broken on gw, run it on aibox" was describing a warm store, and the check
has been reading as intermittent for that same reason.

### The decision taken 2026-10-25: name the gate, do not buy the wiring

`just check` now splits. `check-eval` is the assertion evals plus `fmt-check` — nothing in it
evaluates a machine, so it is honest about being safe on the router and it is what `gw` runs.
`check` depends on it and adds `nix flake check --no-build`, and on `gw` that step is **skipped
with the reason printed** instead of dying halfway through the gate. The router therefore has an
explicit gate rather than a broken one, and no dependency wiring changed to get it.

What was declined, and why it is a decision rather than an oversight:

- **Accept the read** (a `check-eval`/`check` split alone): landed, above. Costs the router a
  flake-level gate, which the 2026-10-22 audit had already concluded it does not have.
- **Copy the path dependencies in a build phase**: kills the read, but moves `common-rs/preview`
  and the two `dto` crates into the SPAs' *dependency* derivations, so editing a shared crate
  can invalidate the cached dep tree of every SPA that reads it. Probably the honest graph; a
  change to what invalidates what on the four apps the build farm exists for, so it has to be
  measured (drvPaths and a timed one-line edit in `common-rs/preview`), not reasoned in.
- **Track per-app copies in git**: deletes the symlink machinery and writes rule 2's "never a
  second copy" into four manifests — and is not even available for `filestore`, whose
  `common-rs/preview` is shared with non-SPA crates, which is the whole reason `common-rs/`
  exists.

**The ordering argument is what settles it.** Behind this door is another of the same class:
`common/nginx/front-door.nix:80` and `common/nginx-sso-config.nix:27` do
`builtins.readFile (config.sopsSecretText name)`, which builds a *decrypted secret* during
evaluation. Fix only the SPA source read and `just check` still cannot run on `gw`, so the
class fix has to be one deliberate change covering both doors — verified with the detector
above — rather than half of it as a side effect of a cleanup. Until then, `check-eval` is the
gate, and `check`'s skip message says so on the box that has to hear it.

**And when a read of that class fails, never answer it by evaluating the option.** The
failure looks like `error: path '<hash>-sops-secret-pg_pass.drv' is not valid` and invites
`nix eval .#nixosConfigurations.<box>.config.services.<app>.databaseUrl` as the probe —
which does two damaging things at once. It realises the derivation (a plaintext secret into
the store, and on a box with `--no-build` it *builds*, which is the very thing this section
is about), and it prints the resolved secret to the terminal, hence to whatever log the
session keeps. Hit on 2026-10-28 isolating exactly that `--no-build` error: the eval printed
an application's plaintext Postgres password into a transcript, which is rotation work and
noted in `docs/follow-ups.md`. The safe probe is the same one this file recommends for every
other question — the *rendered* file from a built system, whose secret you then read by path
(`$SYS/etc/…`) rather than into a terminal — and the safe probe for "is this failure mine"
is `git diff HEAD --stat` plus a stash of the one file you suspect, not an eval of the option
that holds the credential.

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

**`just check-build` cannot finish on aibox, and it is the sandbox, not your change.** The
recipe builds all four toplevels, services1's first, and services1's
`door-entry-management-system` carries `__noChroot` (its `deno i` wants the network) — that
package is the whole reason `sandbox = "relaxed"` exists on services1 and nowhere else
(`AGENTS.md`, "Remote building is on"), so on aibox, which keeps `sandbox = true`, the build
is refused: `derivation '…-door-entry-management-system-…drv' has '__noChroot' set, but
that's not allowed when 'sandbox' is 'true'`. Seen 2026-10-28 after this box's store had been
garbage-collected down 118 GB, which is when it starts to bite: with a warm store the package
*substitutes* and `__noChroot` is never exercised, so the failure looks new and is not. It
also bounds what a run here is worth: every machine is still **evaluated** before anything
builds, so services1's and gw's module assertions (`machines/services1/int-dns.nix`,
`machines/gw/int-dns.nix`, the network-tables guard) really are checked, and only the
compiles behind that one derivation are not. For a compile gate use `nixos-rebuild build
--flake .#<machine>` for the machine you changed plus `nixos-rebuild dry-build --flake .#gw`
for the router, and run `just check-build` on a box that can build what it is asked to.

## Gotchas

- **`nix hash path` is not the hash of a file.** It prints the hash of the *path*
  (the NAR), and for a single regular file that is a different number from the
  file's own digest: `sha256sum f` and `nix hash file f` agree, `nix hash path f`
  agrees with neither. `fetchurl`/`fetchTarball` verify the **file** digest, so a
  hash written down with `nix hash path` fails as
  `hash mismatch in fixed-output derivation: specified: … got: …` — and "got" is the
  correct one, which reads exactly like a corrupted download, a stale CDN edge or a
  broken TLS middlebox. Measured 2026-10-25 on a `raw.githubusercontent.com`
  fetch: three bytes-identical downloads, two digests, and an hour spent on the
  network before the digests were compared with `sha256sum`. Write the pin with
  `nix hash file`, or break it on purpose and copy the `specified:` value out of
  the error; `nix-prefetch-url` reports the same thing `fetchurl` wants.
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
    'cd /home/leigh-admin/Projects/infrastructure-nix-flake && nixos-rebuild switch --flake .#services1 \
       > /tmp/deploy.log 2>&1; rc=$?; echo SWITCH_EXIT=$rc >> /tmp/deploy.log; \
       if [ "$rc" -eq 0 ]; then nixos-confirm >> /tmp/deploy.log 2>&1; echo CONFIRM_EXIT=$? >> /tmp/deploy.log; \
       else echo CONFIRM_SKIPPED=switch_failed >> /tmp/deploy.log; fi; \
       readlink -f /run/current-system /nix/var/nix/profiles/system >> /tmp/deploy.log'
```

Five traps in that block — the first three hit for real on 2026-10-15 (first run of
`docs/network-upgrade-runbook.md`), the last two on 2026-10-21 while running a
`nix build` loop detached the same way. All of them are cheap to fix in a script file
rather than inline — write `/tmp/nu-switch.sh` and `install -m 755 /dev/stdin`
it onto the box:

- **`--setenv=HOME=/root` means `cd ~/Projects/…` is `/root/Projects/…`.** Use the
  absolute repo path.
- **Confirm only if the switch exited 0.** Running `nixos-confirm` unconditionally
  marks the generation *currently in `/run/current-system`* good even when
  activation aborted halfway — and activation flips `/run/current-system` before it
  finishes, so a switch that dies at the end (aibox: a bind mount of a store path
  that moved, exit 4) leaves a half-activated system marked good and the
  auto-rollback timer disarmed. `SWITCH_EXIT!=0` means "read the log", never
  "confirm".
- **Root must trust the checkout**: `sudo git config --global --get-all
  safe.directory` must list the repo, or nix refuses it with libgit2's "not owned
  by current user". services1/services2 have it, aibox did not
  (`docs/follow-ups.md` has the item to declare it).
- **The script's shebang has to be a path that exists**: `#!/bin/bash` is not a thing
  on NixOS, and the unit dies in a heartbeat with `bad interpreter: No such file or
  directory` (status 126) before writing a line of log — which looks like the detached
  pattern not working. Use `#!/run/current-system/sw/bin/bash`, or no shebang at all and
  invoke it as `bash /tmp/the-script`.
- **Bash reads `$m_EXIT` as one variable name.** A loop that logs `BUILD_$m_EXIT=$rc`
  prints `BUILD_=0` for every machine, so the log says "a machine passed" rather than
  which one. Write `${m}_EXIT` in any marker built from a loop variable.

Poll `/tmp/deploy.log` and the unit state (`systemctl is-active deploy-build`,
`journalctl -u deploy-build`); do not sleep to the expected finish time. Chaining
`nixos-confirm` inside the unit keeps the GOLDEN RULE intact — the unit *is* the
shell invocation. `nixos-rebuild switch` does link `/nix/var/nix/profiles/system`,
so the boot target follows the switch; that only needs the manual `nix-env
--profile ... --set` fix when activating a closure by hand with
`switch-to-configuration switch` (see `docs/nix-build-sigkill-2026-10-11.md`).

## Remote building (aibox is the builder)

`common/build-farm.nix` sets `nix.distributedBuilds = true`, and nix 2.34 then
*prefers* the external builder for every build, falling back to local only when no
builder is available. **`--max-jobs 0` does not enable delegation, it removes the
local fallback.** So a plain `nix build` / `just switch` on services1 builds on
aibox and copies the result back; `just build-remote` offloads deliberately. The
builder used to be services2; it moved to aibox on 2026-10-17, when strata was
parked there to give the builds its 59 GiB of RAM (the 6-core `maxJobs` in
`machines/services1/default.nix` is aibox's, not services2's 8).
Flip `nix.distributedBuilds = false` in `common/build-farm.nix` to go back to
local-only. **Before believing a hop is configured, believe the host keys**: the
hop's ssh is the daemon's, not the operator's, and it fails with `Host key
verification failed` long before the build starts —
`common/ssh-known-hosts.nix` and `just known-hosts` are the pin and its
regeneration, and the receipts for why an empty `/etc/ssh/ssh_known_hosts` breaks
`nix build` while `ssh -i ~/.ssh/agent-hop-key …` works are below, at "The hop
itself was broken, separately".

**The delegating half that is easy to miss: the builder substitutes its own inputs.**
`common/build-farm.nix` sets `nix.settings.builders-use-substitutes = true` wherever it
sets a `builderHost`, and until 2026-10-25 it did not — so from the day remote building went
on, every input of every delegated build was substituted by *the client* and then uploaded
to aibox.  That is nix's default (`false`), not a decision anybody made here, and it has the
shape of "the build farm is slower than it should be": a nixpkgs bump is hundreds of paths
that are already on cache.nixos.org, and with the setting off services1 downloads all of
them and ships them over the hop before aibox can start, with aibox's job slots idle.  With
it on, `nix.conf(5)`'s own words are that remote hosts "fetch as many dependencies as
possible from their own substituters (e.g, from `cache.nixos.org`) instead of waiting for the
local machine to upload them all".  Nothing aibox cannot substitute changes: that is what
the ssh-ng builder asks the client for.  To see the difference, delegate a build of
something with a large input closure and read aibox's journal for `substituting '…'` rather
than a stream of `copying path` from here.  It is a **client-side** setting, so a box that
adds a builder gets it from the module and a box that serves builds needs nothing.

Verified working 2026-10-12 (delegated build and a `--max-jobs 0` build both
completed on services2 and copied back, with no SIGKILL sweep on either box) — the
fix that made that true is `common/nixbld.nix`, and the incident behind it is
`docs/nix-build-sigkill-2026-10-11.md`. The hop itself is box-agnostic (the same
key, user and `nix.settings.trusted-users` entry on both ends); re-verify it after
the move with one delegated build rather than inheriting that date. Poll a
delegated build rather than trusting it: **a remote builder that silently falls
back to local is one of the failure modes only polling catches.**

## Never build on the router

gw is a router, not a builder: 4 cores, 8 GB, and it answers DNS and DHCP for every
VLAN. Since 2026-10-21 it is also a **client** of the network's builder
(`infra.buildFarm.builderHost = HOSTS.aibox` in `machines/gw/default.nix`, so
`common/build-farm.nix` writes `/etc/nix/machines` here and turns on
`nix.distributedBuilds`) and it runs its own daemon with **`nix.settings.max-jobs =
0`** (not the deprecated `nix.maxJobs` alias — the setting is load-bearing, so it must
not become a hard error the day nixpkgs deletes the alias). That
second half is what makes the rule mechanical: remote building *prefers* the builder
and falls back to local in silence, and `max-jobs = 0` removes the fallback — a build
aibox cannot take fails instead of compiling on the gateway. Check both with `nix
config show | grep max-jobs` and `cat /etc/nix/machines`. The GC half of that module is
deliberately off on this box (`infra.buildFarm.gc = false`).

One thing to know when a renamed option complains: the warning names the file NixOS
thinks defined it — `The option \`nix.maxJobs\` … defined in
\`/nix/store/…-source/flake.nix\`` — and **`flake.nix` is where the module list lives,
not where the option is set.** The real definition is whichever machine module the
evaluation was for (`machines/gw/default.nix`, in this case). `/nix/store/…-source/`
**is** this repo, at the commit you ran, so the tail of the path is the file to edit and
the head of it means nothing; chasing `flake.nix` costs a read of 400 lines that do not
mention the option.

So the accident is gone and the rule is not: **evaluation is still paid on the router**,
and the deliberate deploy path is still build-there-and-push. Keeping "evaluates" and
"builds" apart still matters, because `just check` here is eval-only on purpose.

**`nix develop` used to be a build.** The one devShell `flake.nix` had depended on
`CRANE.wasmBindgenCli`, the pinned wasm-bindgen CLI, which is compiled from source with
cargo, so entering the shell on a box that had never built an SPA compiled Rust there. On
**2026-10-21** a `just check` on gw did exactly that: `check` ended with `just fmt-check`,
`fmt-check` ran `nix develop`, and four `rustc` processes appeared on the gateway. Nothing
was wrong with the checks — the formatter was reaching for a Rust toolchain to run a
Haskell binary.

Two fixes, in the justfile and in `flake.nix`. The formatter moved to `nix fmt`, and the
toolchain was split: `devShells.default` is cargo/rustc/clippy/lld/just/git/jq/alejandra/
node and **builds nothing** (all of it is substituted, including the Rust toolchain), and
`devShells.wasm` adds the compiled `wasm-bindgen-cli` on top — which is why the SPA
recipes ask for `nix develop .#wasm` by name rather than inheriting it by accident.

The split is now explicit:

| Safe on gw (eval/format only) | Refused on gw by `_build-host-guard` |
| --- | --- |
| `just check` (`nix flake check --no-build`, `nix eval`, `nix fmt -- --check`) | `just test`, `just clippy` (cargo, over the workspace) |
| `just fmt` (`nix fmt`, formatter from the flake's pinned nixpkgs) | `just check-build` (builds all four toplevels) |
| reading rendered files, `nixos-rebuild dry-run`, `just switch` / `just deploy <box>` (the box evaluates and activates itself, aibox compiles) | `just build-frontend`, `just filestore-dev`, `just filestore-test`, `just switch-gui-test` |
| | `just build-remote` (no local fallback by design) |

**Deploying a box is not on the refusal list, and that is the shape of the fleet now:**
`just switch` deploys the machine it runs on, `just deploy <machine>` sshs there and does the
same thing, and in both cases the evaluation is on the target while the compilation is
aibox's. That is what replaced the old `just switch-gw`-from-services1 push path, which made
one box the deploy host for another (see `machines/gw/README.md`, "How this box is
deployed").

`just fmt`/`fmt-check` used to run `nix develop` for alejandra. They now use
`nix fmt`, which takes the formatter from the flake's `formatter` output — a
substitutable nixpkgs package, so a *format* check can never be the reason a box
compiles anything. `just check` also now passes `--no-build` to `nix flake check`
(plain `flake check` builds the `checks` output, if there is one).

Build on **aibox** (the network's builder) or services1. The guard is a belt and
braces, not a substitute for knowing which recipe you are running: it only checks
`hostname = gw`, and `INFRA_ALLOW_BUILD_ON_ROUTER=1` deliberately overrides it. If
rustc ever appears on the router again, `pgrep -a rustc` then `ps -eo pid,args | grep
-E 'cargo|wasm-bindgen'` names the derivation; kill the build, and check whether the
client was `nix develop` (this rule) or a plain `nix build` — the second means either a
trusted client passed `--option max-jobs N` (deliberate) or the delegation has been
un-configured again: `nix config show | grep max-jobs` should say 0 and
`/etc/nix/machines` should name 10.3.1.32.


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
