# Simplifying the flake — the pass of 2026-10-24

This is a second audit, written the way the first one should have been written: it
starts from the tree, not from the previous document, and it answers one question —
*what could a person delete, merge or move here, and what would break?*

**Read `docs/flake-simplification-audit-2026-10-22.md` first if you want the history.**
Most of that audit has already been done — its Status table says so — and the part of it
that is still open is two items in `docs/follow-ups.md`. What that audit is *not* is a
list of what is newly fat: it was written at a tree that no longer looks like this one
(its §7 lists eight wrong pointers, all measured at `f923c6e`, and
`common/check-doc-pointers.nix` is what now makes that class of drift a failed eval). The user
asked "is this repo fatter than it needs to be", and the answer, in one sentence:

> **Yes, and not where you expected. 91 % of the repo's tracked bytes are eleven vendored
> upstream binaries for one browser feature. The prose is fat in a different way: three
> of the 25 rules in `AGENTS.md` are 93 of its 151 rule-lines, i.e. narratives that
> belong in the docs they already cite. And ~1,000 lines of config cannot be deployed
> at all. The nix itself is not the problem — 17,690 lines of it, and the two things the
> last audit called "the flake's real complexity" have already been fixed.**

Everything below is a finding with a verdict, in the order a reader should do them. The
verdicts are **do now** / **decide** / **don't**. The gate for every one is the same:
`just check` before, and for anything touching `common/crane.nix`, the drv-path proof
described in the next section. Section 3 lists what may *not* be cut — the file is written
for a reader who will otherwise delete the right-looking things.

---

## 0. The weight, measured

| What | How much | How to see it |
| --- | --- | --- |
| Tracked files, working tree | **24.7 MB**, of which `crates/whisper-ws/web/openwakeword/` is **22.3 MB** (36 files) | `du -sh --exclude=.git .`; `du -sh crates/whisper-ws/web/openwakeword` |
| Nix source | **17,690 lines / 116 files** — `flake.nix` 477, `common/` 4,677 (30 files), `machines/` 12,460 (84), `crates/*.nix` 76 | `git ls-files '*.nix' \| xargs wc -l \| tail -1` |
| `docs/` | **8,143 lines / 26 files** | `git ls-files 'docs/*.md' \| xargs wc -l` |
| Entry text an agent reads every call | **`AGENTS.md` 780**, `README.md` 157, `justfile` 494 | `wc -l AGENTS.md README.md justfile` |
| Config that cannot be deployed | **6 `# PARKED` modules, 1,044 lines** | `grep -rl '^# PARKED' --include='*.nix' machines` |
| The flake's own source, per evaluation, in the store | **26 MB copies — 27 of them on aibox, 669 MB** | see §1 (the command is long; it is quoted there once) |
| `.git` | 517 MB | `du -sh .git` |

The four biggest nix files are one module of **1,213 lines**
(`machines/services1/services/monitoring-dashboards.nix` — Grafana dashboards, i.e. DATA
written as a module) and then 650 (`machines/gw/firewall.nix`, nft sets), 624
(`machines/gw/lib/check-network-tables.nix`, the guard) and 484 (`common/front-door.nix`, the
vhost table). Nothing is a 3,000-line monster.
**The nix is not what makes this repo feel bloated. The 22 MB of binaries and the 780-line
rules file are.**

---

## 1. The 22 MB of vendored browser assets — **do this first**

### What it is

