# Authentik in a container on services1

**Status since 2026-10-28 19:12Z: this box is the IdP.** `authentik` at **10.3.1.36**
— docker compose in `/srv/authentik`, nothing in the flake — was retired by moving
*that address* onto services1, and everything that used it followed the address: the
switches' `radius-server host 10.3.1.36`, `ldaps://authentik.int.leighhack.org`, and
`common/dns-records.nix`. The VM is stopped in Proxmox's HA (`vm:108`), its containers
are `stopped` rather than removed, and its postgres was left running until the last dump
was taken and restored. Nothing in the flake was pointed at a new address.

This document is still the story of how it got here, and it is written against the
appliance being live for most of its length — that half is history now, and where the
two disagree the cutover section below is the fact.

**The four things that are easy to get wrong now, with their receipts, are in
"The cutover, and what it taught" at the end.**

- Config: `machines/services1/services/authentik.nix` (`staged` is the gate: `true` runs
  the stack without claiming the network — that is how the copy was verified — and the
  cutover commit set it to `false`)
- Migration tool: `machines/services1/services/authentik-migrate.sh`, installed
  as `authentik-migrate`
- The OIDC contract, provider pks and the three invisible authentik bites:
  `docs/authentik-oidc-notes.md`
- RADIUS clients (the switches): `docs/procurve-switches.md`

## The cutover, and what it taught (2026-10-28)

Done in one session, `staged` deploy first, four minutes of IdP-down, no client
reconfigured. Measured outage on the gatus `Authentik` endpoint: **19:08:21Z 502,
19:11:21Z 502, 200 from 19:12:21Z** — appliance stopped at 19:08:19Z, services1 switched
~19:09, `gw` switched ~19:11. Budget was an hour.

The sequence, which is worth keeping as the shape of any future "move a stateful service
onto a flake machine without touching its clients" job:

1. `just switch` with `staged = true` — containers up, **no vhost, no outposts, no
   worker, no timer**, and no claim on the address. The staging check needs no DNS name:
   `curl -H 'Host: id.int.leighhack.org' http://127.0.0.1:9000/…` reaches the app with
   neither DNS nor the door involved, which is why `id-shadow.int` was retired instead of
   re-added to `common/int-dns.nix` and `common/front-door.nix`. (It had been
   unresolvable since 2026-10-24 anyway, and the module still evaluated because a parked
   module is never evaluated.)
2. `authentik-migrate check` → the same server image ID on both sides (`2c9dd0d4db11`),
   postgres 16.9 vs 16.15 (same major, the one warning).
3. `dump` → `restore --force` → `files --force` → `verify`: table-for-table parity,
   including `authentik_core_user 106` against the appliance's 106.
4. **The check that actually mattered** — the doc's claim that "what could have broken
   silently is the provider config: the `redirect_uris` JSON array, the signing key, the
   scopes, the flow assignments", tested by firing *one* real authorize request at each
   instance and comparing the `Location`. Re-run on 2026-10-28 against the Grafana
   provider (strict `matching_mode`, S256 PKCE) with the same challenge on both sides:
   **byte-identical 302s** into `/if/flow/default-authentication-flow/`. Build the request
   from the migrated DB rather than by hand — in 2026.8 `redirect_uris` is a JSON array of
   objects, so it is `jsonb_array_elements(...) ->> 'url'`, and `jsonb_array_elements_text`
   hands you the whole object and looks like a valid redirect URI until the IdP refuses it.
5. Outpost credentials: `authentik-migrate outpost-tokens` → sops
   (`authentik_radius_outpost_env`, `authentik_ldap_outpost_env`), each a one-line
   `AUTHENTIK_TOKEN=`. Both hashes matched the appliance's running containers, which is
   the check that says "the tokens came across", not "a token was written".
6. Final `dump` with the appliance still live → stop the appliance's serving side
   (`docker compose stop server worker` + `docker stop ak-outpost-*`, postgres and redis
   deliberately left up so the last re-dump is one command) → `restore --force` the final
   dump → `just switch` services1 → `just deploy gw`.
7. Verify, then stop the VM: `ha-manager set vm:108 --state stopped` (takes a few seconds
   to take effect, so poll `qm status 108`).

