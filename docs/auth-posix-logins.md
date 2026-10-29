# POSIX identities and logins: what the authentik LDAP layer really is, and what to replace

Deep dive of 2026-10-29, prompted by "the auth was hacked together by someone with no
understanding of LDAP (me) — rewrite it if you can make it better, perhaps by not using LDAP
at all".  Everything below was measured on the running fleet, not read out of a manual; the
parts that could not be measured are labelled as unresolved, and two claims made in this
repo earlier the same day turned out to be wrong and are corrected in §6.

Read `common/auth-posix.nix`'s header first if you only want the rules; this file is the
reasoning, the evidence and the plan.

---

## 1. What the layer is, in one map

```
  authentik (services1, podman) ── LDAP outpost ──ldaps://authentik.int.leighhack.org:636──┐
    106 users, 11 groups, no uid/gid columns, empty redis                                    │
                                                                                             ▼
                                     sssd  ── nss, pam  ──  nscd  ── getent / id / login     aibox
                                     ▲                                                     pi-room-sys
             /etc/sssd/sssd.conf ────┘   (both now rendered by common/auth-posix.nix)        (netboot)
                ▲
                └── one shared bind account, cn=pgina, whose secret is in the nix store

  App logins (Grafana, filestore, switch-gui, gocardless-dashboard, nginx-sso) are a
  separate, and separate-good, story: OIDC + the `groups` scope, no LDAP anywhere.
  Switch logins are RADIUS, and authenticate nobody (docs/procurve-switches.md).
```

Two machines are LDAP clients: `aibox` and the Pi Room netboot image.  Everything else that
says "authentik" on this network talks OIDC to the front door.  So "rewrite the auth system"
in practice means **these two boxes' identity and login path**, and the OIDC half can be left
alone — it works, it is documented, and it has its own traps already written down
(`docs/authentik-oidc-notes.md`).

## 2. Findings

### F1 — Every uid is `2000 + authentik's Postgres primary key`, and that number owns files

Checked against the whole live directory: `uidNumber == 2000 + authentik_core_user.id` for
**105/105** users LDAP serves.  The number is not stored: `authentik_core_user` has no uid
column, and **0 of 106** users have a `uidNumber` attribute.  The provider declares
`uid_start_number = 2000`; the outpost adds the pk per query.

It is load-bearing.  From the NAS that the netboot image mounts as `/home`:

```
10.3.1.6:/mnt/sas-10k/ds-home  →  drwx------ 21 2013 2013  cjdell
10.3.1.6:/mnt/sas-10k/ds-home  →  drwx------ 17 2099 2099  hackspacer
10.3.1.6:/mnt/sas-10k/ds-home  →  drwx------ 24 2101 2101  kevin.gorman.14
```

and cjdell's authentik pk is 13.  So a home directory's confidentiality (`0700`, so ownership
*is* the boundary) rests on a sequence value surviving every future database operation.

Failure modes, in ascending order of how much they worry me:

1. **Delete and re-create a user** → new pk → new uid.  Their `/home/<name>` reappears owned
   by a stranger's number; they get a fresh empty home; the old one is unreadable to them and
   is `0700` so nobody else can clean it up either.  This is reachable *by accident*:
   `crates/gocardless-dashboard`'s sync **creates authentik users** as part of its nightly
   membership run, so accounts do get made and removed by a timer.
2. **Restore the database without preserving pks** — a per-object API import, a partial
   restore, a `--no-owner` replay into a fresh cluster — re-keys every home directory at once.
   The 2026-10-28 IdP migration survived this **only because it was a full `pg_dump` restore**
   (see `docs/authentik-container-migration.md`); nothing in the design would have caught it,
   and nothing would catch it next time.
3. **A future authentik version changes the formula.**  `uid_start_number` is an ordinary
   provider field, editable in the UI by anyone who can reach `/if/`.  Change it from 2000 to
   something else and every uid in the fleet moves, silently, on the next lookup.

### F2 — Group gids are also computed, stored nowhere, and I could not work out what they are

No column, no attribute, and `redis-cli dbsize` is **0** — so nothing persists them.  They are
not `gid_start_number + n` (declared 4000; served 15295…76456), and they are not any simple
hash of the group's name or uuid: crc32, adler32, md5, sha1, sha256, blake2b over seven
spellings of each input, across 17 moduli, reproduces none of the 11 known pairs.  Whatever it
is, it is **recomputed from fields we do not control**.  That is the finding — not the formula.
A rename, a delete-and-recreate, or an outpost upgrade that changes the derivation re-keys
every group-owned file, and no database would notice.

### F3 — Nested groups are unobtainable in both directions, and 105 users have a group that does not exist

`Members` lists `Directors`, `Infra` and `Trusted` as members, and `Public` lists `Members`.
Neither path delivers that to a Unix group:

* sssd's nested walk fails outright (`Error processing direct membership [22]`, "return
  cached data"), and
* the outpost's `memberOf` is **not transitive** — `id bobm6tma` → `(Members)`, and
  `memberOf: cn=Members` only, although `Members ⊂ Public`.

So `Public` reaches nobody who is not a direct member of it.  Measured before and after sssd
config changes, and confirmed against the group's own `member` list.

Separately: **all 105 users have a `gidNumber` with no group object behind it** (each user's
primary gid is their own uid).  That is what sssd is logging about, it means every process has
a primary group with no name and no members, and it makes group-based sharing impossible —
`id Catryan` prints `gid=2019` with no name next to it.

### F4 — The bind secret is public by construction, and here is exactly what it buys

sssd needs `ldap_default_authtok` in its config file; the netboot image's config file is in a
squashfs that anyone who can plug in an Ethernet cable mounts and reads.  The value is
additionally in git.  What that credential can do, measured:

* read **all 105 users' `cn`, `displayName`, `mail` and `memberOf`** — i.e. the whole
  hackspace member list with email addresses, which is personal data, not just a username
  list;
* **not** read `attributes`, so the door-entry card UIDs and GoCardless customer ids are *not*
  exposed (this was my worry; it is fine — the outpost serves a fixed attribute set);
* **not** authenticate anybody: empty password → `Invalid credentials (49)`, wrong → `(49)`,
  anonymous → `Insufficient access (50)`.  The outpost's own access control is correct.

So the exposure is a directory/PII leak and a membership oracle, not an auth bypass.  That is
also why "just make it read-only" is not the fix: the fix is for the client to not need a
member's credential at all (§3).

### F5 — Why this is hard to test, and what I broke while testing it

Three instruments lied to me, and each one is worth writing down because they all point the
same way — *a green login is not evidence of a live directory*:

* **`nscd` is active in front of sssd** on both boxes, and sssd has its own sysdb.  A client
  whose `ldap_uri` was set to `no-such-host.invalid` went on answering `getent passwd` for a
  user never looked up in that boot.  On a netboot client both caches are tmpfs, which is what
  made me say earlier today that every lookup there must be live; **it is not true**, because
  nscd and sssd's cache both survive a config change within one boot.
* sssd's cache is **`/var/lib/sss/{db,mc}`**, not `/var/lib/sssd/` (which exists and holds
  config).  My first "cache destroyed" test destroyed nothing.
* **`ldapwhoami` cannot probe this outpost**: it does not implement the Who-Am-I extended
  operation, so even a *successful* bind answers `Protocol error (2)`.  Use `ldapsearch`.

And two honest self-corrections from this session: a `sed` anchored at column 0 read an empty
bind token and sent **two empty-password binds as `pgina`** (account verified still
`is_active`), and I wrote `pkill -f` patterns that matched my own shell.  Both are in
`docs/follow-ups.md` as traps, because the first one is one keystroke from locking every LDAP
client in the fleet out, and the second killed a tool call.

### F6 — The config itself had already drifted

Two hand-written sssd domains and two copies of the root CA PEM, one per client machine,
differing in `cache_credentials` (`false` on aibox, `true` on a box whose cache is tmpfs), in
`debug_level` (8 vs 6), and in whether the config was generated by `toINI` or pasted as a
string.  Third occurrence of that pattern in this repo, hence rule 2.  Now `common/auth-posix.nix`,
with the CA at `common/pki/leighhack-root-ca.pem`; the rendered result was diffed against both
previous configs (36/38 settings identical; the deltas are the three documented in the
module's header).

## 3. What I would build, and why "no LDAP at all" is only half on offer

The tempting plan — drop sssd, do everything with OIDC — does not work for these two boxes,
and it is worth being blunt about why before proposing anything: **an SSH/console login is not
an OIDC flow.**  Nothing in the stack can turn a bearer token into "uid 2013 owns
`/home/cjdell`", and the POSIX numbers in §F1 are already written into every file on the NAS.
So the uid/gid mapping is not removable; the only question is whether it is *declared* or
*guessed at query time*.  It currently is guessed.

The design I would move to, in three independent pieces, each of which is an improvement on
its own and none of which requires the others to be finished first:

### P1 — Pin the numbers (small, high value, do first)

Make uid/gid **stored facts in authentik** rather than derived ones: a one-time reconciliation
that writes each user's current `uidNumber` (and each group's `gidNumber`) into
`attributes`, matching exactly what is on disk today.  The rule for the tool that does it is
the one that makes it safe: **it may only ever write a number that no file depends on, or the
number the files already use.**  It must refuse, loudly, to renumber a user whose uid appears
in `ds-home` or `filestore`, and it should read those mounts to check.  House shape:
`crates/authentik-posix` with `--audit` first (report-only: dump every uid/gid, cross-check
against the NAS, print what a restore would break), `--pin` later behind a flag.
Precedent for the API side: `crates/gocardless-dashboard/src/authentik.rs` (paginated,
bearer-token, `groups_obj`, `attributes`).