`crates/whisper-ws/web/openwakeword/` vendors the whole OpenWakeWord runtime so the
whisper-ws status page can do wake-word detection **in the browser** ("no CDN, nothing
leaves the LAN"). Its `README.md` already names where every file came from:

| File | Bytes | Upstream |
| --- | --- | --- |
| `ort/ort-wasm-simd-threaded.wasm` | **11.9 MB** | `onnxruntime-web@1.23.2` npm dist |
| `models/*.onnx` (9 models) | **10.4 MB** | `dnavarrom/openwakeword_wasm` `models/` |
| `ort/*.mjs`, `engine/WakeWordEngine.js` | ~0.1 MB | npm dist / GitHub (**the engine file is patched for this repo — it must stay**) |

### Why it costs more than 22 MB

The flake's source (`self`, git-filtered) contains that directory, so **every evaluation
copies 26 MB into the store**, 22 MB of it bytes that have never changed and will not change
until someone upgrades a wake-word model. Measured on aibox: 27 store paths named
`*-source` contain this repo's `common/crane.nix`, 25–26 MB each, 669 MB together — one per
distick evaluation, because a doc edit changes the source and the assets are inside it.

That is the *same* mechanism `docs/nix-dev-workflow.md` spends a page on ("`builtins.getFlake`
of an absolute path re-streams the working tree"). That doc's numbers — 7.7 GB against
"the 26 MB of tracked source" — are right and are about a different mistake, but the thing
that makes "26 MB" big is this directory. It also costs: `git clone`, `git stash`, `just sync-gw`
(`justfile:100`, whose rsync excludes only `target/`, `node_modules/`, `/result`, `/result-*`
and `/.git/config` — so the router's mirror carries the 22 MB of assets *and* the whole
`.git`, which is 517 MB in this checkout and is 517 MB because of these bytes), and every
`-source` in every box's store.

### The change

`engine/WakeWordEngine.js` + the README stay (the engine is patched here); the rest becomes
pinned fetches, which is what the repo's own convention already says it should be —
`flake.nix`'s header: *every input is a network-wide source, nothing is read out of `~/Projects`*.
Concretely, either:

* `flake.nix` inputs: `fetchzip` the npm tarball (`onnxruntime-web/-/onnxruntime-web-1.23.2.tgz`)
  and the models tarball, each with `sha256`, and have `crates/whisper-ws/package.nix`'s
  `owwAssets` assemble the store path from those inputs instead of `src = ./web/openwakeword`;
  or
* `buildNpmPackage`'s npm deps for `onnxruntime-web` — the app already runs `buildNpmPackage`
  with an `npmDepsHash` pin, so one of the four npm packages it needs is already pinned this way.

Payoff, measured: tracked bytes 24.7 MB → ~2.4 MB; every `-source` 26 MB → ~3.7 MB; the
669 MB of `-source` copies on aibox → ~100 MB (they are garbage-collectable, so this is
mostly a *GC and re-fetch* saving, not a saving you get for free — say so in the commit).

### What breaks, and the risk

* **Network at eval time.** The repo is already network-dependent for nixpkgs/crane/sops-nix,
  so this is not a new *kind* of dependency, but it is a new *failure point*: with the inputs
  un-fetched, `nix build .#whisper-ws` on a box with no cache for those paths fails instead
  of compiling 22 MB of copies. The built binary embeds the bytes (`include_bytes!`), so an
  already-running whisper-ws keeps working offline; only a *rebuild* needs the fetch.
* **Upstream rot.** The models come from a personal GitHub repo. If it goes, the pin goes
  with it. The honest mitigation is the repo's own habit: record the URLs and hashes in that
  README table (they are already recorded by name), and if the fetch ever fails, `git log`
  still has the bytes — say that in the README so the next person does not hunt.
* **Rule 8 / rule 21 stay true.** This is not permission to use a `path:` literal for the
  assets: `src = ./web/openwakeword` is already a path literal, and moving it to an input is
  what makes the copy cost-per-eval disappear. Do not "fix" this by adding a `--` filter to
  `cleanSource` around a path literal.

Verdict: **decide, then do** — it is one input stanza and one package file, and it is the
largest single lever in the repo by a factor of ten. If the wake-word page must survive an
offline rebuild, keep the 12 MB `.wasm` vendored and move only the models: 10.4 MB of the
22 MB is still most of the win.

---

## 2. One crane shape for four SPAs — **do now, it is also the prerequisite for everything else**

Four service modules build a Dioxus SPA and a native binary that embeds it. All four write
the same eleven-line shape, and **the same comment twice each**: the block starting
`The binary.  Its build.rs embeds the SPA` appears verbatim in all four files, and the
`pinned wasm-bindgen-cli and the wasm build recipe live in common/crane.nix` sentence in
three of the four.

`machines/services1/services/filestore.nix:45` · `gocardless-dashboard.nix:39` ·
`machines/aibox/frigate-monitor.nix:34` · `machines/services1/services/switch-gui.nix:131`

```nix
frontendDist = CRANE.wasmSpa { pname = "<x>-web"; version = "0.1.0"; src = ...;
  cargoLockPath = ../../../crates/<x>/frontend/Cargo.lock; wasmName = ...; };
pkg = CRANE.crate { pname = "<x>"; member = "<x>"; sharedCrates = [ … ];
  preBuild = "export <X>_DIST=${frontendDist}"; };
```

Plus: three of the four spell `src` as `frontendSrc` and one (`filestore`) inlines the path;
`version` is repeated on both halves; and `sharedCrates` is a hand-written list per crate.

### The change

One helper in `common/crane.nix` — and note that the last audit never looked at this at all
(`grep -c 'wasmSpa' docs/flake-simplification-audit-2026-10-22.md` is **0**), so this is a
finding this pass adds, not one it left open:

```nix
spaApp = name: … # derives crates/<name>/frontend, its Cargo.lock, <name>_web, the DIST var
```

returning `{ binary, dist }`. Four call sites go from ~18 lines to ~6, and the *repeated
comment* goes with the helper, not with the sites. (Keep the per-crate `sharedCrates`
comment where it is — "which common-rs crates this one needs" is a per-crate fact and the
crate's own `Cargo.toml` is the wrong place for it. What dies is the *shape* comment.)

### Why this is a prerequisite

The same four sites are where the open eval-time reads live. `crate` and `wasmSpa` hand
`lockFile`'s `writeText` derivation to crane as `cargoLock` **without** `cargoLockContents`
(read: `common/crane.nix:158`, `:231`, `:322` hand `lockFile`'s derivation to crane as
`cargoLock`; `vendorDirFor`, at `:86`, is the one site that does the pairing — `:92` and `:93`). Crane reads `cargoLock` at eval time, so the read *builds* the derivation,
which is why `just check` on `gw` dies (`Cannot build '…-Cargo.lock.drv'`, then one read
later at `…-filestore-web-src.drv` — both documented in `docs/follow-ups.md` and
`docs/nix-dev-workflow.md`, not re-measured this pass, and see §6).

So: do the helper **and** the pairing in one change. Payoff beyond dedup: `just check`
works on the router, and the **eight** places that currently carry the sentence
"`just check` is broken *on* gw — run it on aibox" collapse to one: `AGENTS.md:106` and
`:124` (rule 22), `AGENTS.md:305` ("Deploying"), `README.md:82`, `docs/nix-dev-workflow.md:229`,
`justfile:377` (the `check:` recipe prints it), `docs/follow-ups.md` (two items),
`docs/network-upgrade-runbook.md:637`. **Those sentences are the repo's largest block of
procedure that exists only because a bug is unfixed** — 53 lines of `AGENTS.md` rule 22 are
mostly this one failure, and the fix must be proven the way `docs/follow-ups.md` already says:
fix the class (no `builtins.readFile` on a derivation), then prove the three toplevel
`drvPath`s unchanged, on aibox, not on gw.

---

## 3. What may **not** be deleted (read this before §4–§8)

A reader with this document and a diff will otherwise delete the right-looking things. These
look big and are load-bearing:

* **The DATA tables** — `common/dns-records.nix` (429), `common/front-door.nix` (484),
  `common/monitoring-endpoints.nix` (342), `common/vlans.nix`, `common/int-dns.nix`. They
  are *the* network. `machines/gw/lib/check-network-tables.nix` refuses an evaluation where
  they disagree, and the router renders `/etc/dnsmasq.d/network.conf` and its nft sets from
  them. Their size is the point: an address is one row in one table so a lease and a vhost
  cannot disagree. `common/monitoring-endpoints.nix` even says "auto-generating this list is
  deliberately not done (repo audit §3.7)" — twice.
* **The long comments in the nix.** They are the contract: `common/check-doc-pointers.nix`
  reads `AGENTS.md`/`README.md`/`justfile` and fails the eval when they promise a recipe, a
  path or a doc that is not there; `# PARKED` first lines are *the* list of what is not
  deployed (`grep -rl '^# PARKED' machines/`); every parked module's re-verify command is
  the thing that makes "just uncomment it" wrong.
* **The guards.** `machines/gw/lib/check-network-tables.nix`, `machines/services1/int-dns.nix`,
  `common/sops.nix`'s `infra.buildTimeSecrets` assertion, `common/nixbld.nix`,
  `machines/gw/lib/check-network-tables.nix`. They are one-fact-one-place made mechanical.
  AGENTS.md rules 23/24 are *about* the guards.
* **`pi-room-sys` as a fifth configuration that runs on no box.** It looks like an orphan
  machine and is a netboot image aibox builds and serves (`machines/aibox/netboot.nix`).
* **`crates/network-status/frontend/dist/` being committed** (rule 19) and the four Dioxus
  `dist/`s being git-ignored and flake-built.
* **The `.env.sample`**, `machines/services1/junk/` (tracked on purpose), the `z` tmpfiles
  rules (they are a repair, not an install — `just install-sops-key` is the mechanism).

Nothing in §4–§8 may remove any of the above. If a change would, the change is wrong.

---

## 4. `AGENTS.md`: three of the 25 rules are the file's rules section

Measured: the "The rules, in one place" section is 151 lines for 25 rules; rules 1–21 average
1–4 lines, and rule 22 is **53 lines**, rule 24 **24**, rule 25 **16**, rule 23 **8**.

Rule 22 is a narrative — the hop turning out to be broken, then the `Cargo.lock` read moving
the failure point, then the SPA-source read, then the front-door flip, then lego/minica.
Its own text already says "Full mechanism, receipts and what was tried:
`docs/nix-dev-workflow.md`" and "…`docs/router-front-door-migration.md`". **AGENTS.md's own
contract — "if you learn something, add it to the doc that owns the topic and leave a
one-line pointer here" — is what these three violate.** They are the biggest block of prose
an agent reads on every single call, and the thing they describe is already in the docs that
the rule cites.

The change: rules 22, 24, 25 become two lines each — the rule, the one fact that decides it,
the pointer — and the narrative moves into `docs/nix-dev-workflow.md` ("Never build on the
router", "The hop itself was broken, separately") and `docs/router-front-door-migration.md`,
which already own those topics. Same shape as the justfile's `_build-host-guard` block
(the ~20 lines at `justfile:27–46`, above the guard at `:47`): the guard prints one line and
points; the story is in the doc.

Payoff: ~780 lines → ~690, and more importantly, the rules section stops being the place
where a *fact about a lock* (the hop) and a *fact about a health check* (`front-door-cert-check`)
and a *fact about a broken command* (`just check`) are stored where nobody will read them in
order. The rules must keep: what decides behaviour, and the pointer.

Verdict: **do now** (docs-only, cannot break a box). This is the one change that makes the
repo *feel* smaller without touching nix.

**Done.** rules 22/24/25 are 13/8/9 lines where they were 54/21/13; `AGENTS.md` is 723 lines
and its rules section 91 for 25 rules. Nothing was deleted that a doc did not already own,
with one exception found while cutting — rule 24's *why the guards are modules, and why the
table-only half stays a `just check` evaluator* existed nowhere else, and now heads a
paragraph in `docs/router-front-door-migration.md`'s "Where the facts live". The justfile's
`_build-host-guard` block went the same way: 14 lines, the story pointed at "Never build on
the router". What the rules keep is what decides a behaviour an agent can get wrong without
reading the doc: the refused recipes, where `just check` runs, the one mandatory check after
a switch on `gw`, the two guard names, and the pin that makes the hop work.

---

## 5. Procedures that are now ceremony — the table the last audit never made

You asked: "some of the documented procedures may not be necessary any more." Here they are,
each with what carries it now, and the verdict. **Do not act on a row whose verdict is
`decide`** — those need a human with the hardware.

| Procedure, where it is written | Why it exists | What could carry it | Verdict |
| --- | --- | --- | --- |
| **`just check`'s documented workaround** — "broken on `gw` as of 2026-10-22, run it on aibox" (`AGENTS.md` rule 22, `AGENTS.md` "Deploying", `README.md:82`, `docs/follow-ups.md` ×2) | an eval-time read of a derivation, in crane | §2 — the `cargoLock`/`cargoLockContents` pairing + the SPA-`src` reader going away | **do (the workaround stays until then; do not start deleting these sentences before the fix lands, and do not start by deleting them, either — the fix is the point)** |
| **`just router-known-hosts`** (`justfile:251`) — rsyncs `known_hosts` into `/var/lib/network-status/known_hosts` for the ssh-probe mode | the mode is dead: `machines/gw/network-status.nix:77` renders `ExecStart` with `--local` (reads local `/proc`), and the same box hosts the thing it used to ssh to | one recipe less; the `--known-hosts`/`--ssh-key` CLI stays for debugging the router *from another box* | **decide** — deleting the recipe leaves no way to provision the probe mode; deleting the CLI is a Rust change. If the probe mode is truly gone, say so in `machines/gw/README.md` and in the recipe's own comment, and keep the recipe. |
| **`just install-sops-key`** | repair of a `z` tmpfiles rule that does nothing when the path is absent (`docs/nix-dev-workflow.md` proved a `z` rule is inert on a missing path), and the audit's open item | a boot-time unit + an assertion, or a boot-time `tmpfiles`d directory with a *unit* whose failure is the eval error (`common/sops.nix` carries an open item saying exactly this) | **already open, still open** — a `systemd.services.sops-key-provision` with `restartTriggers = [ configFileHash ]`, which also closes the "a missing key costs an activation, not an eval" line in `machines/gw/sops.nix`'s header |
| **`just known-hosts`** (`/etc/ssh/ssh_known_hosts` regeneration, `justfile:268`) | rule 25: the remote-build hop is the *daemon's* ssh, so the operator's `~/.ssh/known_hosts` is the wrong file | it is already the right shape (DATA in `common/ssh-known-hosts.nix`, declarative). Keep the recipe — it is the day-a-box-is-reinstalled job. **The stale half is the docs:** `justfile:236` still explains the *old* pin (`/etc/dns-sync/known_hosts`, read by a tool that no longer exists) inside the recipe's own comment block | **do (trim the narrative into `docs/nix-dev-workflow.md`; keep the recipe)** |
| **the `10.3.1.254` sentences still standing in the instruction files** (`justfile:60–63`, above `GW_HOST := "10.3.1.1"`; `machines/gw/README.md` ×4, each marked historical) | the 2026-10-21 cutover; `common/check-doc-pointers.nix` check 4 *refuses* an un-historical line, which is why the historical ones survive at all | one sentence in the recipe's comment; the rest is `machines/gw/README.md`'s, which is a dated doc and may keep them | **decide** — the checker is what makes the marking enforceable; do not delete the checker's shadow-word list (`:282`) to make these lines removable |
| **a machine address spelled in a recipe** (`justfile:64`: `GW_HOST := "10.3.1.1"`) — the address is *right*, but it is a copy | rule 25's hop and `common/hosts.nix` are supposed to be the one table of addresses; `just known-hosts` (`:200`-region) already `nix eval`s that table for the same job | `GW_HOST := $(nix eval --impure --raw --expr '(import ./common/hosts.nix).router')`, same as `known-hosts` already does | **do — small.** The grep that matters is `10\.3\.1\.[0-9]+` across the tree: **141 occurrences in 46 nix files, 7 in the justfile**, of which 103 are on lines that are not comments. That count is *counted, not triaged*: a line carrying an address is either the table itself, an interpolation (`http://${hosts.aibox}:4200`), generated text, or a second spelling — the triage is §6 item 3 below, which names the ones this pass could prove are duplicated |
| **`just switch-netboot`'s hardcoded client loop** (`justfile:281`, with `BASE_IP="10.3.14."` at `:291` and the loop at `:303`) | the Pi Room clients | those ten clients **are rows in `common/dns-records.nix`** (PI-ROOM-01…10, and that file even says "the address number is the room number"), and the iPXE menu already reads the table | **do — derive the loop from the table** (one list, `nix eval`'d, plus the one `sudo umount` line that stays a command). This is the classic "same fact, three places" the whole audit keeps finding, in a recipe rather than in nix |
| **`just check-build` promising four machines** while `aibox` cannot build the door-entry package (`__noChroot` vs `sandbox = true`, `docs/follow-ups.md`) | a package that cannot be built in a sandbox | the audit already says: a recipe per machine, or the recipe says what it can do on the builder | **decide, and cheap-fix now:** the list is inline (`justfile:436`: `for m in services1 services2 aibox gw`); name it in the recipe's echo so a box that cannot build the door-entry package says so in its own line |
| **`check-config-to-gitlab.nix` running as one of `just check`'s four assertion evals** while `gitlab.nix` is `# PARKED` on services1 | it guards a serializer for a container that is not deployed (its own header says "when the module is parked it says so too… a green line about a container that is not deployed is not evidence") | park it with the thing it guards (same date/commit/doc convention, rule 23), which removes 153 + 81 + 49 + 140 = 423 lines (`check-config-to-gitlab.nix`, `config-to-gitlab.nix`, `gitlab-config-example.nix`, `services/gitlab.nix`) from the "must stay green" surface and one of the four assertion evals | **decide** — un-parking gitlab needs it back, and that is exactly what a `# PARKED` line says it needs. Do not delete the file; park it. |
| **`just pxe-client-bios` / `just pxe-client-uefi`** (`justfile:317`, `:328`) writing `./tmp/OVMF_CODE.fd` and `./tmp/OVMF_VARS.fd` **into the repo** (`:336–337`; `tmp/` is git-ignored, but the tree then holds the firmware), **and** building `nixpkgs#OVMF.fd` at `:332` **without** `_build-host-guard` | dev aids for testing the netboot image | the firmware belongs in `/tmp`; and these two recipes are the only build-shaped recipes not guarded by `_build-host-guard`, so on `gw` they are the documented way to start a build on the resolver | **do (two lines) for the path; decide for the guard** — `OVMF.fd` is substituted from nixpkgs in practice, so it is not a compile, but it is a `nix build` on the box that answers DNS, and every other `nix build` recipe refuses there |
| **two commented-out `llama-cpp` pins inside an overlay** (`flake.nix:279`, `:281`: `# llama-cpp-leigh-rocm = …`, `# llama-cpp-cpu = llama-cpp.packages.…default`) in aibox's `nixpkgs.overlays` | the same class the audit found in `inputs` — "dead by definition, but it reads as an alternative pin", and here it *is* a live-looking alternative to the one engine aibox runs | the two alternatives in the comment above (`:272–274`) or in a doc; the overlay keeps one line | **do** — same verdict the audit gave for this class in `inputs`; there is no `machines/aibox/README.md`, so the comment is the place, and if it grows, that file is the new one |
| **`README.md:63`'s fact-table row for a build-time secret's name** says "also written in: the header comment of `machines/services1/sops.nix`" | `machines/services1/sops.nix:4–10` now says the opposite ("the build-time half is NOT listed here any more… `infra.buildTimeSecrets` is the list") — the audit §4.1 fix happened and the README row did not follow | one cell: `infra.buildTimeSecrets` (`common/sops.nix`) is the owner, and "caught by" becomes "the eval refuses a name not in it" | **do** — this is the audit's own lesson applied to the README: a prose list of a list is stale within one change, and `check-doc-pointers.nix` only checks paths and recipe names, not what a sentence claims about a file |
| **`reboot.sh`**, tracked at the top level, referenced by nothing | the justfile has `hard-reboot` inline; `grep reboot.sh` finds no reference in the whole tree (the `check-doc-pointers` checker only scans `AGENTS.md`/`README.md`/`justfile`, so a dead top-level file is invisible to it) | delete, or move into `machines/services1/junk/` with a header saying why it is kept | **do** |
| **the assertion-eval list written into prose** — `README.md:126` and `AGENTS.md` "Deploying" name all four of `flake.nix:362–400`'s `checks.<system>`; `docs/follow-ups.md`'s gw-workaround paragraph names **three** and calls them "the four standalone assertion evals" | the eval-only subset that survives on gw, which is the thing a reader reaches for when the gate is red | have the recipe print the list (`just check`'s own comment can point at `flake.nix`'s `checks`), or at least stop re-listing it | **do** — it is one wrong list in one doc, and it is the same shape rule 23 replaced: a set of five-named-things written where the set lives is a list nobody updates |

---

## 6. The same fact in three places, where the last audit stopped at DNS

The last audit's headline was about DNS/vhosts. Measured now, the pattern is still there in
three more shapes, and each has a one-place answer:

1. **Domain names.** `int.leighhack.org` is written **167 times across 47 nix files** (bare
   `leighhack.org`: 294 times across 60). Both constants exist — `common/config.nix:64` is
   `LOCAL_DOMAIN = "int.leighhack.org"` (used **14** times, in 15 files) and `:73` is
   `PUBLIC_DOMAIN = "leighhack.org"` (**5** times, 4 files) — and the constant's own comment at
   `:66–68` already says who still spells the name out: *"Not yet used by the files that also
   spell the name out (`common/nginx/`, the gatus endpoints, the machines' `networking.search`):
   open in `docs/follow-ups.md`."* Most of the 167 are **attribute keys** of `virtualHosts`
   attrsets and keys of the dnsmasq/int-dns tables, which cannot interpolate a value; the
   interpolable subset is the URL strings, and that subset is the open item above. **Verdict:
   don't** — do not hand-replace 167 occurrences, and do not re-audit this finding: the comment
   at `:66–68` is what keeps it from looking open. Keep it.
2. **Import depth**: `common/config.nix` is imported on **25 lines** and `common/int-dns.nix`
   on **8**, out of **80 relative `common/` imports** in the tree (38 at `../../../`, 41 at
   `../../`, 1 at `../`). The depth is not
   decoration — it is the thing that makes `import ../common/x.nix` from a machine file read
   that file while `import ./common/x.nix` reads nothing, and `docs/nix-dev-workflow.md`
   warns about the *other* depth trap (a path literal that resolves against the working tree).
   `flake.nix` already passes `flakeInputs` via `specialArgs`; passing `{ CONFIG, HOSTS, CRANE, … }`
   the same way would make depth stop mattering and delete the repeated `import ../../common/config.nix` boilerplate at
   every use site. **Verdict: decide** — it removes 80 chances to get `../` wrong and adds one
   `specialArgs` block; the cost is that a module's arguments are no longer visible at its
   top line. The audit §2.4 says the same thing about `infra`: *leave `infra` alone, this is
   not that* — `infra` is the option namespace, this is an argument-passing shortcut, and the
   audit's objection ("an argument shadows an option") does not apply here. Worth a small
   experiment on one machine directory before it goes fleet-wide.
3. **The four SPA shapes** (§2). Verdict **do**.
4. **Addresses, where the table exists and is not always read.** This is already an open row
   in `README.md`'s fact table (`:57`: the NAS address, "caught by: **nothing**") — the audit
   found it and the README carries it. What this pass adds is the size: the NAS address is in
   **five places** (`hosts.nix:13`, `common/nas.nix:51` — an option default nothing overrides,
   `common/front-door.nix:273` — a vhost row, `common/dns-records.nix:75`,
   `machines/pi-room-sys/configuration.nix:7` — `homeServer`), and the pattern is wider than
   the NAS: `common/front-door.nix` reads `hosts.nix` **17** times and hard-codes **10** rows
   (`:225–:320`: the VM at 10.3.1.30, the authentik appliance at .36, Proxmox at .11, the NAS
   at .6, yunohost at .39) — all of them boxes `common/dns-records.nix` already names. **Verdict:
   decide** — `hosts.nix`'s header is the permission ("one table so the nginx vhost proxies,
   the Prometheus scrape targets and the gatus checks cannot disagree about where a box lives"),
   and it already mixes non-flake entries (the NAS, the three switches). Nothing catches the
   drift: `check-network-tables.nix` compares `hosts.nix` against `dns-records.nix`,
   `vlans.nix` and the ACL — never against a `front-door.nix` row.

**Already shared — do not look for a second copy here.** The two status dashboards
(`machines/aibox/status-dashboard.nix`, `machines/services2/status.nix`) both import
`common/status-dashboard.nix` and set only their own option block; the two Strata packages
are `machines/aibox/strata-package.nix` and `machines/services2/strata-package.nix` (called
at `flake.nix:333` and `:334`), and the two *modules* differ by 29 lines, most of which is
the park marker. The dedup the last audit was looking for is already done in both cases —
if a reader comes here for "two copies of a service module", this pass found none.

---

## 7. Config that cannot be deployed — **decide, don't delete**

1,044 lines in six modules that are never evaluated (`grep -rl '^# PARKED' --include='*.nix'
machines common`, line counts: `services1/services/authentik.nix` 421, `aibox/strata.nix` 302,
`services1/services/gitlab.nix` 140, `affine.nix` 89, `samba.nix` 50, `aibox/nvidia.nix` 42).
They are correctly marked (rule 23) and the marking is load-bearing, so the question is only
*where* dead config lives:

* **keep as-is** — the honest default: a parked module is a decision with a date, a commit,
  the doc that owns it, and a re-verify command. Deleting it deletes the reason.
* **move to one directory** (`graveyard/` or the existing `machines/services1/junk/` habit,
  which is *tracked on purpose*) so `grep -rl '^# PARKED' machines/` stays the list and the
  nix stops being a candidate for someone's `imports`. Payoff: the 1,044 lines stop being
  read as "config". Cost: the import lists' one-liners have to say where the file went,
  which is a second place to be right — the same shape rule 23 exists to avoid.
* **the authentik one (421 lines) is the special case.** `docs/authentik-container-migration.md`
  says the appliance is the live IdP and the flake half is a parked shadow; the shadow was
  built, verified end to end, and the cutover is a flag. 421 lines of *working* config for an
  appliance that no longer wants a shadow is the most likely thing to rot silently. If the
  decision is "the container authentik is not happening", the module and
  `machines/services1/services/authentik-migrate.sh` (shell, against rule 1, kept for the
  migration it did — the only two files that go together) and the doc's story go together.
  and the doc keeps the whole story. **Verdict: decide with the human who owns the IdP.**

Related: `machines/services1/lib/gitlab-config-example.nix` (49 lines, 2,138 bytes) exists precisely
so the parked gitlab's assertions still have something to assert over. If gitlab is parked,
those four files (`services/gitlab.nix`, `lib/config-to-gitlab.nix`,
`lib/check-config-to-gitlab.nix`, `lib/gitlab-config-example.nix`) park together — and
`check-config-to-gitlab` is one of the four evals `just check` runs, so parking it is also
what stops the gate spending an eval on config that is not deployed.

---

## 8. Two things that look like fat and are two bugs already written down

Do not "fix" either from this document — they are the two open items in
`docs/follow-ups.md`, and both are about *secret material in a world-readable store path*:

* `config.sopsSecretText` is now *gated* (`infra.buildTimeSecrets`, every name with a
  sentence — the audit's §4.1 is enforced), so the audit's finding is narrower than it was
  written. What remains is the mechanism, not the list: the decryption is an eval-time
  `runProcess`, so it **builds a derivation during evaluation** — the same class as §2's
  crane reads, "they fail in a fixed order: gw dies on `Cargo.lock.drv` before the secret one
  is reached". The two should be fixed by whoever fixes the class: no `builtins.readFile` on a
  derivation in a module.
* `common/nginx-sso-config.nix` no longer puts the *whole* config in a store path (the audit's
  §4 finding 1 as written is gone); only two named secrets go through it, each with its
  sentence. Follow-ups' item still says `nginx_sso_config` — that name is now nowhere in the
  tree (`grep -rn nginx_sso_config --include='*.nix' .` → nothing). **The item needs
  re-reading, not re-doing.**

---

## 9. The order, and the gate

Worked through from 2026-10-24, one item per commit, each marked where it landed and what
proved it. The commits are `git log --oneline` forward from `d78a6d5` (which lands this file).

- [x] 1. **`AGENTS.md` rules 22/24/25 → 2 lines each** (§4). Docs-only, zero risk, biggest
   "feels smaller" effect. Same commit: the justfile's guard narrative. — **done**; see §4
   for the measured result and the one fact that had to move into a doc rather than be cut.
- [ ] 2. **The asset move** (§1). Largest real lever. Prove it with `du -sh` before/after and one
   `nix flake check` on aibox.
- [ ] 3. **The SPA helper + the `cargoLockContents` pairing** (§2). This is the prerequisite for
   making `just check` uniform; do the class fix (no eval-time `readFile` on a derivation,
   §8) in the same change, and run the drv-path proof the follow-up prescribes, on aibox.
- [ ] 4. **The small, obviously-safe nits**: `reboot.sh` (§5), `just switch-gw` reading
   `common/hosts.nix`, `just switch-netboot` deriving its client list from
   `common/dns-records.nix`, `pxe-client-uefi`'s firmware into `/tmp`, the stale sentences in
   the recipes' own comments (§5 rows marked **do**).
- [ ] 5. **The `# PARKED` decisions** (§7) with the human who owns each service.
- [ ] 6. **The import-argument experiment** (§6.2) on one machine directory, not fleet-wide.
- [ ] 7. **Nothing else.** The DATA tables, the guards, the narrative comments in nix, `pi-room-sys`
   (§3) are the shape; leaving them is not laziness, it is the design.

Gate for all of it: `just check` (on aibox, not on gw, until 3 is done), `just check-build`
for anything that builds, the drv-path proof for anything touching `common/crane.nix`
(`docs/nix-dev-workflow.md`: compare the three toplevel `drvPath`s before/after),
`nix eval --impure --raw -f common/check-doc-pointers.nix` after any change to
`AGENTS.md`/`README.md`/`justfile`/`docs/`, and rule 5 (`git add` new files — untracked is
invisible to the flake).

---

## 10. What this pass measured, what it read, what it did not verify

**Measured with commands this session** (on gw's checkout, branch `main`): every number in
§0's table and the counts quoted in §4–§7 — 17,690 nix lines / 116 files; 26 MB flake source;
27 `-source` copies of 25–26 MB each on aibox's store containing `common/crane.nix` and
`crates/whisper-ws/web/openwakeword/` (22.3 MB of each); 167 `int.leighhack.org` literals in
47 files; 80 relative `common/` imports (38 at `../../../`), `common/config.nix` imported on
25 lines and `common/int-dns.nix` on 8; 1,044 lines across 6 `# PARKED` modules; rule 22 = 53
lines of a 151-line rules section; `monitoring-dashboards.nix` = 1,213 lines; `crates/whisper-ws/web/openwakeword` file sizes; `check-doc-pointers.nix`'s
scanned-file list.

**Read, not run**: `lockFile`'s result reaching `cargoLock` without `cargoLockContents`
(`common/crane.nix`); the four SPA call sites; `network-status`'s `--local`
`ExecStart` (`machines/gw/network-status.nix:77`) vs its `--known-hosts` default
(`crates/network-status/src/main.rs:1002`); `check-config-to-gitlab`'s own caveat; the
`# PARKED` first lines; `monitoring-endpoints.nix`'s "deliberately not done" paragraph.

**Not verified, and stated as such rather than tested**: that `just check` still fails on
`gw` today (it is documented as measured there on 2026-10-22/24, at two different reads;
re-running it costs minutes on the box that answers DNS for the network — rule 21 says
deploy-and-read, and the code read above is why the failure exists at all); that the fetch
URLs in §1 are reachable and hash-stable; that `check-build` on aibox still stops at the
door-entry package; whether the `--known-hosts` probe mode is used by anything not in this
repo; whether aibox's GC has already pruned some of the 27 `-source` copies.

**The commands that answer the three that matter** (each cheap, each bounded):

```bash
# is the flake's own store copy still fat? (aibox)
nix-store -q --references /nix/store/*-source 2>/dev/null | head # then: for p in $(grep -l common/crane.nix …); nix path-info -S "$p"
# does the eval read a derivation? (aibox; the proof §2 must remove)
nix flake check --no-build 2>&1 | grep -E 'not valid|Cannot build'
# the doc gate this pass edits
nix eval --impure --raw -f common/check-doc-pointers.nix
```

## 11. Where the last audit is now stale (the four-plus it did not have)

The 2026-10-22 audit's Status table is honest and current at the commit it was written at
(`f923c6e`). Its **body** is not, and a reader who follows it will do work that is done:

| The audit's body says | The tree now |
| --- | --- |
| §1.1 — "a `*.int` vhost name … 152 literal `int.leighhack.org` strings across 46 `.nix` files (measured)" | **the table was added** — it is `README.md:40` ("Where a fact lives") and its rows at `:52–65`, written as approximations (`~150` literals across `~45` files) and `nothing` for the NAS row. This pass measured 167 in 47, and five spellings of the NAS address. So §1.1 is **done, and its numbers are the ones that drift** — the row to touch is the README's, not the audit's |
| §2.4 — "`infra` is not a module option, don't fix that" | still true, and `flake.nix:42`'s comment now says *why* it stays |
| §2.5 — "four commented-out input URLs (`flake.nix:9,14,18,19`) are live-looking" | gone, and `flake.nix:3–9` now says *"there are deliberately no commented-out alternative URLs in this block"* plus the `--override-input` way to try one — the audit's rule was enforced with a sentence. **The same class is still live 270 lines later**: two commented-out pins in aibox's `nixpkgs.overlays` (`:279`, `:281`), see §5. The strata narrative is still nine lines at `:51–59`, but it now cites `machines/services2/STRATA.md`, so it is a pointer, not a duplication |
| §3.4 — "`nix.maxJobs` still works, but it is the deprecated spelling, so it emits a warning" | `machines/gw/default.nix` sets `nix.settings.max-jobs` (the renamed spelling, comment at `:53`) — **this pass did not re-run `nix flake check`**, so the audit's two remaining warnings (Grafana's plaintext `secret_key`, four `stdenv.is*` deprecations behind `packages.strata-cuda`) are inherited, not confirmed; rule 9 says re-read that baseline at the current lock rather than pass it down |
| §5 — "gw has no remote builder; `just switch-gw` builds on the router" | `gw` delegates to aibox with `max-jobs = 0` (rule 22); the "build on the router" path is now the emergency fallback, and `just switch` on gw would be refused by the guard |
| §6 — "add the `# PARKED` first line, then `grep -rl '^# PARKED' machines/` **is** the list" | it is: 6 modules, 1,044 lines |
| §7.1 / §7.3 — split `docs/nix-dev-workflow.md`; `README.md` gets the fact table | both already there (`just check` steps in `README.md:119–130`) |
| §8 — "`just fmt-check` is safe on gw" | still safe, and measured this pass: `nix fmt -- --check` over the tracked `.nix` files reports 116 files, Alejandra-clean, in **1.3 s** on gw |

The fix for that table is not to rewrite the audit: it is that the audit already carries a
Status table, and a pointer to it should say *"the body is the 2026-10-22 tree; the Status
table is now."* — which is what the row added to `AGENTS.md` does.