### 1. The outposts answer `10.3.1.36` because conntrack says so

`10.3.1.36` is now a **second address on `vlan225`**, after `10.3.1.20` so `.20` stays
the primary and stays the source address of everything else this box does. The outposts
are podman published ports, netavark installs them as `NETAVARK-HOSTPORT-DNAT`, and
conntrack therefore replies **as the address that was dialled**. That is the entire
reason a switch with a connected UDP socket gets its Access-Challenge: an answer sourced
from `.20` is dropped in silence and reads as an auth timeout. So the outpost ports are
`0.0.0.0` on purpose — and the corollary is a testing trap: **probe RADIUS from a
different host**. A locally-generated probe on services1 is a confusing experiment, while
from aibox the outpost logs `Matched CIDR instance=switches ip=10.3.1.32`, which is the
real path a switch takes.

### 2. `AUTHENTIK_HOST` must be a URL, and a healthy unit means nothing

The first cutover deploy had both outposts `active`, both ports published, and nothing
bound: the env said `authentik-server:9000`, the outpost SDK parses that as a **scheme**,
and the containers sat in `unsupported protocol scheme "authentik-server"` retries.
`systemctl is-active` was green throughout. Fix: `http://authentik-server:9000`.

The value the appliance actually used is worth knowing, and is recorded because it was
not guessable: the outpost row in the DB (`authentik_outposts_outpost._config`) says
`authentik_host = "https://id.int.leighhack.org/"`, `authentik_host_browser = ""`,
`authentik_host_insecure = false`. So **the switches' RADIUS reached authentik's API out
through the front door and back in** — RADIUS depended on the door being up. Naming the
container is both shorter and truer. Read the outpost's log, not the unit state.

### 3. The LDAPS certificate is a database row, so aibox and the pi-room clients survived

`authentik.int.leighhack.org`'s serving certificate lives in
`authentik_crypto_certificatekeypair` (fingerprint sha256 `6E:B0:69:1B…F1:58`, expires
2035-11-15) and **migrates with the dump**; the appliance's `/srv/authentik/certs` was
empty. Verified after the move by comparing the live leaf on `.36:636` with the row —
identical. That is what makes aibox's `ldap_tls_reqcert = demand` keep working, and
`id Catryan` on aibox returning `groups=…,21159(Infra)` over `ldaps://authentik.int` is
the proof: the real client, the real name, the real certificate enforcement.

The consequence for whoever adds a certificate mount here: the appliance also kept
`./certs:/certs` in its compose file, so a `certs` volume with a *different* key/cert
pair would be worse than useless — aibox and the netbooted pi-room clients would fail
their CA check and the symptom would look like a broken IdP.

### 4. Dead config the read-back caught (open)

The module's `createCommand` block (`--memory 1G --cpus 2` on postgres and the server)
is **not applied** by this nixpkgs' `virtualisation.containers`: the rendered start script
is `podman run --replace …` with no resource flags and no `--network authentik`, so the
stack rides the default `podman` network (name resolution works; that is why nobody
noticed). Rule 10 — read back the rendered unit — is what caught it, and the fix is not
mechanical: adding the network back means recreating the whole stack together, because a
container cannot join a second network while its peers stay on the old one and the
outposts would lose `authentik-server`. Open in `docs/follow-ups.md`.

### What the cutover did *not* fix, and two things it proved were already broken

- **Switch logins still do not authenticate via RADIUS.** Nothing about the move changed
  that, and the row-level reason is now on record: there is **no `agent` user in
  authentik** (the usernames look like `Catryan`, `DougAddams`), so the identification
  stage fails, the flow — written for interactive prompting — recurses, and the outpost
  gives up with `exceeded stage recursion depth` and **sends no reply at all**. That is
  the appliance's behaviour reproduced exactly (same DB, same flow, same image ID), and
  it means the switches time out rather than being rejected. Fixing RADIUS means creating
  the account the switches actually send and setting `radius-server key` — NVRAM, so not
  today: `docs/follow-ups.md`.
