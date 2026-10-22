# Simplifying the flake — what trips a newcomer, and what would make it auditable (2026-10-22)

A read of `flake.nix`, `common/`, `machines/` and the justfile looking for one thing:
**where does a competent person who has not seen this repo lose time or make a wrong
call?** Not a bug hunt — `docs/follow-ups.md` is the live backlog for that, and several
findings below are already open there; what is added here is the *cost* of them to a
reader, plus the structural changes that would remove the class rather than the
instance. Everything asserted is marked with how it was checked: **measured** (a command
in Appendix A, run at `f923c6e` on 2026-10-22), **read** (a file:line in the tree), or
**inferred** (flagged as such).

Numbers, so the scale is visible: 112 tracked `.nix` files, 15 864 lines, 5
`nixosConfigurations`, 26 MB of tracked source. `AGENTS.md` is 611 lines and `docs/` is
7 595 lines — the ratio is about 1 : 1 : 1 between config, rules and prose, and that
balance is itself the main finding: **the flake is not hard because it is clever, it is
hard because the same fact lives in three places and the two documents that explain it
disagree about the router.**

---

## Status

Updated as items land, so this section is the only thing a reader has to trust; every
row names what was actually done, not what was planned.

| § | Item | State |
| --- | --- | --- |
| 7 | `AGENTS.md`'s router text and its `dns-sync` rules | **done 2026-10-22** — rule 14 and the Deploying bullet now name `common/int-dns.nix`, and every "replacement router / 10.3.1.254 / co-exist" claim in `AGENTS.md` is gone (the layout section says the box *is* the router at 10.3.1.1, and that its checkout is a `--delete` mirror rather than a place to edit) |
| 7 | the other six wrong pointers in the drift table | **done 2026-10-22** — `justfile`'s network-status note, `machines/gw/networking.nix`, `printer-monitoring.nix`, `machines/services2/configuration.nix`, `common/dns-records.nix`, and `secrets/.env.sample`'s real path |
| 6 | the stale counts ("all three machines") | **done 2026-10-22** — `AGENTS.md` says four deployable machines and five configurations, `common/nas.nix`/`common/containers.nix` name the three that import them, `machines/gw/configuration.nix` and the `just check` comment stop saying "both"/"all three" |
| 3 | `nix.maxJobs` → `nix.settings.max-jobs` | **done 2026-10-22** — the option, the three places that quoted it as load-bearing, and the docs; verified by evaluating `nixosConfigurations.gw.config.warnings` (now `[]`) and `nix config show`'s `max-jobs` is unaffected. `docs/nix-dev-workflow.md` gained the misattribution note the same row asked for |
| 2.3 | `door-entry-management-system` follows `nixpkgs` | **done 2026-10-22** — `inputs.nixpkgs.follows = "nixpkgs"`, `nix flake lock` drops the second nixpkgs node (19 lock nodes → 18, the 435 MB eval-time fetch gone), and the proof asked for was run: `nixosConfigurations.services1.pkgs.door-entry-management-system` builds against the network's nixpkgs in 37 s. Two facts that came out of the proof and are **pre-existing, not caused by this change** — both read off the old lock's drv on services1 as well as the new one's on aibox: the package sets `__noChroot`, so aibox's `sandbox = true` daemon refuses it outright and only services1/services2 (relaxed) can build it, and it is not reproducible (`--rebuild` reports `may not be deterministic`). Both are in `docs/follow-ups.md` now, because the first of them makes `just check-build` fail on the builder for a reason that has nothing to do with the change being checked |
| 1.1, 3 | the fact table and `just check` in `README.md` | **done 2026-10-22** — `README.md` gained "Where a fact lives" (the table above, with the five-layer chain in front of it), "The four commands" with the two things that bite, the `machines/<m>/` shadows `common/` line, and the three machines its Machines section had never heard of |
| 2.1, 2.2, 2.6 | `mkSystem`, `flakeInputs` in `specialArgs`, `checks.` | **done 2026-10-22** — five copies → `mkSystem` + two named module lists (359 lines of `flake.nix` rewritten, the machine list from 176 lines to ~50); `flakeInputs` moved into `specialArgs`, so the four machine dirs are plain modules and aibox's double signature is gone; the three assertion files are now also `checks.${system}`, evaluated (not built) by `nix flake check --no-build` and proved to bite by breaking `common/hosts.nix` and watching the checker's own message come out. **Proved neutral, not argued:** `services1`, `services2` and `gw`'s toplevel `drvPath`s are byte-identical before and after, and aibox's `/etc` differs in exactly one line — the squashfs path of the netboot bind mount, which is the known pi-room-sys churn (`docs/follow-ups.md` has the measurement) |
| 2.4, 2.5 | devShell that builds nothing, dead text in `flake.nix` | **done 2026-10-22** — `devShells.default` is all-substituted and `devShells.wasm` carries the compiled `wasm-bindgen-cli`; the four justfile recipes that build a bundle ask for `.#wasm` by name, and the docs that said "`nix develop` is a build" now say when it was and which shell still is. The stale "gw has no remote builder" sentence, the four `# url =` lines, the nine-line strata narrative (STRATA.md owns it) and the second `import nixpkgs` in the formatter are gone; `nix-shell-registry` is `useFlakeNixpkgsAsRegistry`; `packages.${system}` keeps no `default` — the three attributes it does have are named where they are defined, and a flake with five machines has no honest single default |
| 4.1 | the nginx-sso secret off `sopsSecretText` | open |
| 4.2-4, tmpfiles note | "what build-time decryption costs", one decrypt per secret, `yq` quoting, the key-exists assertion | open |
| 7 | the `docs-lint` binary | open |
| 5 | `streams.ignore`, `common/overrides/`, the `# MODULE:`/`# DATA:` headers | **done 2026-10-22** — every file in `common/` opens `MODULE`/`DATA`/`HELPER`/`CHECK`/`PACKAGE` (README's layout note carries the legend and the grep), `common/overrides/ipxe.nix` is `common/ipxe.nix` with one call site moved, `streams.ignore` is a tracked, labelled dead fragment under `machines/services1/junk/` (the only `junk/` the .gitignore lets through, and the reason written where someone would delete it), and `common/systemd.nix` keeps its name because the header now says it is a HELPER. `machines/<m>/` shadows `common/` is one README line. `hosts.nix`/`config.nix`/`vlans.nix` as options stays unscheduled — the audit's own call |
| 6 | the `# PARKED` convention, and `check-config-to-gitlab` checking nothing | **done 2026-10-22** — AGENTS.md rule 23 fixes the shape, the six parked modules carry it as their first line (each with its own re-verify command), the two import lists are now names, and `grep -rl '^# PARKED' machines/` prints exactly six files. `check-config-to-gitlab` reads the same import list the deploy reads and says, on stderr and only while the module is off, what its green line is *not* evidence of |
| 5 | `hosts.nix`/`config.nix`/`vlans.nix` as options | **deliberately not scheduled** (audit's own call) |

---

## 1. The five layers, and a map of where facts live

A newcomer's first task is "where does this setting come from", and the answer is a
five-layer chain with no single file that states it:

```
flake.nix                               the machine list, pkgs, the shared module list,
                                        the inputs, the outputs that are not machines
  └ machines/<m>/default.nix            hostName, build-farm options, the import list
      └ machines/<m>/configuration.nix  stateVersion, kernel/modules, what differs
          └ common/base.nix             timezone, bootloader, sudo, autoRollback
      └ machines/<m>/services/*.nix     the apps
common/*.nix                            shared modules, and shared *data* tables
```

All five layers can define the same option, and `flake.nix`'s module list is the
only place that says which of them a machine gets. `nixos-rebuild`'s output does not
show it; the rendered `/etc` does not show it.

### 1.1 The fact table

This is the table the README should carry and does not. Each row is one fact, the file
that owns it, and what catches a mistake.

| Fact | Owned by | Also written in | Caught by |
| --- | --- | --- | --- |
| A machine's address | `common/hosts.nix` | `common/dns-records.nix` (the reservation), `common/vlans.nix` (`ipv4` for the router) | `machines/gw/lib/check-network-tables.nix` (one direction only: every `hosts.nix` address is answered by *some* reservation) |
| The NAS address | `common/hosts.nix` (`nas`) | `common/nas.nix:49` — the option **default** `"10.3.1.6"`, never set anywhere (measured: no `infra.nas.host` assignment in the tree) | nothing |
| A `*.int` vhost name | `common/int-dns.nix` | 152 literal `int.leighhack.org` strings across 46 `.nix` files (measured) | `machines/services1/int-dns.nix` (names vs the vhosts services1 serves) |
| The zone suffix | `common/config.nix`'s `LOCAL_DOMAIN` | the 152 literals above; `follow-ups.md` records this as open | the gw checks use `LOCAL_DOMAIN`, so the literals are the drift |
| Which devices exist and keep an address | `common/dns-records.nix` | — | `machines/gw/lib/check-network-tables.nix` |
| Which VLANs/blocks exist | `common/vlans.nix` | the ACL in `common/config.nix` (as nginx text, re-parsed by `machines/gw/lib/network-tables.nix`) | the same check |
| A runtime secret's path | `common/config.nix` | per-service `CONFIG.X` reads; `machines/<m>/sops.nix` | nothing (a typo is a missing `/run/secrets/<name>`) |
| A build-time secret's name | the 15 files that `readFile` a `sopsSecretText` path at eval time | the header comment of `machines/services1/sops.nix` — which is out of date, see §4 | nothing |
| Which services run | `machines/services1/services/default.nix` | `docs/services.md` (by crate, not by machine) | `nix flake check` for the *imported* ones only |

**Recommendation (highest value, lowest risk):** put this table in `README.md` verbatim,
under a heading like "where a fact lives". It costs nothing to maintain, it is the thing
a newcomer actually wants, and every row already has an owner. Three of the "caught by"
cells are `nothing`, which is the honest answer and worth writing down.

---

## 2. `flake.nix` is five machines written as five copies

### 2.1 The five blocks are the same block

`flake.nix:131,168,198,284` plus `piRoomSys` at `:104` are five `nixpkgs.lib.nixosSystem`
calls differing only in three ways: two extra module imports, one `pkgs` config flag, and
one extra `specialArgs` entry. `import nixpkgs {…}` appears **nine times** (measured);
`config.allowUnfree = true` six times; the shared module list
(`sops containers crane-args build-farm nas tools users`) is copy-pasted **verbatim**
into three of the four blocks, and gw's four of them are a subset you have to diff by
eye. The one fact worth reading in that whole section — *the router deliberately does not
get `nas.nix`/`tools.nix`/`containers.nix`* — is currently expressed as a 30-line comment
next to a list you must diff against another list to confirm.

Suggested shape (behaviour-preserving; the module lists are the diff to check):

```nix
mkSystem = { modules, pkgsConfig ? {}, extraArgs' ? {}}:
  nixpkgs.lib.nixosSystem {
    inherit system;
    pkgs = import nixpkgs {inherit system; config = {allowUnfree = true;} // pkgsConfig;};
    specialArgs = flakeInputs // extraArgs // extraArgs';
    modules = [nix-shell-registry nixos-utils.nixosModules.rollback] ++ modules;
  };

# and one visible per-machine table:
services1 = mkSystem {
  modules = commonModules ++ serviceOnlyModules ++ [nixos-utils.nixosModules.containers
              ./machines/services1];
};
gw = mkSystem {
  # no nas.nix: the router must not wait for a NAS.  no containers.nix: it runs dnsmasq.
  modules = commonModules ++ [./machines/gw];
};
```

Two lists (`commonModules`, `serviceModules`) turn "what does the router not get" into a
two-line diff, and `pkgsConfig = {cudaSupport = true;}` replaces services2's whole
duplicated block. Target: the 176-line `nixosConfigurations` block (`:130-305`) in ~70
lines, every remaining one of them a decision rather than a copy.

### 2.2 `((import ./machines/services1) flakeInputs)` — a curry that exists for one line

Each machine directory's `default.nix` is `flakeInputs: { … }`, called from `flake.nix`.
**read:** the only thing the four call sites need `flakeInputs` for is
`flakeInputs.sops-nix.nixosModules.sops` in their `imports` (the dashed attribute name is
not a legal function-argument pattern, so it cannot come in as an ordinary module
argument). The signature is also inconsistent — `machines/aibox/default.nix` is
`flakeInputs: {config, pkgs, ...}: {…}` while the other three are `flakeInputs: {…}`, so
a newcomer copying aibox's file gets a module that is a function returning a module.

One line in `flake.nix` deletes all four curries:

```nix
extraArgs = { INFRA = …; flakeInputs = flakeInputs; };
```

after which `((import ./machines/services1) flakeInputs)` is just `./machines/services1`,
each `default.nix` becomes a plain module `{flakeInputs, ...}: {…}`, and aibox's odd
double signature disappears. This is the single cheapest simplification in the file.

### 2.3 Every input follows `nixpkgs` except one, and that one costs a second nixpkgs

`nixpkgs` is followed by `nixos-utils`, `llama-cpp`, `nix-software-center`, `sops-nix`
and `strata`. **`door-entry-management-system` (`flake.nix:13`) does not.** Three inputs
are written as a bare `url =` with no nested block — `nixpkgs` itself, `crane`, and this
one — and for the first two that shape is harmless (`crane.mkLib pkgs` never imports
crane's own nixpkgs), so the line that looks like the others is the tell only if you
already know the answer. In `flake.lock` the consequence is a *second* `nixpkgs` node at
`NixOS/nixpkgs` rev `20075955` (19 Aug 2025) while the flake's own nixpkgs is
`nixpkgs-unstable` rev `319e4ce` (14 Oct 2026) — **fourteen months apart**, two stdenvs,
two of everything, and `just update-pkgs` (`nix flake update nixpkgs`) cannot move the
old one. **Measured:** evaluating `services1` on aibox fetched
`/nix/store/pxnx7dbr…-source`, 435 MB, from cache.nixos.org — i.e. the *evaluation* is
not hermetic and not offline-safe because of this one missing line.

Fix: add `inputs.nixpkgs.follows = "nixpkgs";` to that input, then `nix flake lock` (it
re-resolves the follow edge and drops the extra `nixpkgs` node from the lock), then build
the door-entry package before trusting it (that package will start compiling against the
network's nixpkgs for the first time). If it does not build against a current nixpkgs, that is
worth knowing out loud rather than hidden behind a 435 MB pin.

### 2.4 There is one devShell and it compiles Rust

`devShells.${system}.default` contains `CRANE.wasmBindgenCli`, which is **built with
cargo** — the fact `flake.nix:364-376` and `AGENTS.md` rule 22 spend paragraphs on. Two
consequences a newcomer hits in order: (a) `nix develop` on a fresh box compiles Rust
(delegated to aibox now, but still ~minutes); (b) **`nix flake check` without
`--no-build` builds the devShell**, so the universal Nix idiom for "did I break
anything?" is exactly the command `AGENTS.md` forbids on the router. `follow-ups.md`
("Nix / build") already records that `nix flake check` builds `packages.strata`,
`strata-cuda`, `whisper-ws` and the devShell — ~10 min on a lock bump. `just check`
hides this by passing `--no-build`; the README never explains it, and the README is what
a human reads first.

Fix, small and worth it: keep one `devShells.default` that builds nothing (alejandra,
just, git, jq, node) and move the wasm toolchain to `devShells.rust`, then say in
`README.md` that `nix flake check` is not the gate, `just check` is.

### 2.5 Dead and half-dead text in `flake.nix`

- `flake.nix:372` says *"gw has no remote builder, so `nix develop` on it compiles on the
  box that answers DNS"*. **read:** stale — `machines/gw/default.nix:44` sets
  `infra.buildFarm.builderHost = HOSTS.aibox` and `nix.maxJobs = 0` (added 2026-10-21,
  the very same day as the incident this comment describes). A reader of `flake.nix`
  concludes the router is unprotected; a reader of `machines/gw/default.nix` concludes it
  is doubly protected. Both files are current, so the contradiction is invisible without
  reading both.
- Four commented-out input URLs (`flake.nix:9,14,18,19`) are live-looking. A `# url =`
  inside `inputs` is dead by definition, but it reads as an alternative pin, and the
  `strata` block carries 9 lines of narrative (`:37-45`) that belong in
  `docs/nix-dev-workflow.md` — including the line “`inputs.nixpkgs.follows` keeps the eval
  on one nixpkgs”, which is precisely the rule §2.3's exception breaks.
- `nix-shell-registry` (`:64`) is named after a command nobody in this repo runs; what it
  does is pin `nix registry nixpkgs` to the flake's nixpkgs. Rename in place —
  `useFlakeNixpkgsAsRegistry` costs nothing.
- `packages.${system}` has no `default`, so `nix build` (no attribute) fails with
  "does not provide `packages.x86_64-linux.default`", which reads like a broken flake.
  Either add `default = whisper-ws;` or a one-line comment naming the three attributes.

### 2.6 The assertion files are not flake checks

`just check` runs three hand-written evaluators
(`machines/gw/lib/check-network-tables.nix`, `common/check-printer-states.nix`,
`machines/services1/lib/check-config-to-gitlab.nix`) by `nix eval -f`, because they are
plain functions, not modules. **read:** `machines/gw/lib/assert-network-tables.nix:19`
then encodes their contract as `assertion = (import ./check-network-tables.nix) == "ok\n"`
— so the report half and the assertion half are glued by a trailing newline, and the
*only* way to see which record is wrong is to know to run `nix eval --raw -f` by hand.

Fix: expose them under `checks.${system}.…` so `nix flake check` and any future CI run
them, keep the `just check` invocations (they print the useful message), and say so in
both files. Better still: have the checker *return the list of failures* (`["msg", …]`)
and let both callers format it, so `"ok\n"` is not load-bearing.

---

## 3. The gate does not behave the same on every machine

This is the section that costs a newcomer the most, because the failure looks like their
change.

- **`just check` is red off services1. Measured:** `nix flake check --no-build` at
  `f923c6e` on **aibox** exits **1** with
  `error: path 'zicnr1hgclx6vmnrhh17w30sd2h4f8dz-sops-secret-pg_pass.drv' is not valid`,
  while `/nix/store/zicnr1hg…-sops-secret-pg_pass.drv` **is** valid on services1 — which
  is why the same command passes there. This is `follow-ups.md` → "`just check` is
  machine-dependent", open (added today), and the mechanism is §4 below: reading a
  derivation at eval time *builds the secret* (`--no-build` only declines to build the
  command's own goal) — 16 `builtins.readFile (config.sopsSecretText …)` sites in 15 files,
  plus the `readSecret` helper called twice more. Two
  additions to that item: it now fails on the network's **build machine**, so the
  documented fallback ("run the gate on services1 or aibox") has one leg; and the gate is
  what `common/nixbld.nix` points at for its declarative guard ("fails `nix flake check` /
  `just check`"), so on a box where the gate dies early the nixbld assertion never runs.
- **`config.warnings` is not empty and not what `AGENTS.md` rule 9 records. Measured**
  with `nix eval --apply … '.#nixosConfigurations.<m>.config.warnings'`:

  | machine | warning |
  | --- | --- |
  | services1 | `Grafana passwords will be stored as plaintext in the Nix store!` (known) |
  | aibox | 3 × `stdenv.isLinux/isAarch64/isDarwin is deprecated` (known — the CUDA set) |
  | **gw** | **`The option 'nix.maxJobs' … has been renamed to 'nix.settings.max-jobs'`** (not in `follow-ups.md`) |
  | services2, pi-room-sys | none |

  The gw one is a **one-word fix** (`machines/gw/default.nix:46`), it is a renamed option
  so it becomes a hard error when nixpkgs deletes the alias, and `nix.maxJobs = 0` is
  quoted as load-bearing in three places across `AGENTS.md` and the justfile — all three
  can say `nix.settings.max-jobs`, which is the name `nix config show` prints anyway. Note the
  warning's own text blames `/nix/store/…-source/flake.nix`, not the file that defines
  it; that misattribution is worth a sentence in `docs/nix-dev-workflow.md` because it
  sends people to the wrong file.
- **`README.md` does not mention `just check` at all.** Its "Applying Config" list is
  boot/switch/netboot/hard-reboot/`list-generations`. The gate lives only in `AGENTS.md`.
  For a human-first entry point that is backwards: the README should carry the four
  commands (`just check`, `just fmt`, `just switch`, `just switch-gw`) and the two things
  that bite (`--no-build`, and confirm-in-the-same-invocation).

---

## 4. There are two secret models and one comment covering both

`AGENTS.md` states the better one: *"Prefer `EnvironmentFile` on the unit so a
`DynamicUser` service never opens the secret file (the pattern `switch-gui.nix` uses)."*
**read:** `machines/services1/sops.nix` renders ~15 secrets to `/run/secrets/<name>` at
`0440 root:secrets`. And in the same file's header comment: a list of the names that
instead go through `config.sopsSecretText`, which `common/sops.nix` implements as a
`pkgs.runCommand` that decrypts **the whole `secrets.yaml`**, extracts one key with `yq`,
and returns a store path — which callers then `builtins.readFile` into a *built
configuration*.

What that means, in order of how much it should worry a reader:

1. **Anything built this way is world-readable, forever, by name.** Measured on services1
   (paths and modes only; no contents read, per `AGENTS.md` rule 13):
   `nginx-sso.service`'s `ExecStart` carries `-c
   /nix/store/5mg4p1ai…-nginx-sso-config`, mode `-r--r--r--`, and
   `machines/services1/lib/nginx-sso-config.nix` puts `nginx_sso_auth` (the cookie
   *signing* key) and `nginx_sso_client_secret` (an authentik client secret) into that
   file. The justification comment in `common/sops.nix` is written for
   `status_dashboard_lan_token`, "a LAN-only convenience credential, not a real secret" —
   and the same mechanism is quietly carrying the key that mints an `X-WEBAUTH-USER` for
   every SSO-fronted app on the box. `read` + measured: the *intended* audit surface is the
   comment at the top of `machines/services1/sops.nix`, which names seven secrets that go
   down this path; the tree actually sends **ten** through it — the comment has never been
   the list, because `grafana_admin_password`, `grafana_secret_key` and `strata_api_key`
   are read with `sopsSecretText` and are not named there. That is what happens when an
   exception list lives in a comment: `nginx_sso_auth`, `nginx_sso_client_secret`,
   `pg_pass`, `slack_url`, `unifi_db_password`, `mattermost_authentik_secret`,
   `status_dashboard_lan_token` (all listed) plus the three that are not.
   **Direction, not a patch:** nginx-sso is the worst case *and* the easiest to see — the
   config is one file, so `sops.secrets.nginx_sso_config` (0440, `/run/secrets`) plus
   `-c /run/secrets/nginx_sso_config` is a drop-in, and it turns the file back into the
   model `AGENTS.md` already prescribes. Then a rule: **a secret goes through
   `sopsSecretText` only if it must be *in* a generated file**, and each such name gets a
   one-line reason in `machines/<m>/sops.nix`.
2. **The age key is group-readable by the build-user group, and builds are delegated.**
   `common/sops.nix` adjusts `/var/lib/sops-nix/key.txt` to `0440 root:nixbld` (a `z`
   tmpfiles rule — see the audit win below) and adds `/var/lib/sops-nix` to
   `extra-sandbox-paths`; builds run as `nixbldN`; services1 and
   services2 additionally set `nix.settings.sandbox = "relaxed"` (i.e. no sandbox); and
   services1/services2/gw delegate builds to aibox (`common/build-farm.nix`), where the
   same key file exists (`read`: measured `-r--r----- root nixbld` on aibox). So the real
   secret boundary is *"any code that runs during a build, on any machine that takes a
   build"*, not *"root:secrets"*. All of that is true of a design that was chosen
   deliberately; none of it is in one place, and `AGENTS.md`'s secrets section does not
   say it. It should — one paragraph, "what build-time decryption costs".
3. **One decrypt per secret, and the whole file per decrypt.** 18 eval-time reads ⇒ 18
   derivations that each decrypt every secret, keyed on the whole file. Changing
   `slack_url` re-runs all of them and dirties every unit that embeds any secret — which
   is a plausible explanation for "why did this one-line change restart half of
   services1", and is worth writing down even though the fix is the same as 1's.
4. Minor, same file: `yq -r ".${name}"` interpolates the secret name into a shell command
   (`common/sops.nix:56`). Every caller passes a literal today; a name with a quote or a
   dot is a silent subtree read or a broken command. `yq -r '."' + name + '"'`-style
   quoting, or a `lib.nameIsValid`-style assertion, removes the class.

**Concrete audit win:** `sopsSecretsKeyFile`'s description says the tmpfiles rules
"*enforce* the group ownership". `read`: they are `z` rules, and `z` **does nothing when
the path is absent** — verified on gw with a two-line fragment (`z` created nothing; the
`f` rule of the same fragment did). So `just install-sops-key` is the mechanism and the
tmpfiles rules are only a repair of an existing file; skipping the recipe fails later, as
a sops error inside an unrelated build. Either say that in the option's description, or
have the module assert that the key exists when any `sopsSecretText` is read.

---

## 5. Names that mislead

Each of these is a place the filename says one thing and the contents say another. All
`read` from the tree.

| Path | Why it misleads | Fix |
| --- | --- | --- |
| `machines/services1/streams.ignore` | 40 lines of live-looking `location /cams/…` nginx config, **untracked and git-ignored** (`.gitignore` has `*.ignore`), referenced by nothing in the tree (measured: no mention anywhere). It reads like config and is dead text — and `AGENTS.md` rule 5 exists precisely because an ignored file is invisible to the flake. | delete it, or move it to `machines/services1/junk/` and un-ignore |
| `common/overrides/` | holds exactly one file, `ipxe.nix`, and it is *not* an overlay — `machines/aibox/netboot.nix:18` `callPackage`s it by path | move to `common/ipxe.nix` or say "package overrides" in a header |
| `containers.nix`, `int-dns.nix`, `networking.nix`, `configuration.nix`, `status.nix`, `strata.nix`, `sops.nix` | all exist in both `common/` and `machines/services1/` (and several in two machine dirs). `import ./containers.nix` inside a machine means *that machine's* file; the `./common/containers.nix` in `flake.nix` is the other one | unavoidable in NixOS; worth one README line: "a name in `machines/<m>/` shadows `common/`" |
| `common/systemd.nix` | not a module, a `{lib}: {…}` helper set injected as `INFRA` | rename to `infra-systemd.nix` or accept, but see the next row |
| four *kinds* of file in `common/` | `common/` mixes NixOS modules (`nas.nix`: `{config, lib, pkgs, ...}: {…}`), data tables (`hosts.nix`: bare attrset), functors (`systemd.nix`, `monitoring-endpoints.nix`: `{lib}: …`), plain-eval checkers returning strings (`check-printer-states.nix`), and shell scripts. You cannot tell which by looking | two headers-only conventions: every module starts `# MODULE:` and every data table starts `# DATA:` (`hosts.nix`, `vlans.nix`, `dns-records.nix`, `int-dns.nix`, `config.nix`, `printer-states.nix`) — and prefer `_module.args`/options over `import` for the data (next row) |
| `…-source/machines/…` in every nix error | errors quote the store copy, so the path you must edit is the tail of it, and `-source` looks like a third-party tree | one line in `docs/nix-dev-workflow.md`: "`/nix/store/…-source/` **is** this repo, at the commit you ran" |

The `import` pattern itself deserves a sentence: `common/config.nix` and
`common/hosts.nix` are pulled in by **40 path literals with three different relative
spellings** (`../../../common/config.nix`, `../../common/config.nix`,
`../config.nix`; measured). That works and is idiomatic, but it means adding a module
requires getting `../` depth right and there is no way to override a fact for a test. If
`hosts.nix`/`config.nix`/`vlans.nix` became options in a `common/facts.nix` module
(`infra.hosts.services1`, `infra.config.localNetwork`) the 40 call sites shrink to one
argument name, and the tables get `mkOption` descriptions and defaults for free. Worth
doing once §1.1's table exists, not before.

---

## 6. Parked and commented-out state has no shape

- 25 lines are commented-out `# ./something.nix` imports (measured). Most are honest —
  `machines/services1/services/default.nix` explains each — but the effect is that
  **"what does services1 run" is the uncommented subset of an import list**, and a
  newcomer cannot grep it, `nix flake show` does not tell them, and the parked files are
  *not evaluated at all* (the header says so: "they can rot silently"). Concretely:
  `affine.nix`, `authentik.nix`, `gitlab.nix`, `samba.nix`, `aibox/nvidia.nix`,
  `aibox/strata.nix`, `gw`'s coexist-era comments and `crates/dns-sync` (deleted) are
  eight parked things in four directories, and only authentik's parking is owned by a
  *document* rather than by the comment beside the commented-out import.
- `lib/check-config-to-gitlab.nix` still runs from `just check` and prints `ok`
  (measured) for a GitLab container that is not deployed. A green check that checks
  nothing is worse than no check, because it is quoted as evidence.
- `common/users.nix` carries a commented-out `cjdell` account with two public keys. Fine
  as a reminder, but it reads like a disabled account that should be re-enabled.
- The counts in the prose are stale in the direction that matters: `AGENTS.md` says
  auto-rollback is on "**all three** machines" and `just check-build` builds "all three
  machines' system toplevels"; `just check-build` builds **four** (measured from the
  justfile), and `flake.nix` has five configurations. `common/nas.nix`,
  `common/containers.nix` headers say "imported for both machines".

**Recommendation:** pick one convention and write it in `AGENTS.md`: a parked module gets
`# PARKED — <date> — <doc that owns it> — re-verify with: <command>` as its **first
line**, and the import list just names the file. Then `grep -l '^# PARKED'` *is* the list
of parked things, which it is not today.

---

## 7. Documentation drift, with the receipts

`AGENTS.md`'s own contract is that it is the rules file and `docs/` owns the detail. The
tree is admirably good at that and has no mechanism to stay there. Live pointers that are
wrong at `f923c6e` (measured: every backticked `` `just <name>` `` checked against
`just --summary`, and every repo path with a file extension in the four *instruction*
files checked against the tree):

| Where | Says | Reality |
| --- | --- | --- |
| `AGENTS.md:61` (rule 14) and `:246-247` | `just dns-sync-sync` / `-prune` / `-check` for new `*.int` names | those recipes no longer exist — they were deleted on 2026-10-22, and later in the *same file* (line 438) that is written down: "`crates/dns-sync` ... was deleted on 2026-10-22, together with the `just dns-sync-*` recipes". Rule 14 is the one an agent obeys first, so it is the one that gets run |
| `AGENTS.md:164-172` (+`:20`, `:112`) | gw is "a **replacement** router **deployed at 10.3.1.254** in **co-existence mode** (`machines/gw/coexist.nix`)… the OPNsense box at 10.3.1.1 is still the network's router", DHCP/RA "emitted commented-out" | the cutover happened 2026-10-21: gw **is** 10.3.1.1 (`AGENTS.md:423`, same file), `coexist.nix` does not exist, and `justfile` uses `GW_HOST=10.3.1.1`. This is not a stale sentence, it is an **inverted safety model**: an agent told "DHCP is inert, it is a shadow" will not treat a DHCP change as dangerous on the box that hands out every address in the building |
| `AGENTS.md:161` | `secrets/.env.sample` | the file is `machines/services1/secrets/.env.sample`; `secrets/` holds only `secrets.yaml` |
| `justfile:200` | "see the note in `machines/services1/network-status.nix`" | that file moved to `machines/gw/network-status.nix` on 2026-10-21 |
| `machines/gw/networking.nix:56` | "The NIC names `machines/constants.nix` owns (see that file's header…)" | `machines/gw/constants.nix` |
| `machines/services1/services/printer-monitoring.nix:8` | the Prometheus scrape job is "configured in `machines/services2/monitoring.nix`" | observability moved to services1 on 2026-10-17; that file does not exist |
| `machines/services2/configuration.nix:7-12` | "module list in `flake.nix` is therefore services1's **minus containers/crane**", "no services enabled yet" | services2 imports `common/containers.nix` **and** `common/crane-args.nix`, and runs the status dashboard + the Strata engine |
| `common/dns-records.nix:6` | "`./hosts.nix`, which is **generated** from this table's mirror image" | `hosts.nix` is hand-maintained; nothing generates it (the one-directional check is `check-network-tables.nix:264`) |

Note which of these are *narrative* and which are *instructions*: the dated docs
(`docs/work-log-2026-10.md`, `docs/repo-audit-2026-10-07.md`) legitimately reference files
that no longer exist — that is what a post-mortem is. The table above is all in the four
files that tell you what to **do**.

**Recommendation: a doc lint, and it fits the house style exactly.** This repo already
writes evaluators for exactly this shape of problem
(`check-printer-states.nix`, `check-network-tables.nix`, `check-config-to-gitlab.nix`).
Add a small Rust binary in `crates/` (rule 1) — call it `docs-lint` — that reads
`AGENTS.md`, `README.md`, `justfile` and every `*.nix`, and fails when:

1. a backticked `` `just <name>` `` in those files is not in `just --summary` (measured:
   four hits — the three `dns-sync-*`, which are gone, and `just update-netboot-input`,
   named twice in past tense (`justfile:254`, `machines/pi-room-sys/README.md:88`), which
   is exactly what an inline allow-comment is for);
2. a repo-relative path with a file extension in those files does not exist
   (**instructions only** — skip `docs/*.md` dated write-ups and any line matching
   `used to|was deleted|moved from|no longer|now deleted`; that filter is what keeps
   `machines/services1/config.nix`, correctly described as deleted, out of the results);
3. a `docs/<file>.md` referenced from `AGENTS.md`'s table is missing, or a `docs/` file is
   referenced by nothing (orphan);
4. the same machine/address appears with two different meanings in `AGENTS.md` — cheap
   version: `10.3.1.254` and "co-exist" may not appear in `AGENTS.md` at all.

Run it as the last step of `just check` (~2 s, no evaluation, no build, so it is safe on
gw by construction). The list in the table above is its first test fixture.

---

## 8. If you only do five things

1. **Fix `AGENTS.md`'s router text and its `dns-sync` rules** (§7). Everything else here
   costs someone an afternoon; this row tells an agent that the box running DHCP for the
   whole building is a harmless shadow, and it is 100 % confident about it.
2. **Add `inputs.nixpkgs.follows = "nixpkgs"` to `door-entry-management-system`** (§2.3).
   One line, one second nixpkgs (435 MB, 14 months stale) and one eval-time network
   download removed.
3. **`nix.maxJobs` → `nix.settings.max-jobs`** in `machines/gw/default.nix` and the three
   `AGENTS.md`/`justfile` mentions (§3). One word, one live warning, one future hard
   error.
4. **Write the fact table into `README.md` and put `just check`/`just fmt` in the README
   too** (§1.1, §3). The README is the only human-first entry point and it currently
   mentions neither the gate nor where a fact lives.
5. **`mkSystem` + `flakeInputs` in `specialArgs` + `checks.`** (§2.1, §2.2, §2.6). The
   176-line `nixosConfigurations` block becomes ~70, the four machine dirs become plain modules, the
   router's *differences* become a two-line diff, and the three custom assertion files
   start running from `nix flake check` instead of only from a justfile recipe.

Then, at leisure, in the order that matches how much they hurt: the `nginx-sso` secret
(§4.1), the doc lint (§7), `streams.ignore` and the naming pass (§5), the parked-module
convention (§6), the `common/sops.nix` cleanups (§4 items 3-4 and the tmpfiles note).

**Deliberately not proposed here:** collapsing `common/`'s data tables into options, or
rewriting the 152 `int.leighhack.org` literals onto `LOCAL_DOMAIN`. Both are right, both
touch every service module, and doing them before the five above means a large diff in
transit while the docs still tell you the router is a shadow. `follow-ups.md` already
tracks the `LOCAL_DOMAIN` half.

---

## 9. Things that look wrong and are not (do not spend a day on these)

- **`pi-room-sys` in `nixosConfigurations`** although `nixos-rebuild --flake .#pi-room-sys`
  does nothing useful: correct — it is a real system that runs on no host, and aibox reads
  it out of `specialArgs`. The consequence worth knowing (and recorded in
  `follow-ups.md`) is that **`just switch` on aibox builds a second OS image**
  (kernel + initrd + squashfs store), because `machines/aibox/netboot.nix:12-13` passes
  the whole `piRoomSys` config through and mounts `${build.squashfsStore}` in aibox's own
  `fileSystems`. Two `nfsServer` spellings in one flow (`10.3.1.32` in `flake.nix`,
  `aibox.int.leighhack.org` in `netboot.nix`) are deliberate and each says why.
- **`import nixpkgs` without `allowUnfree`** at `flake.nix:128` (formatter) and `:351`
  (devShell): alejandra/just/rustc are all free, so the inconsistency is cosmetic — but it
  is why "it builds with `just fmt` and not in the devShell" would be possible. Prefer one
  `pkgs` binding for the file (§2.1).
- **`allowUnfree = true` is not deprecated** at this lock (checked the pinned nixpkgs:
  `pkgs/stdenv/generic/check-meta.nix` still reads `config.allowUnfree ||
  NIXPKGS_ALLOW_UNFREE`), so it is not a warning to chase. It *is* maximally wide: six
  copies, no `allowUnfreePredicate`/`permittedLicenses` narrowing, so "which unfree
  packages are in this fleet" is answerable only by building it.
- **`sopsSecretText` decrypting at build time** is not a bug in `common/sops.nix` — sops-nix
  exposes no decrypted contents, and the file says so. §4 is about *which* secrets use it,
  not about the mechanism.
- **`common/nas.nix` generating `.automount` units explicitly** and `x-systemd.automount`
  still appearing in `shareOptions`: both correct (the option is dead in unit-file mounts
  and the header says so, loudly). Do not "clean up" either half.
- **`machines/gw`'s assertion module and `just check`'s standalone eval are the same
  assertions twice**: intentional (`machines/gw/default.nix:60-70`) — one makes a deploy
  fail, the other prints which record is wrong.
- **`nix flake check --no-build` needing a full checkout** (it reads
  `machines/pi-room-sys/ssh_host_ed25519_key` through a path literal) is a symptom of the
  open item about that private key, not a flake bug.
- **`security.sudo.wheelNeedsPassword = false` + `mitigations=off`**: a decided tradeoff,
  written once in `common/base.nix` with the reason.

---

## Appendix A — what was run (all read-only; no switch, no build)

Run 2026-10-22 at `f923c6e`. Per `AGENTS.md` rule 22 all nix commands were run on aibox,
never on gw, and nothing was built or activated.

```bash
git ls-files '*.nix' | wc -l                                   # 112
grep -c 'import nixpkgs' flake.nix                              # 9
grep -rn 'builtins.readFile (config.sopsSecretText' --include='*.nix' . | wc -l   # 16
grep -rn 'builtins.readFile (sopsSecretText' --include='*.nix' . | wc -l           # 1 (the helper)
grep -rho 'int\.leighhack\.org' --include='*.nix' . | wc -l     # 152
grep -rhoE 'import [^ ]*/(config|hosts)\.nix' --include='*.nix' . | wc -l          # 40

# the gate, on aibox
nix flake check --no-build; echo $?                             # 1 — sops-secret-pg_pass.drv
nix path-info /nix/store/zicnr1hgclx6vmnrhh17w30sd2h4f8dz-sops-secret-pg_pass.drv  # on services1: valid
nix fmt -- --check $(git ls-files '*.nix')                      # exit 0, 112 files clean
for f in machines/services1/lib/check-config-to-gitlab.nix common/check-printer-states.nix \
         machines/gw/lib/check-network-tables.nix; do nix eval --impure --raw -f "$f"; done  # ok ×3

# warnings, per machine
for m in services1 services2 aibox pi-room-sys gw; do
  nix eval --apply "w: builtins.concatStringsSep \"\n\" w" ".#nixosConfigurations.$m.config.warnings"
done

# doc pointers checked against the tree / `just --summary`
just --summary | tr ' ' '\n' | sort -u
for f in AGENTS.md README.md justfile; do grep -ohE '`just [a-z0-9-]+`' "$f"; done \
  | tr -d '`' | cut -d' ' -f2 | sort -u | grep -vxFf <(just --summary | tr ' ' '\n')
  # → dns-sync-check, dns-sync-prune, dns-sync-sync, update-netboot-input (past tense)
for f in AGENTS.md README.md justfile; do
  grep -ohE '(common|machines|crates|docs|secrets)/[A-Za-z0-9._/-]+\.(nix|md|yaml|sample)' "$f"
done | sort -u | while read -r p; do [ -e "$p" ] || echo "missing: $p"; done
grep -rhoE '(common|machines)/[A-Za-z0-9._/-]+\.nix' --include='*.nix' . | sort -u \
  | while read -r p; do [ -e "$p" ] || echo "missing: $p"; done   # → machines/constants.nix

# tmpfiles `z` semantics (in /tmp, on gw; no system paths touched)
printf 'z /tmp/zt-test 0640 - - -\nf /tmp/zt-f 0640 - - -\n' >/tmp/r.conf
systemd-tmpfiles --create /tmp/r.conf; ls /tmp/zt-test          # No such file (the f line was created)

# secret exposure, paths and modes only (rule 13: contents were not read)
systemctl show -p ExecStart --value nginx-sso.service | grep -oE '\-c [^ ]+' | cut -d' ' -f2 | xargs ls -l
# -> -r--r--r-- 4 root root 1440 /nix/store/5mg4p1ai32bg6gk4b26jqz8k8cvyxlfb-nginx-sso-config
```

## Appendix B — what was already open elsewhere

Cross-references so this doc does not compete with the backlog: the eval-time secret read
and the `--no-build` failure, the store-warm-ness of `just check`, `nix flake check`
building the devShell, the pi-room-sys/aibox drv churn, `LOCAL_DOMAIN` not being used by
the files that spell the name out, the private host key in `machines/pi-room-sys/`, the
Grafana plaintext-password warning and the `stdenv.is*` deprecations are all in
`docs/follow-ups.md`. The nginx-sso and `nix.maxJobs` items in §3 and §4 are **not**, and
should be copied there if these findings are accepted.
