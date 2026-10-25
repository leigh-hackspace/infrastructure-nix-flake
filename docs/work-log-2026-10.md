# Work log — October 2026

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` is rules; this is the record of
what was changed and what the changes proved, kept because the *lessons* are still
load-bearing and because "why is it shaped like this" is the first question a new
change runs into.

Still-open items live in `docs/follow-ups.md`.

## 2026-10-25 — a camera firmware update was almost run on a guess, and the risk was the settings

**Asked:** update one OpenIPC camera, guarantee WiFi and SSH come back, and see whether an
SD card is an unbrick. **Done:** nothing was written to any device; the work stopped at
measurement, and the measurement is now four recipes and an audit —
**`docs/camera-firmware-audit-2026-10-25.md`**.

**What turned out to be true.** The boot chain is *not* what an update threatens: the
current NOR image is kernel + rootfs only, and `sysupgrade` writes neither IPL nor U-Boot
nor env. What it threatens is the settings, because a split NOR layout has no fixed slots —
`rootfs_data` starts where the new squashfs ends, so a rootfs that changes size **moves the
partition the WiFi credentials and the ssh keys live in**, and they cross the gap as a blob
whose failure message is *"the camera starts from defaults"*. Upstream's own comment records
that an empty blob was staged by mistake on **every NOR upgrade since the hand-off (#2648)**.
Separately: whether the current build fits the 8 MB chip at all is decided by ~109 KB, and
**this repo has never recorded a single partition size for any of the three cameras**; the
guard that would catch it is documented as failing *open*, and `-f` is "do not validate
anything". And the safety net everyone expects is not there: OpenIPC's `failsafe` (network +
SSH after repeated boot failures) is gk7205-only, so a crash-looping ssc33x is a bench visit
behind a shield nobody has opened.

**Lesson worth keeping:** the numbers that made this look settled were **invented** — by the
agent, in the repo's own ✅/❓ house format, and into three files — and survived until someone
re-grepped them (`grep` the tree, then `git log -S` the history: no source). A plausible
number in the right format reads better than "unmeasured" and passes review; that is the
failure mode of automating a documentation style this precise, and the strike list in the
audit's §8 exists so the next reader does not inherit them as citations. **A camera number
that does not name the command that produced it is not a fact about the camera.**

## 2026-10-22 — `*.int` is declared, not synchronised; and gw stopped being a shadow

**What was wrong.** The network's answer for `*.int.leighhack.org` lived in a mutable
file on the router (`/var/lib/dnsmasq/names.hosts`, 183 lines) that **no deployment could
produce**. `crates/dns-sync` edited it over ssh from services1, behind
`just dns-sync-sync`, when a human remembered. Two things followed. First, correctness
depended on a habit: a vhost deployed without the command was live, TLS-served and
unresolvable, and the only thing that would notice was the same tool's `check`, run by
hand. Second, the file had accreted: 46 of its lines were device names duplicating
`common/dns-records.nix` (restored verbatim from the retired OPNsense box on 2026-10-21),
3 were switches in no table at all, and the guard rail that kept `authentik.int` answering
at the appliance rather than at the front door was a case in a Rust `match`, not a fact in
the config. A rebuild of gw from this flake would have answered a *different* zone —
which the 2026-10-21 cutover had just demonstrated by going NXDOMAIN for the whole zone.

**What it looks like now.** Three generated store paths, one table per fact:
`common/vlans.nix` + `common/dns-records.nix` → `network.conf` +
`dnsmasq-records.hosts`, and the new **`common/int-dns.nix`** → `dnsmasq-int-vhosts.hosts`.
Every one is in dnsmasq's `restartTriggers`, because dnsmasq reads `addn-hosts` **at
startup** and nixpkgs only restarts it for `/etc/hosts` — the same trap that bit the
records file on 2026-10-17, and now the reason a record and its restart cannot be
separated. `common/int-dns.nix` has four lists, because "the front door answers it" is not
the only true thing a name can do: `names` (vhosts → services1's three addresses, read
from `common/dns-records.nix`, never copied), `fromHosts` (switches: named by
`common/hosts.nix`, not DHCP clients), `legacy` (five names nothing serves today that
resolved anyway, each kept deliberately with its reason), and `exempt` (`authentik.int`,
served by nginx, answered by the appliance because RADIUS and LDAP dial it directly).

**The guard is an evaluation, not a command.** `machines/services1/int-dns.nix` derives the
`*.int` names from `services.nginx.virtualHosts` — via the same
`common/nginx/int-vhost-names.nix` the gatus guard uses — and refuses to build services1
unless `names ∪ exempt` equals that set. It earned its place on its first run: it failed
`just check-build` over `mqtt`/`nginx`/`tailscale`, which I had put in `names` because they
*resolve* today when nothing *serves* them. `machines/gw/lib/check-network-tables.nix` got
the mirror-image rules, and they are what stops one name being answered from two tables
(dnsmasq unions every `addn-hosts` file, so that is a name with two addresses, not a
conflict anyone sees).

**What 10.3.1.20 has to do with it.** Everything, and it is not special: it is services1,
whose nginx holds every `*.int` `server_name` in the network and proxies to whichever box
actually runs the app. A `*.int` name is a pointer to a vhost, every vhost is on services1,
so the answer to "what is special about 10.3.1.20" is "it is the front door, and it has
three addresses" — IPv4, public IPv6, ULA — which is why a record is one line per address
and why the check counts the lines per name (a `*.int` name without its AAAA is a
dual-stack client hanging before it falls back, per `machines/gw/README.md`).

**Measured, not assumed.** The deploy was made provably answer-neutral before it was
allowed to touch the router: the set of `(address, name)` pairs the running router serves
(252, both hosts files, bare aliases included) was diffed against the set the generated
files render — **zero added, zero lost**. `dig`ed all 89 names plus seven controls
(lease-only names like `artemius.int`, `gw.int`, one nonexistent name) before and after
`just switch-gw`: **byte-identical**. The rendered `network.conf` differs from the previous
generation's by four blank lines. Along the way three claims in the docs turned out to be
stale and are fixed: **there is a `dig` on gw** (bind is in its systemPackages; there is
still none on services1), **`gw.int` has always been answered from `/etc/hosts`** —
dnsmasq reads that file too, which is the fourth source of answers and the reason
`networking.extraHosts` on the router is a network-wide act while services1's `extraHosts`
(`unifi.int`) is invisible to everyone else — and the "10 unsynced vhosts" I nearly
recorded as the audit's finding were grep noise: all 34 vhosts the deployed nginx serves
did have records.

**Also: co-existence mode is deleted.** `machines/gw/coexist.nix` (which held `.254` on
every VLAN while the appliance lived), its `hostAddr` substitution, its `commentBlock`
helper and the `!COEXIST.enable` branches in `dns.nix`/`networking.nix`/`firewall.nix`/
`uplink.nix`/`sops.nix` are gone; gw is the router, permanently, and nothing in the tree
can put it back into being a shadow. Its four switched-off behaviours survive as the four
checks near the top of `machines/gw/README.md`, because "a router that is advertising
nothing" and "a router that cannot advertise" look identical from `systemctl`.

**Left, on purpose:** `names.hosts` still on disk and still created by tmpfiles — an older
generation names it in `addn-hosts`, dnsmasq **exits** on a missing hosts file, and
`system.autoRollback` can install that generation for five minutes; deleting the file and
the rule together is a later deploy (`docs/follow-ups.md`). The DigitalOcean zone's ~40
`*.int` CNAMEs are now maintained by nobody (one is fixed: `gatus.int` was created with an
FQDN name, so it published `gatus.int.leighhack.org.leighhack.org` and resolved nowhere);
and a `*.int` wildcard record there is a *maybe*, blocked on someone proving ACME renewal
survives it.
---

## 2026-10-17 — strata parked on aibox, and aibox becomes the network's builder

The ask was "disable strata on aibox and make aibox the new builder". The two are
one change: the IQ2_XS expert arena is ~33 GiB of RAM, and a builder needs RAM more
than a second copy of the engine needs to exist. services2 keeps the Coder engine;
it is now the only live Strata instance in the network.

What moved:

- `machines/aibox/default.nix`: the `./strata.nix` import is commented out and
  `services.strata.enable = true` is removed (parked, not deleted — the module,
  `STRATA.md`, the tuning scripts and the `.#strata` package stay), and
  `infra.buildFarm.serveBuilds = true` is added.