Until that exists, the cheap mitigation is a guard, not a fix: a `just` recipe / timer that
dumps `username→uid` and alerts on drift from the previous run.  That would have caught F1
existing at all.

### P2 — Take the secret off the netboot client (this is the real "no LDAP" win)

A diskless client cannot hold a secret, so stop asking it to.  Serve the *identity* data the
way aibox already serves everything else the client needs:

```
aibox: timer → read authentik API → /srv/netboot/posix.{passwd,groups}  (served by the
       existing aibox.int vhost, alongside /boot/bzImage and the squashfs store)
image: activation fetches both, renders /etc/passwd + /etc/group deltas, no sssd at all
```

Why this is not a new hole: the client already mounts aibox's squashfs as its own root
filesystem over NFS and executes it.  It trusts aibox completely today; fetching identity data
from the same host over the same network adds no new trust and removes a credential from every
image ever built.  It also removes a boot-time dependency (a Pi Room box can no longer fail to
list its users because the IdP is restarting), and it makes the uid map an **auditable
artefact** — the file is right there in the build output.

What P2 does *not* remove is password verification, and there is a neat property to lean on
here: **a bind as `cn=<user>,…` with that user's own password needs no service credential
anywhere** — that is precisely what the `pgina` bind already is.  So a netboot client can keep
verifying member passwords over LDAPS while holding no secret, because the credentials in the
transaction are the user's.  The mechanism to use is a small per-user bind (a `pam_exec`
helper in Rust, or `pam_ldap` configured with no `binddn`), not sssd's search-then-bind.

Open question to settle before building: authentik's LDAP provider has `bind_mode = direct`
and `mfa_support = false`.  A per-user bind therefore **bypasses authentik's MFA and flows
entirely** — a member with TOTP on the web UI is password-only on a Pi Room console, and LDAP
logins appear in the event log as ordinary `login` events.  If that is not acceptable, P2's
authentication half should be SSH-certificate-only (§P3) and the console should be a kiosk.

### P3 — Replace the long-lived key story for admin access

Today the netboot image ships `PermitRootLogin = "yes"`, a **private host key committed to the
repo** (`machines/pi-room-sys/ssh_host_ed25519_key`, already a known open item), two shared
root `authorizedKeys`, and an initrd SSH daemon with `emergencyAccess = true`.  A diskless
client has no reason to have a fixed host key at all — generate one per boot into tmpfs — and
admin login does not need a personal long-lived key either.  The robust shape is what the rest
of the industry does: a short-lived **SSH certificate** from a fleet CA, issued on proof of an
authentik session (the OIDC device flow, which the fleet already speaks), carrying the groups
as principals, and `AuthorizedKeysCommand`-free.  That deletes the committed private key, the
shared authorizedKeys and the "who is uid 2019" question from the SSH path, and it composes
with P1/P2 rather than depending on them.

### Order, with the gate on each step

| # | Step | Gate — what has to be true before the next one |
|---|---|---|
| 0 | (done, deployed to aibox 2026-10-29 11:59) one module, one CA, documented deltas | rendered config diffed against both previous configs (36/38 settings identical, 3 documented deltas); aibox switched, confirmed good, and the netboot image re-checked in QEMU against the deployed store — see the ladder note below for the export gotcha |
| 1 | `authentik-posix --audit` | it runs clean, and its report is reviewed by whoever owns the NAS |
| 2 | pin uid/gid as attributes, one account, verify LDAP still serves identical numbers, then all | `--audit` shows zero change for every user; `just pxe-client-uefi` still logs in |
| 3 | POSIX snapshot served by aibox; netboot image fetches it | image boots with nscd *stopped* and sssd *disabled* and still resolves a real member |
| 4 | per-user bind for console auth (or kiosk + P3 only) | a real member logs in at a Pi Room console once — see the open item on `pam_exec` |
| 5 | SSH CA, remove the committed host key, `PermitRootLogin prohibit-password` | admin can ssh in with a cert and cannot with a bare key |

## 4. What I would not do

* **Do not "just use SSSD's `access_provider` group gate" yet.**  It is the right destination
  and today it is a gate on *direct* membership only (F3), evaluated by a daemon whose cache
  makes its own behaviour unobservable with the instruments in §F5.  `accessGroup` is an option
  on the module, defaulting off; turning it on needs a working test first.
