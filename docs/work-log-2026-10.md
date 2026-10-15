# Work log — October 2026

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` is rules; this is the record of
what was changed and what the changes proved, kept because the *lessons* are still
load-bearing and because "why is it shaped like this" is the first question a new
change runs into.

Still-open items live in `docs/follow-ups.md`.

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