- `machines/services1/default.nix`: `builderHost` goes from `HOSTS.services2` to
  `HOSTS.aibox`, and `maxJobs` is set to 6 — aibox is a Ryzen 5 6600H (6c/12t),
  where the default 8 was services2's core count. `common/build-farm.nix` now
  defaults to 6 and its header/options name aibox.
- `machines/services2/default.nix`: `serveBuilds = true` is removed. It still
  builds locally; it no longer accepts other boxes' work.
- The comments and the live docs that named services2 as the builder or aibox as a
  Strata host (`AGENTS.md`, `docs/services.md`, `docs/nix-dev-workflow.md`, the
  justfile, both `STRATA.md`s, `machines/services2/README.md`) now name the box
  that is true today. `docs/network-upgrade-runbook.md`'s order/rationale changed
  with it: the builder is second (aibox), not last, and the witness role the
  services2-last rule was built on is gone (gatus moved to services1 earlier the
  same day).

Why aibox: 59 GiB of RAM (services2: 30), 6 cores (services1: 4), and its apps are
leaf services that services1 proxies, so a build storm degrades frigate/immich/
whisper rather than the box every public request goes through. services2 was the
right builder only while its RAM was free; once the RTX 3060 arrived that RAM was
the engine's, and the 2026-10-15 upgrade measured a full build pushing it to 0 MiB
free and 6 GiB of swap.