* **Do not disable nesting to silence the log noise.**  Measured: `ldap_group_nesting_level = 0`
  does not reduce the errors (9 cold-cache failures became 18).  The failures are in direct
  membership processing, not in a depth limit.
* **Do not "fix" the `#`-suffix secret-from-file convention into the config.**  sssd documents
  reading a secret from a file that way and I could **not** prove it works here — every test was
  defeated by the caches in §F5, and one test that looked like a pass was sssd serving stale
  data.  Proving it requires: stop `nscd`, wipe `/var/lib/sss/{db,mc}`, restart sssd and *check
  the pid changed*, look up a never-queried user, and read the outpost's own log on services1
  for the bind.  Until then it does not go in the auth path of two machines.
* **Do not experiment with deliberately-wrong binds against `pgina`** — every LDAP client in
  the fleet authenticates as it, and authentik does record failures.
* **Do not renumber anything to "clean this up".**  Uid 2013 is not a detail; it is the ACL on
  a person's home directory.

## 5. Robustness work that is not architectural

* **A QEMU netboot test can silently test the previous image.** After the 2026-10-29 deploy of
  this page's P0 step, aibox looked entirely healthy — switch retried and confirmed, no failed
  units, nginx serving the new menu and the new kernel/initrd — while
  `/exports/netboot-squashfs` still bind-mounted the *old* generation's squashfs, because the
  activation's restart of that mount failed (see the `exports-netboot\x2dsquashfs.mount` item in
  `docs/follow-ups.md`, where a boot fetch succeeds against a stale store and nothing errors).
  **Before believing a netboot test, compare** `findmnt -no SOURCE /exports/netboot-squashfs`
  **against** `nix eval
  .#nixosConfigurations.pi-room-sys.config.system.build.squashfsStore.outPath`, **and prove
  which image booted from inside it** — read a config value the change was supposed to alter
  (the deployed image was confirmed by `ldap_id_use_start_tls` being absent and
  `cache_credentials=false` being present, neither true of the previous one).

* The netboot image still has `pam_exec` mounting `//10.3.1.6/filestore` with the
  `hackspacer` credentials **in the module text** (visible to anyone who can read the config,
  as with F4), and its success has never been observed — the open item in
  `docs/follow-ups.md`.  Either make it a secret and prove it runs, or delete it.
* `PermitRootLogin = "yes"` plus a committed host key on a diskless image (§P3).
* nscd in front of sssd is a *cache of identity decisions* with no TTL discussion anywhere in
  this repo.  If it is there for speed, say why; if it is there because an old NixOS default
  put it there, turning it off makes half of this page's measurement problems disappear.
* `id` succeeding is treated as proof of health in several docs and one monitoring check.  It
  is not (§F5).  The health check for this subsystem is a bound-and-searched probe to the
  outpost, which is what `just check`-style evals should do — not `getent`.

## 6. Status of this page (2026-10-29)

**P0 is deployed.**  aibox runs the shared module's generation (`nixos-system-aibox-…bb0305p…`,
confirmed good) and the Pi Room image built from it boots and authenticates: verified in QEMU
against the deployed store — new kernel/initrd/menu fetched, NFS store mounted, `display-manager`
up, no failed units, `sssd` active with the three intended deltas present *inside the image*, and
the LDAPS path proven from inside the client with a positive (a live search resolving
`Catryan` → `21159(Infra)` with `reqcert = demand`) plus a real negative control (the same
search against a single unrelated-but-valid CA returns nothing, so the trust decision is made on
file *content* and our CA is necessarily in the bundle the module supplies).

Two instrument failures during that verification, recorded because they will be repeated:
**`openssl` is not installed on the netboot image**, so three `openssl … | grep` probes of the CA
bundle were silently no-ops (a `0` that meant nothing) — the positive/negative pair above is the
real test; and **`getent -s sss` cannot be trusted as a liveness probe**, only as a
module-selection hint, for the caching reasons in F5.

The deploy also surfaced a hazard that had nothing to do with this change and had been
misdiagnosed for a day: see the `exports-netboot\x2dsquashfs.mount` item in
`docs/follow-ups.md` and §5 above.  P1–P3 remain open and P1 is the one to do next.

## 7. Corrections to what this repo said before this dive

* `docs/follow-ups.md` (2026-10-29, earlier) claimed the nested-group failure was "noise only
  because authentik flattens membership".  **Wrong, and the script written to prove it is what
  disproved it** — see F3.  The item now records the correction.
* `machines/pi-room-sys/README.md` (same day) implied a netboot client's tmpfs cache means its
  lookups are live. **Wrong** — nscd sits in front (F5).  Corrected.
* Also corrected there: "nothing has been observed on the hardware".  Both boards' boots are in
  aibox's nginx log, attributable by EUI-64, including one 25 minutes after the IdP cutover.