- **aibox's sssd was already failing before the window**: 13 × `Unable to establish
  connection` between 2026-10-27 and 19:00 today, the last at 18:19:36. After a restart it
  resolves identities again, so the pre-existing GSSAPI/`krb5_validate` story in
  `machines/aibox/sso.nix` is still the open item, not the certificate.
- **`sss_userdk` does not exist in authentik any more** (queried: no matching username),
  so the end-to-end test `machines/aibox/sso.nix` recommends is a dead canary. Use
  `id <real-member>` instead.
- **The appliance VM must stay stopped.** Its MAC still has a live DHCP reservation
  (`common/dns-records.nix`'s `authentik` record — the shape the network-table guards
  accept for an address that belongs to a flake machine), so a booted VM takes `.36` back
  and two hosts answering one address is silent for whoever is logging in. Deleting the
  VM or the record needs the `exempt` question settled in the same change, because
  `authentik.int` resolves *because* that record exists.
- **The LDAPS/RADIUS address is in the operator's `known_hosts`** (`10.3.1.36`, three
  lines). Those keys belong to the retired VM and can never validate again; ssh to `.36`
  now reaches services1.


## The two halves, as they stood before the cutover

Kept because it is the comparison the migration was designed around. Two rows are now
false in the interesting direction: services1 has the vhost (`id.int.leighhack.org` as an
origin behind the door, not `id-shadow.int`) and the outposts, and the appliance has
nothing running.

| | appliance (retired 2026-10-28) | services1 (the IdP) |
| --- | --- | --- |
| host | `authentik` VM, 10.3.1.36 | 10.3.1.20, plus `.36` as a second address |
| runtime | docker compose, `/srv/authentik` | podman `oci-containers`, `/srv/authentik` |
| web | `0.0.0.0:9000` | `127.0.0.1:9000` only, nginx in front |
| name | reached via nginx's `proxyPass` | `id.int.leighhack.org`, an `origin` row in `common/front-door.nix` |
| database | `authentik-postgresql-1` (postgres:16-alpine) | `authentik-postgres` (same image) |
| redis | `authentik-redis-1` (redis:alpine = 7.2.2) | `authentik-redis` (redis:7.2-alpine) |
| worker | running | running (`staged = false`) |
| outposts | `Radius Outpost`, `LDAP Outpost`, created by authentik over the docker socket | declared in nix, **running** |
| secrets | `/srv/authentik/.env` | sops: `authentik_env_file` → `/run/secrets/authentik_env_file` |

`staged` (that is what `shadow` was renamed to on 2026-10-28 — it stopped being a shadow
the day the appliance stopped answering) was the whole difference while staging: it picked
the vhost, kept the worker off at boot, and kept the nightly dump off a timer. The worker
is the half that runs authentik's scheduler, sends mail and drives outposts, so a second
one running against a copy of the real data would be a second copy of every scheduled job.

One deliberate difference rather than a shadowing detail: the appliance
publishes its **postgres on `0.0.0.0:5432`**, so any host on the LAN can try the
IdP's database. The shadow publishes nothing — postgres and redis are reachable
only over the podman network, by container name. Do not copy the appliance's
compose file back over this without noticing the difference.

## Why the pieces are shaped like this

**Its own postgres container, not the host cluster.** The appliance is on
postgres 16 and services1 is on 17. Same-major means the dump can be restored
in *either* direction, which is the rollback, and it keeps authentik's
connection pool off the cluster that outline, gocardless-dashboard, switch-gui
and the door system share. It also sidesteps the `template1` collation-version
mismatch (docs/work-log-2026-10.md): the dump is created and restored inside
the same image.

The appliance's `postgres:16-alpine` is 16.9 and a pull today is 16.15, so the
tags matching is not the same thing as the versions matching. `check` reports
both and warns; same *major* is what the rollback needs, and 16.9 ← 16.15 is
that.

**Version pins are not upgrades.** `authentikImage` is the tag the appliance
runs, verified by image ID (`2c9dd0d4db11…` on both sides as this was written).
A database restored into a newer authentik is migrated on first boot and cannot
be taken back, so the two move together or not at all. `check` compares the
image IDs and refuses before `restore` can do the damage.

**No docker socket, so no managed outposts.** The appliance's worker runs as
root with `/var/run/docker.sock` mounted so authentik can create its own outpost
containers. Handing a container the socket that controls every other container
on the box is a bigger grant than the feature is worth, and podman's
docker-compatible API is not what authentik's docker controller was written
against. The outposts are therefore declared as units instead.

**`--env-file`, not `environment`.** The secret file is handed to podman, so no
systemd unit and no generated config ever contains the signing key. But podman's
env-file parser is *not* systemd's: it keeps quotes and does not expand
variables, so `FOO="bar"` arrives as `"bar"` (tested against this box's podman
5.8.8). The shared `env_file` secret quotes its values for systemd;
`authentik_env_file` must not. `authentik-migrate secrets` checks for that
specific mistake.

**uid 1000.** The server image's own `USER` is 1000 and it writes uploads under
`/media`, so the activation script owns `media`, `custom-templates` and `certs`
to that uid. 1000 is unassigned on services1 (leigh-admin is 1234) and is given
a name (`users.users.authentik`) so nothing else can claim it.

**The nightly dump is a custom-format `pg_dump`, not `postgresqlBackup`.** That
module only knows the host cluster. It writes to
`/mnt/backups/services1.int.leighhack.org/authentik/`, two weeks deep, and is
off until the shadow becomes real — a dump of a copy of a copy would only
confuse a restore.

## Running the migration

All of it is read-only against the appliance; the one write is `pg_dump`, which
takes a consistent snapshot and does not block readers. Run as root on services1.

```bash
authentik-migrate check          # images match, secrets sane, space, connectivity
authentik-migrate all --force    # check, dump, restore, files, verify
```

Or step by step, which is what you want the first time:

```bash
authentik-migrate check
authentik-migrate dump              # -> /srv/authentik/migration/authentik-<utc>.dump
authentik-migrate restore --force   # stops the app units, replaces the DB, starts them
authentik-migrate files --force     # media/, custom-templates/, certs/ (rsync --delete)
authentik-migrate verify            # row counts, appliance vs this box
```

`restore` refuses a target that already has tables and `files` refuses a
non-empty one, because `--delete` and `DROP DATABASE` are in play — and on any
box where the stack has booted once the target *is* non-empty (first boot
leaves a stock migrated database behind). So `--force` is normal here, not an
eyebrow-raiser: the shadow is disposable and `all --force` rebuilds it in half
a minute.
Dumps and the extracted outpost tokens are `0600` under
`/srv/authentik/migration` (mode 700) and the script never prints a secret —
`AUTHENTIK_SECRET_KEY` and the database passwords are compared by truncated
sha256 and nothing else.

Then look at it: `https://id-shadow.int.leighhack.org` (after `id-shadow` is in
`common/int-dns.nix` and gw has been deployed — the record is generated from that table
now that `crates/dns-sync` is deleted), log in as `cjdell`, and check the
four providers in `docs/authentik-oidc-notes.md`'s table are all still there.