Two consequences stated so the next reader does not discover them:

- With the strata module gone, `llama-server` gets its default `wantedBy` back
  (`machines/aibox/ai.nix`), because the displacement was strata's. It starts at
  boot again; stop it before a long build if it has a model resident.
- Disabling the service does not delete the model data: the GGUFs under
  `~/Models/Qwen3.8-Flash-Next-GSQ-RCO-GGUF` and the pack/MTP under `~/Strata`.
  aibox's root measured 82 % full on 2026-10-15, and the builder is the box that
  grows the store. Deleting those paths is a manual decision (open in
  `docs/follow-ups.md`), not something a switch does.

Not yet verified: the delegated hop itself after the change. The key, `sshUser`
and `trusted-users` wiring is box-agnostic and aibox has the hop key, but the last
proof (both a delegated build and a `--max-jobs 0` build) was against services2 on
2026-10-12. Run `just build-remote` once after switching aibox, and read the
builder name in the log rather than assuming.

Deploy order for this change is **aibox → services1 → services2**, not the
runbook's default: aibox has to be accepting builds before services1's new
`nix.conf` starts asking it to, and services2's switch is what removes the old
`serveBuilds` (it can be last, and nothing delegates to it meanwhile). Chain
`sudo nixos-confirm` with each switch per the golden rule.

---

## 2026-10-17 — the observability stack moved from services2 to services1

The ask was "move everything off services2 except strata".  What was on services2
was gatus, Prometheus, Grafana, its dashboards and its own status dashboard; what
stayed is strata, `status.nix`, the exporters and the build farm.

