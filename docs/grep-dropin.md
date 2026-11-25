# The grep the agents get: one symlink named `grep`, one gate that refuses a lie

**Written 2026-11-23, on services1.** The ask was "have a multicore, grep-compatible
replacement aliased as `grep` system wide, so the agents use it without realising". Two
of those words decided the shape, and one of them was wrong in a way that costs a day.

| You are about to… | Read |
| --- | --- |
| change the drop-in, or ask why `grep --version` on this box answers `ugrep` | §The one fact that makes it work, §What it replaces and what it cannot |
| ask whether the swap changed a number a recipe prints | `just grep-compat`, then §The differences, whitelisted by class |
| ask how fast it actually is, and where the "multicore" part sits | §The speed, and where it is not |
| deploy it on the other four boxes | §Not proven here |

## "Aliased" is not the mechanism — measured

An alias cannot reach what the agents run. Measured, on this box:

```
$ bash -c 'shopt -s expand_aliases; alias grep=echo; bash -c "grep hi"'
(nothing — the alias does not reach the child non-interactive shell)
```

pi's tool calls are `bash -c`, `sh -c`, `env`, `ssh '…'`: none of them read
`/etc/bashrc`, `/etc/profile.local`, or `~/.bashrc`. **PATH is inherited through all of
them, an alias is not**, so the mechanism is a file on PATH named `grep`, not an alias,
and the deploy that makes it true is the *closure*, not a dotfile.

The other half of why: on this pinned nixpkgs every PATH directory ahead of
`/run/current-system/sw/bin` is **missing** (measured, per entry):

```
1  ~/.local/share/pi-node/current/bin    writable, pi owns it — the flake cannot
2  ~/.pi/agent/bin                       writable, pi owns it — the flake cannot
10 /run/wrappers/bin                      read-only, and `security.wrappers` = a setuid-root grep
11 ~/.nix-profile/bin                     MISSING
12 /nix/profile/bin                       MISSING
13 ~/.local/state/nix/profile/bin         MISSING
14 /etc/profiles/per-user/leigh-admin/bin MISSING
15 /nix/var/nix/profiles/default/bin      MISSING
16 /run/current-system/sw/bin             read-only, holds grep
```

The empty profile dirs are rule 32's fact seen from the other side — this pinned
nixpkgs writes no home files, so the per-user profile mechanism that would otherwise
be the shadow target is simply not there. Nothing earlier than 16 can be filled
declaratively, so the drop-in **replaces** what is in 16.

`/run/wrappers/bin` is on PATH at 10 and the flake *can* write it through
`security.wrappers`, and that is a trap rather than an option: the wrapper is setuid
root, and ugrep reads files and prints them, so a setuid-root `grep` is a general
facility for reading root-owned files (`grep x /etc/shadow`) by every account. Not
shipped, and not to be reached for.

## The one fact that makes it work

**ugrep keys its grep-compatibility mode to `argv[0]`**, so the name is load-bearing and
no flag is needed. Measured, from a directory holding a symlink `grep` → ugrep:

```
$ grep --version | head -1
ugrep 7.8.5 (grep compat) x86_64-pc-linux-gnu +avx2; -P:pcre2jit; -z:zlib,bzip2,lzma,lz4,zstd,brotli,7z,tar/pax/cpio/zip
$ grep --stats -r PARKED machines/ | tail -5      # the options compat mode turned on
  --basic-regexp
  --directories=recurse
  --hidden
  --sort=name
```

That is what keeps GNU's **BRE default** (`grep 'a+'` is a literal `a+`, not a
quantifier — measured identical to gnugrep on `a+ here` / `plain a plus` / `aaa+`),
what makes it search hidden files like grep does, and what makes it **not** read
`.gitignore` like ripgrep does (measured: a match inside a `build/`-ignored directory
is found by both engines).

`egrep`/`fgrep` names take their own mode, so the trio would behave like GNU's trio;
only `grep` is shipped today.

Rejected, with the measured reason:

* **ripgrep** is not a drop-in: `rg -r` is **replace**, not recursive —
  `rg -r foo 'sigma omega' many2` rewrote the matched text to `foo` in the output. It
  has no BRE, and it honours `.gitignore`, so it silently skips `crates/target/` and
  anything a person's own ignore file hides.