## Cutover

The appliance stays live until someone decides otherwise. What has to happen,
in the order that minimises the time the network has no IdP:

1. **Decide what happens to 10.3.1.36.** The VM also holds the only copy of the
   pre-migration database, so stop it rather than delete it, and keep it
   stoppable for a rollback.
2. **Outposts.** `authentik-migrate outpost-tokens` writes the two config tokens
   out of the migrated database to a 0600 file; put them in the
   `authentik_radius_outpost_env` / `authentik_ldap_outpost_env` sops secrets as
   `AUTHENTIK_TOKEN=<token>`, set `enableOutposts = true`, deploy, and only then
   stop the appliance — two containers registering the same outpost token
   against two authentik instances is a split brain on the switch logins.
3. **RADIUS clients.** The switches and APs name the appliance's *address*
   (`radius-server host 10.3.1.36`, docs/procurve-switches.md), so they have to
   be rewritten to 10.3.1.20 — which means writing switch NVRAM, which is the
   one thing nobody does without being asked. The alternative is a router-level
   redirect of 1812/udp from 10.3.1.36 to 10.3.1.20 and leaving the switches
   alone.

   **What was done on 2026-10-28:** neither. A third option, cheaper than both and the
   reason this section is here — move `10.3.1.36` itself onto services1 as a second
   address, so the NVRAM string and the DNS answer are both still true of a box that is in
   the flake. No router rule, no switch touched, no `dns-records` edit. The consequence is
   documented in "The cutover, and what it taught" above (and it is why the outposts' ports
   are published the way they are).