**Why the move, in the direction that surprised me:** the file that owned the
arrangement argued the opposite of the ask — "the box that answers *is this thing
down* must not be the box being asked", which is why gatus was on services2 at
all.  The reason it stopped holding is in `machines/services2/STRATA.md` and
`docs/work-log-2026-10.md`: the Coder engine measured **28.3 GiB RSS against
services2's 30 GiB of RAM**, journald was logging "Under memory pressure,
flushing caches" with the monitor living alongside it, and the OOM victim does not
have to be the engine — so the thing that was meant to survive a failure was the
thing being reaped during one.  services1 has the headroom (96 GB free on its root
at the time of the move, against services2's 121 GB) and already terminates TLS
for these vhosts, so the apps moved to it and the vhosts stayed where they were.

What moved, and the shape it left behind:

- `machines/services2/gatus.nix` → `machines/services1/services/gatus.nix`
- `machines/services2/monitoring.nix` → `machines/services1/services/observability.nix`
  (named for what it is, so it cannot be confused with the vhost file that was
  already called `monitoring.nix` one directory up)
- `machines/services2/monitoring-dashboards.nix` →
  `machines/services1/services/monitoring-dashboards.nix`, pulled in by
  `observability.nix` rather than by the services list
- the vhosts (`machines/services1/services/monitoring.nix`) kept their names and
  their ports — 8999 gatus, 3000 Grafana, 9091 Prometheus — and their `proxyPass`
  went from `${HOSTS.services2}:<port>` to `127.0.0.1:<port>`
- Grafana's `http_addr` went `0.0.0.0` → `127.0.0.1`: the proxy is on the same box
  now.  Prometheus and gatus keep their ports and are reached over loopback either
  way, so nothing about them changed except which unit runs them
- `machines/services2/sops.nix` is now empty: Grafana's authentik OIDC client
  secret was its only runtime secret, and services1 already declared the same one
  (`machines/services1/sops.nix`) — one secret, one client, and it has one reader
  now instead of two
- `common/monitoring-endpoints.nix` swapped its `Services1` ping for a `Services2`
  one: gatus now runs on the box it used to ping, so that check is one that can
  only ever catch a hung nginx, while the check of 10.3.1.21 is the one that is
  worth having — and there was no check of services2 before, only of services1
- `common/check-printer-states.nix` reads the two files it asserts over by path,
  so it moved with them; its message strings now name `observability.nix`

**What the move costs, stated so the next reader does not re-derive it:** gatus,
Prometheus, Grafana and services1's own exporters now live on the box they
measure, so a dead services1 goes down with its own monitor and reports nothing.
What still works is the half that keeps being worth having — gatus pinging
services2 / aibox / the router / the NAS / the switches / the printers, and
services1's Prometheus scraping services2's exporters, which is how a strata-driven
death on the box that cannot report itself stays visible.  `services2-status.int.leighhack.org`
is that box's only self-measurement and is the thing that answers while the engine
is thrashing.

Verified after the deploy, on services1: the three vhosts answer over the same
names and ports they did before; the rendered units read back with the loopback
binds and the ports unchanged (`just switch` then `/etc/nginx/nginx.conf` and the
unit files, per the rule about reading rendered output rather than evaluating);
`nix flake check --no-build` stays warning-free at the new lock; the gatus drift
assertion still passes with the new endpoint list; and services2's own switch
removes the three services without touching strata or the status dashboard.

---

## 2026-10-15 — the first full network upgrade, and the four things its runbook got wrong

`docs/network-upgrade-runbook.md` was written the day before it was first executed.
That is the only way to find out what a runbook is worth: **all three boxes are now
on `26.11.20261014.319e4ce`** (nixpkgs `8edc0c72e3a`/2026-10-08 →
`319e4cebef1`/2026-10-14, plus crane, nixos-utils, sops-nix; strata,
nix-software-center and door-entry were offered to the same `nix flake update` and
legitimately did not move; llama-cpp left pinned to the model branch), 0 failed
units, `cur == system-good` on all three, no reboots, endpoint probe identical to
the pre-run baseline, prometheus `count(up)` 12 / `count(up==0)` empty.

**The gate that gated nothing.** `just check-build` was `nixos-rebuild dry-build`.
This box's `nixos-rebuild` is nixos-rebuild-ng, whose own man page says dry-build
"[s]how[s] what store paths would be built or downloaded … but otherwise do
nothing". Measured: three machines, a 786-derivation "these derivations will be
built" list, **exit 0 in 57 seconds, nothing compiled, builder load 0.00**. The
Phase 3 gate — the one that exists because `nix flake check` cannot catch "evaluates
but will not build" — had never caught anything. It is now `nix build` on each
machine's `config.system.build.toplevel`: the same derivation, and unlike
`nixos-rebuild build`/`boot` it writes no boot loader entry, so the next reboot
cannot silently come up on a configuration nobody activated. The real gate then
took **77 minutes** and 2 310 derivations, which is the number the runbook now
quotes instead of "slower than `just check`".

**A confirm is a claim about the running system, so never confirm a failed
switch.** aibox's first switch exited 4 — activation died on
`Failed to restart exports-netboot\x2dsquashfs.mount` — but `/run/current-system`
had already been flipped to the new toplevel and nginx/sshd/immich/strata had all
restarted, and the chained `nixos-confirm` (from the `systemd-run` recipe in
`docs/nix-dev-workflow.md`, which ran it unconditionally) marked that half-activated
generation good. That is the exact state auto-rollback exists to catch, disarmed by
the tool whose job was to arm it. The recipe is now `&&`-style: confirm only on
exit 0. The mount itself is the second finding: `/exports/netboot-squashfs` is a
bind mount **of a store path**, so its source moves on every nixpkgs bump, and the
unit is boot-generated from `/etc/fstab` so a switch can never restart it. The fix
is the `just switch-netboot` dance without the client reboot — `umount -f -l`,
`mount -a`, switch again — and the real fix (a stable export directory holding a
symlink) is in `docs/follow-ups.md`. Nothing was actually broken: no client was
mounting the export, `/nix/store` is served read-only to the whole VLAN, and the
clients pick the new image up on their next boot.

**The deploy depended on state that exists on two of the three boxes.** aibox's
switch died in one second with `repository path '…' is not owned by current user
(libgit2 error code = 7)`: `sudo nixos-rebuild --flake .` opens the checkout as
root, root's HOME is `/root`, and only services1 and services2 have
`safe.directory` in `/root/.gitconfig`. Undeclared, imperative, per-host — and
invisible to every eval this repo has. Phase 0 now checks it alongside `df`.

**Two smaller corrections worth the sentence.** `nix flake check` at the new lock
reaches all five configurations and exits 0: the `sops-secret-….drv is not valid`
error that `AGENTS.md` and two docs call the known-bad baseline **did not
reproduce**, so "the baseline is red" was a fact about the 2026-10-08 lock, not a
law — and the two warnings rule 9 wants gone (Grafana plaintext `secret_key`, and
four `stdenv.is*` deprecations from the CUDA package set behind
`packages.strata-cuda`) are now both visible from one command. And delegation is
lumpier than the docs imply: the fixed-output download derivations are
`preferLocalBuild`, so services1 built 754 of its own 916 while the builder idled
at load 0.0 — and services1's load average reached 37 while every SSO vhost kept
answering in 8-50 ms. **Judge a box by endpoint latency, not by load**, because its
baseline already includes frigate's ffmpeg workers.

Order used: services1 → services2 → aibox, which is *not* the runbook's default
(services1 → aibox → services2). It was benign — Phase 3 had finished, so nothing
was delegated to the builder, and services2 gives the other two boxes nothing but
monitoring — but the reason the default puts the monitor last is that switching it
blinds the run for every box after it, and that is now written down where I will
read it next time. Kernel bumps were patches (6.18.55→6.18.56, aibox
7.2.9→7.2.10) and postgres stayed 17.11, so no reboot and no migration; immich is a
pinned container image, so the mobile-client check did not apply. `jq` and
`python3` were added to `common/tools.nix` mid-run because inspecting a `flake.lock`
or a prometheus response without them costs more tool calls than it saves.

---

## 2026-10-14 — evaluation warnings cleaned up (deployed)

`nix flake check --no-build` is warning-free now, which is what makes it a useful
gate (see the rule in `docs/nix-dev-workflow.md`). Three classes fixed:

- `buildDepsOnly` states `pname`/`version` instead of letting crane guess them from
  the *virtual* workspace manifest (it warned about both on every evaluation).
- `nix.trustedUsers` became `nix.settings.trusted-users`, which renders the
  identical `nix.conf` (`trusted-users = root leigh-admin`, so remote building is
  untouched).
- `filestore.service` and `gocardless-dashboard.service` gained the
  `wants = [ "network-online.target" ]` their `after` had always implied — the house
  pattern every other net-dependent unit already had.

Naming the dependency derivation changes its name, so the first build after it
recompiles the shared workspace deps once; already paid
(`crates-workspace-deps-0.1.0` is in the store).

The same day produced the big-lock lesson (an aborted `nix eval` kept running and
held `/nix/var/nix/db/big-lock`, so every later nix command looked "slow" while
doing nothing) and the path-literal lesson (evaluating through a path literal copies
`crates/target/` into the store). Both are written up in
`docs/nix-dev-workflow.md`; the one-line hardening item is open in
`docs/follow-ups.md`.

## 2026-10-13 — `crates/`, one cargo workspace, and the `--no-auth` bug it exposed

Every Rust crate moved under `crates/` and the native ones became one cargo
workspace (`crates/Cargo.toml`): one `Cargo.lock`, one `target/`,
`cargo --workspace` for test/clippy, and **one shared dependency derivation instead
of eight**. The SPAs stayed separate workspaces and kept their symlinks; the reason
is at the top of `crates/Cargo.toml`.

While moving it the startup plumbing was deduplicated into a new
`crates/common-rs/service`, and **that is where the bug was**: filestore's
`is_loopback` ended in `host.starts_with("127.")`, so `--no-auth` — the escape hatch
whose *only* guard is "this bind is loopback" — accepted `--bind 127.example.com`, a
hostname anyone with a DNS zone can point at a routable interface. Verified against
the deployed binary before the change (accepted, and it went on to try to bind) and
after (refused). switch-gui had already written the careful version, which is the one
now shared: parse the host as an `IpAddr` and ask `is_loopback()`, accept `localhost`
by name and nothing else that will not parse. `common_service::is_loopback` carries
the tests that were scattered across the three callers, including `127.example.com`,
`127.0.0.1.evil.com` and `::1234`.

**The lesson:** an escape hatch guarded by a string comparison of a hostname is not
guarded. Deduplicating the helper into one place is what made the two versions
visible next to each other.

Also shared out at the same time: `common_json::quoted`/`error_object` (four copies
of a JSON string escaper existed, one of them — frigate-monitor's — not escaping
control characters at all) and the `pg` row readers (`text`/`opt_text`/`integer`/
`opt_integer`, duplicated between switch-gui's two DB modules).

Verified at the time of the change: the tree built clean for all three machines
(`nixos-rebuild build` for services1 and aibox, `dry-build` for services2),
`cargo test --workspace` green with no new clippy warnings, and the filestore binary
built from it was run by hand through the loopback gate above.

## 2026-10-13 — services1 could not `CREATE DATABASE`

`sudo -u postgres psql -c "CREATE DATABASE x"` failed with
`ERROR: template database "template1" has a collation version mismatch` — created
under collation version 2.42, the box now runs 2.44 (a glibc upgrade, and Postgres
17 refuses to copy a template whose recorded version differs). Nothing existing
broke because no new database had been asked for since the upgrade; `switch_gui` was
the first.

Fixed by refreshing the recorded version on the **maintenance** databases:
`ALTER DATABASE template1 REFRESH COLLATION VERSION;` (and `postgres`).
`template0` refuses with `invalid collation version change` because it records no
version, and does not need one: `CREATE DATABASE` copies `template1`. Verified by
creating and dropping a probe database.

What the refresh *asserts* is that the on-disk collation still matches the indexes —
for a 2.42 → 2.44 bump almost certainly true but not *proven*; the strict route is
`REINDEX` of the collated indexes. **The application databases still record 2.42**
and still log the mismatch on connect; refreshing them is per-database, so it is an
open item in `docs/follow-ups.md`.

Unrelated but seen in the same window, and **correct behaviour, not a fault**:
`switch-gui.service` can start before `postgresql-setup.service` has created its
role, so its first start exits with `role "switch_gui" does not exist` and the
never-give-up policy brings it up five seconds later.

## 2026-10-13 — switch-gui: shadow copy + write queue + Dioxus SPA, deployed

The page used to read the switches inside the request and keep its cache and audit
log in memory; it now reads a shadow copy that a background sync maintains in
Postgres and writes through a queue in the same database. Design and the three queue
bugs that came out of the first week: **`crates/switch-gui/README.md`**. Deploy
state and what is left: `docs/follow-ups.md`.

What a deployer needs to remember:

1. **It needs the database to start at all.** `switch_gui` (role + DB) is created by
   `machines/services1/services/postgres.nix`; reading all three switches as Manager
   since 2026-10-13 14:18. The DSN is the unix socket, so there is no new secret to
   add and `switch_gui_env_file` keeps its four keys.
2. The old in-memory audit log is gone and there was nothing to migrate; the first
   read of each switch seeds its row, so the pages say "not read yet" until the sync
   has been round (Refresh forces one).
3. `--ttl` is gone (it was the page's cache). `--interval` is the sync's read
   interval and is **60 s** in the unit, not the 30 s default, because every read is
   a login and every login is two authentik `login_failed` events until the RADIUS
   password is fixed (`docs/procurve-switches.md`).
4. authentik has a `switch-gui` OIDC provider (**pk 35**) copied from
   gocardless-dashboard — strict redirect
   `https://switch.int.leighhack.org/auth/callback`, the three OpenID mappings plus a
   `groups` scope, and the same flows/signing key. Verified by a real S256 PKCE
   `/authorize` request: 302 into the auth flow, where a bogus client id gets 400.
   Its `groups` scope reuses gocardless-dashboard's ScopeMapping row (see the
   authentik notes for why).
5. `switch3` answers slowly, so login reads use `SWITCH_GUI_LOGIN_TIMEOUT`
   (default 45 s) rather than the 20 s command timeout — at 20 s the read times out
   before the prompt arrives.

## 2026-10-12 — remote building verified; the `nixbld` fix is what made it safe

With the fix in `common/nixbld.nix` deployed to services1 and services2, a delegated
build and a `--max-jobs 0` build both completed on services2 and copied back to
services1 with no SIGKILL sweep on either box. It is now the default path rather than
opt-in (nix 2.34 prefers an available builder). The incident itself:
`docs/nix-build-sigkill-2026-10-11.md`.

**Lesson worth keeping:** services2 was found running a *newer* generation than its
git tree — its checkout was two commits behind `origin/main` after the 2026-10-11
deploys were done from a dirty tree. **Check `git log -1` on a box before blaming a
config change for its behaviour.**

## 2026-08-26 — aibox NAS mounts (resolved, kept for the pattern)

Commit 1811d93 ("Make aibox startup more reliable") made aibox mirror services1:
explicit automounts + `_netdev` + `wait-for-network` ordering + `wait-for-nas`. The
automount gotcha that made this necessary (unit-file mounts ignore
`x-systemd.automount` in `Options=`) is a standing rule in `AGENTS.md`.

Unchanged since: ~~**evaluating aibox locally requires the `pi-room-sys` git input
to exist at `/home/leigh-admin/Projects/pi-room-sys`.**~~ **Fixed 2026-10-15** —
that tree is `machines/pi-room-sys/` now and `whisper-ws` is `crates/whisper-ws/`,
so the flake has no inputs outside its own checkout and `just check` runs on any
box (which is what services2 needed to be able to act as the build machine for
checks and not only builds).

## 2026-10-13 — borg server upgraded to 1.4.5

Details, the version-coupling constraint and the open backup hazards:
`docs/nas-and-borg.md`.