* **hypergrep** is not in this nixpkgs (searched), needs vcpkg + Intel Hyperscan +
  libgit2, and its `-h` means `--help`, not `--no-filename`.

## What it is, in the flake

`common/tools.nix` (MODULE) — in the `# System Tools` group, so it lands on every
machine that imports the module:

```nix
grepCompat = pkgs.runCommand "grep-as-ugrep" { } ''
  mkdir -p $out/bin
  ln -s ${pkgs.ugrep}/bin/ugrep $out/bin/grep
'';
```

plus `grepCompat` and `pkgs.ugrep` in `environment.systemPackages` — `ugrep` and `ug`
under their own names too, so a person typing `ug` still gets the interactive flavour
and the gate can always find a standalone engine to compare against.

## What it replaces, and what it cannot

The swap is **PATH-visible**, not everywhere-visible, and that is a fact and not a
complaint. Things that name grep by store path keep GNU grep, unchanged:

```
machines/gw/uplink.nix            ${pkgs.gnugrep}/bin/grep -q 'inet '        (ExecStart)
machines/services1/services/unifi-os-server.nix  ${pkgs.gnugrep}/bin/grep -q '^system_ip='
machines/pi-room-sys/netboot.nix  ${pkgs.gnugrep}/bin/grep -q "root::" /etc/shadow  (activation)
common/nixbld.nix, machines/gw/network-status.nix, machines/gw/uplink.nix
                                  pkgs.gnugrep in a unit's `path = [ … ]` list
```

A unit's `path =` list puts *that package's* store `bin` on the unit's PATH, so the
probe inside `network-status.service` and the `nixbld-guard` timer keep GNU grep even
after the switch. What changes is everything that types `grep`: the agents, every
`just` recipe, and the guards the flake runs in shells. A unit that needs the new one
adds `grepCompat` to its own `path =` list.

## The differences, whitelisted by class