4. **`authentik.int.leighhack.org`.** aibox's sssd connects to
   `ldaps://authentik.int.leighhack.org` (machines/aibox/sso.nix) and that name
   is answered at the appliance, 10.3.1.36, on purpose: it is `exempt` in
   `common/int-dns.nix` (which is how "deliberately answered elsewhere, unlike the
   front door" is written down since `dns-sync` went; docs/router-and-dns.md). It has
   to move to 10.3.1.20 with the LDAP outpost, or aibox's `ldap_uri` changes. `id.int`
   and `ldap.int` already point at services1, so HTTP needs no DNS work at all.

   **What was done:** nothing moved, which is the point of step 3 — the name still answers
   `.36`, and `.36` is now this box. `exempt` stays, with its prose corrected to say that
   the exemption is about LDAPS/RADIUS ports rather than about which box is the IdP. (The
   plan's assumption that aibox's `ldap_uri` would need editing was wrong in an
   uncomfortable direction: editing it would have broken the netbooted pi-room clients too,
   which read the same name from a different module and were not in the checklist at all.)
5. **Flip `shadow = false`** and deploy. That moves the vhost from
   `id-shadow.int.leighhack.org` onto `id.int.leighhack.org` +
   `id.leighhack.org` + `authentik.int.leighhack.org` and turns the worker and
   the dump timer on. **Delete the `proxyPass = "http://10.3.1.36:9000"` vhost
   from `machines/services1/http.nix` in the same change** — two vhosts for the
   same name is an nginx eval error, which is the safe way to fail.

   **What was done:** `staged = false`, and step 5's second half is obsolete —
   `machines/services1/http.nix` has had no `id.int` vhost since the front door landed on
   2026-10-24; the proxy lives in `common/front-door.nix`, so the change to make there is
   `upstream = "http://10.3.1.36:9000"` → `"origin"`, and it goes in the same commit as the
   flag because the origin/DNS guards on both boxes refuse anything else. `id-shadow.int`
   was deleted from `common/int-dns.nix` rather than revived: a staged copy needs no name
   (loopback + `Host:` reaches it) and an unserved name in that table is a drift magnet.
6. **Take a final dump after the appliance is stopped** and restore it, so the
   copy includes everything that happened while the shadow was being built.
   Steps 5 and 6 in either order; 6 after 5 means one restart of the server.

   **What was done:** the final dump was taken *before* stopping the serving side (so
   nothing could be lost in the gap), the serving side was stopped with postgres and redis
   deliberately left up, the final dump restored, then services1 and then gw switched.
   Measured IdP-down: about four minutes, 19:08:19Z → 200 at 19:12:21Z on the gatus
   endpoint.
7. Delete `id-shadow` from `common/int-dns.nix` once the name is no longer wanted, and
   deploy gw. (There is no `prune` any more: the record is the table, so removing a name
   is removing a line and switching the router.) — done 2026-10-28, in the same gw deploy.

**Rollback, as it now stands.** There is no instant one, and that is worth being blunt
about, because the older version of this paragraph promised `docker compose up -d`. The VM
is stopped and `10.3.1.36` belongs to services1. To go back: `git revert` the cutover
commit and switch **services1** first (that releases `.36`), then start `vm:108` on
monster1 with `ha-manager set vm:108 --state started`, then `docker compose up -d` in
`/srv/authentik` (its containers were `stop`ped, not removed; the two `ak-outpost-*`
containers need an explicit `docker start` — the compose file never names them, they were
created by the worker over the docker socket), then revert the `id.int` row and switch
**gw**. If anything was written to this box's database in the meantime — and after a day
it will have been: sessions, events, last-login — `pg_restore` the relevant dump onto the
appliance's postgres 16 first, which is the reason the image pins are pins. Do **not**
start the VM while services1 still holds the address: that is two hosts answering `.36`,
and it fails silently for whoever is logging in rather than loudly.

## What was verified (2026-10-15)

Deployed on services1 with `shadow = true`; the appliance was never stopped,
restarted or reconfigured during any of this and is still what the network
authenticates against.

- **First boot works.** postgres, redis and server come up on their own; the
  server's `ak migrate create-superuser` creates the default instance and the
  web app serves `/` → `/setup`.
- **The migration runs.** `authentik-migrate dump` took 4 s over the network
  (28 MB custom-format, verified by listing the archive through the target
  container); `restore` replaced the database in 19 s, stopping and restarting
  the app units around itself; `files` rsynced `media/` (1.2 MB) and the two
  empty template/cert trees in 2 s.
- **`verify` is clean:** 104 users, 11 groups, 29 applications, 23 providers,
  13 tokens, 21 flows, 3 outposts, 776 migrations — identical on both sides,
  tenant `base_url` `https://id.int.leighhack.org`. The database *sizes*
  differ (371 MB vs 209 MB): a fresh restore has none of the appliance's
  accumulated bloat. That is the one number that will never match.
- **OIDC works on the copy.** `authentik-migrate check` passes; the Grafana
  provider's discovery document is served, and an `/application/o/authorize/`
  request with the real client_id, redirect_uri and an S256 challenge 302s into
  `default-authentication-flow` with **byte-identical parameters to the same
  request against the live appliance**. So provider lookup, redirect-URI
  validation, PKCE and flow assignment all survived the copy.
- **The vhost works** at `id-shadow.int.leighhack.org` over the wildcard cert
  with the LAN ACL, generating URLs on its own host (the tenant fallback).
  Tested with `curl --interface 10.3.1.20 --resolve id-shadow.int.leighhack.org:443:10.3.1.20`,
  because **the DNS record did not exist then** (it is a table entry now;
  `common/int-dns.nix`).
- **The backup path works.** `systemctl start authentik-db-dump` wrote a 28 MB
  dump to `/mnt/backups/services1.int.leighhack.org/authentik/` at 0600
  root:backups. The test artifact was deleted; the timer stays off in shadow
  mode.
- **Outpost credentials are extractable** — `authentik-migrate outpost-tokens`
  writes the two of them (LDAP, Radius) to a 0600 file. They are *not* in the
  outpost row's `_config` jsonb: each outpost authenticates with an API token
  named `ak-outpost-<outpost uuid>-api` in `authentik_core_token`.
- **No error-level logging** from the shadow server. The three outpost rows are
  there and nothing connects to them, which is the expected state until the
  nix-defined outposts are enabled.

Four bugs in the first version of the script were found by running it, not by
reading it, and are worth remembering when writing the next one against this
appliance: `psql -tA "$sql"` silently ignores the query (it needs `-c`);
`systemctl --ignore` is **ambiguous** in systemd 261 (say
`--ignore-dependencies`); `docker inspect -f '{{.Image}}'` prints `sha256:<hex>`
where podman prints the bare hex, so an image-ID comparison has to normalise
both; and `_config` is `jsonb`, so `coalesce(_config,'')` is a type error.

## Secrets handling, and one thing to rotate

`AUTHENTIK_SECRET_KEY` had to be copied verbatim — it is what decrypts the
secrets stored *in* the database (provider keys, LDAP bind passwords) — so it
now exists in `secrets/secrets.yaml` as part of `authentik_env_file` and at
`/run/secrets/authentik_env_file` on services1. The appliance's database
password was deliberately **not** copied: `authentik-migrate` uses a fresh one,
because the target is a new cluster and nothing outside the box needs the old
value.

During the work a transcript showed the first 40 characters of
`AUTHENTIK_SECRET_KEY` and of the appliance's `PG_PASS` (a verification command
whose redaction did not work). Treat both as disclosed to anyone with access to
that session. The appliance's database password should be rotated at a
convenient time — it is only used by the appliance's own compose file, so
`docker-compose down`, change `PG_PASS`, `docker compose up -d`, done. Rotating
`AUTHENTIK_SECRET_KEY` is a different matter: authentik cannot re-encrypt what
it has already encrypted, so it means regenerating every stored provider key
and bind password by hand. Do not rotate it casually; do consider that the key
has been sitting in a world-readable `/srv/authentik/.env` on a VM since 2023.