`just grep-compat` runs every invocation this repo actually makes under both engines
and refuses unless the differences are exactly these classes, with these counts.
Nine cases agree byte-for-byte and on exit status, including the numbers the recipes
print: `-A4 '^  cam5 = {'` → the address, chained through `-oE '10\.3\.[0-9]+\.[0-9]+'`
(`cam-check`, `cam-set` and the ten other copies in `just/cameras.just`), `grep -c ""`
and `grep -c "^mtd"` (`cam-fw-verify`'s comparison), `-cE '^[[:space:]]+Endpoint:'`
(the pbx health line), `-v ' total$'` (`check-source-size`), `-q` with no match,
`-c` with no match (prints `0`, rc 1 — the shape that killed a printer's post-start
under `set -e`), `-qi thingino /etc/os-release`, `-h` on a tree, and BRE staying BRE.

| class | what changes | who could be fooled |
| --- | --- | --- |
| **order** (2 cases) | compat mode sorts by **path name**; GNU gives readdir order. Same lines, different order. | A recipe that takes a recursive `grep`'s output in order — e.g. the list AGENTS.md rule 23 says is *the* list of what is not running, `grep -rl '^# PARKED' machines/`, or anything piping `grep -rn <needle> common/` into a second stage that reads line 1. Sorted is at least as defensible as readdir order, and it is now deterministic instead of directory-inode-shaped. The class had a third arm until **2026-11-25**: the parked-list needle was in the gate as an `order` case, and when services3's wired module came out of parking it left exactly one parked file in the tree, where a one-path list has no order to change — so the arm stopped differing and the gate called that a finding (the counts below are asserted precisely so this kind of quiet re-tuning cannot happen). It is retired from `just/checks.just` with a dated note there, and is worth restoring the day a second module is parked. |
| **first-line** (1 case) | `grep -rh <needle> dir/ \| head -1` names a **different file**. | Any recipe that takes the first hit of a multi-file recursive grep. None in the tree depends on this today; the class exists so the next person who writes one gets told rather than served a different file. |
| **notice** (1 case) | GNU writes `grep: FILE: binary file matches` to **stderr**; ugrep writes `Binary file FILE matches` to **stdout**. | Anything counting matches off a **pipe** whose data carries NULs or invalid UTF-8: `journalctl … \| grep x \| wc -l` gains a phantom line. This is the one measured difference that changes an arithmetic result rather than a reading order. |
| **rc** (1 case) | `grep needle docs/` without `-r`: GNU errors (rc 2, `Is a directory`), compat mode **recurses** (rc 1). | A mistyped path stops being an error and becomes a successful recursive search — the typo is hidden. Always pass `-r` explicitly (every command in this repo already does). |

A fifth difference the gate does not classify but that matters under
`set -euo pipefail`: the exit status a **closed** pipe reports differs (GNU died of
SIGPIPE, `rc=141`, in one of the cases above; the drop-in answered `rc=0`). The new
engine is the one that cannot make a recipe fail because of a pipe.

## The speed, and where it is not

Measured on services1 (4 cores, warm cache, pattern `sigma omega`, averages over 15):

| | 5000 files, 36 MB | 62 MB single file | `cat file \| grep` |
| --- | --- | --- | --- |
| GNU grep 3.12 | 60 ms | 75 ms | 79 ms |
| ugrep `-J1` (no threads) | 43 ms | — | — |
| ugrep, default threads | 30 ms | 20 ms | 21 ms |
| ugrep `--mmap=256M` | **7 ms** | **6 ms** | — |

Match counts identical (11155 / 19004 both). Read the shape before repeating this
sentence elsewhere: **the threads buy 1.4×, the engine (SIMD + RE2) buys 2.5–3×, and
`--mmap` is the 10× lever.** On a 4-core box, multicore is the weakest dial in this
result. And no tool here parallelises *inside* one file or one pipe: `rg --threads 1`
and `rg --threads 4` on a single 62 MB file answered identically (26 ms both), measured.
The only real multicore for one big log is splitting the stream (`parallel --pipe
--block 10M`, or `split -n l4` then four greps — measured 80 ms → 42 ms, I/O-bound).
GNU Parallel is in this nixpkgs (`pkgs.parallel` evaluates, `pname = parallel`) but is
not installed on any box; `pkgs.rust-parallel` is.

## What it changes that must not change

`just check`, `just check-eval` and the guards themselves call `grep` —
`check-source-size` pipes `wc -l` through `grep -v ' total$'` and `grep -v ' total$'`
is in the parity set for that reason. The gate runs inside `check-eval`, so a green
`just check` on any box now means "this box's `grep` agrees with that box's gnugrep on
these invocations" instead of "the flake evaluates".

## Not proven here

* **One box switched.** services1 only; the other four need their own `just switch`
  (rule 27: `just deploy <machine>` degenerates to exactly that when typed on the
  machine itself). The merge of `bin/grep` into `system-path` against the gnugrep that
  nixpkgs' base closures also carry was accepted here — the other boxes take the same
  module and the same merge, but say so only after they have switched.
* **`gw`'s CPU.** This box's banner is `+avx2`; the router is a Supermicro X9SCL/X9SCM
  and its `grep --version` banner is unmeasured. One identical binary is substituted to
  every box, so check the banner per box after switching rather than assuming the ISA
  is runtime-detected.
* **The `-e`/`-P` surface beyond what the repo runs.** ugrep's `-P` is PCRE2 with JIT
  (better than GNU's PCRE), but the gate covers this repo's flags, not every regex a
  future recipe may write; a backreference inside `-P` and lookbehind are ugrep's call
  to make, not ours.
* **`egrep`/`fgrep`** are not shipped. Code that types them keeps GNU's.

## Re-verify

```bash
just grep-compat                        # the gate (check-eval runs it); prints why it skipped on an unswitched box
grep --version | head -1                # `ugrep 7.8.5 (grep compat) …` once deployed
ls -l /run/current-system/sw/bin/grep   # a store symlink -> ugrep, not -> gnugrep
nix-store -q --references /run/current-system/sw | grep gnugrep    # GNU is still in the closure: units that name it keep it
# the preview, without deploying anything:
PATH=<dir-with-a-grep-symlink-to-ugrep>:$PATH just grep-compat
# where the win is, on a file of a few tens of MB:
time grep  -r 'sigma omega' <corpus> | wc -l   # then the same through the drop-in
```
