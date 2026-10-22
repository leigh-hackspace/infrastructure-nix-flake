# Authentik in a container on services1

The IdP has been run out-of-band on its own VM — `authentik` at **10.3.1.36**,
docker compose in `/srv/authentik`, nothing in the flake — since before this repo
described anything. This document is the other half: the same authentik as a
podman stack on services1, fed a copy of that VM's database, so the two can be
run side by side and the move is a config change instead of an outage.

**Status: built, verified, parked — and 10.3.1.36 is the IdP.** The module is
**not imported** (`./default.nix`), so services1 currently runs none of this: no
containers, no `id-shadow.int` vhost. While it *was* up it ran a copy of the
appliance's database and was checked against the live appliance (see "What was
verified"), and it was switched off on 2026-10-15 with the cutover left for a
later session. Throughout, `id.int.leighhack.org` resolved to services1 and
nginx proxied it to `10.3.1.36:9000` (`machines/services1/http.nix`); every
client on the network authenticates against the appliance and nothing here was
switched over.

**To re-enable:** uncomment `./authentik.nix` in
`machines/services1/services/default.nix`, `just switch`, then
`sudo authentik-migrate all --force` — the state under `/srv/authentik`
survives parking (bind mounts, not volumes) but the copy goes stale, and
`restore` refuses to overwrite a non-empty database without `--force`.

Parking removed the containers (the generated units' `ExecStopPost` is
`podman rm -f`, and the bind mounts are host directories, so nothing under
`/srv/authentik` went with them) and the `id-shadow.int` vhost, and it also
cleared the working copies: the dumps and `outpost-tokens.txt` under
`/srv/authentik/migration` were deleted, since both are re-creatable and one of
them held two live outpost credentials. `database/` (424 MB) and `media/` were
left alone — wipe them with `sudo rm -rf /srv/authentik/database` if the next
session should start from nothing.

- Config: `machines/services1/services/authentik.nix`
- Migration tool: `machines/services1/services/authentik-migrate.sh`, installed
  as `authentik-migrate`
- The OIDC contract, provider pks and the three invisible authentik bites:
  `docs/authentik-oidc-notes.md`
- RADIUS clients (the switches): `docs/procurve-switches.md`

## The two halves

| | appliance (live) | services1 (shadow) |
| --- | --- | --- |
| host | `authentik` VM, 10.3.1.36 | 10.3.1.20 |
| runtime | docker compose, `/srv/authentik` | podman `oci-containers`, `/srv/authentik` |
| web | `0.0.0.0:9000` | `127.0.0.1:9000` only, nginx in front |
| name | reached via nginx's `proxyPass` | `id-shadow.int.leighhack.org` (LAN ACL) |
| database | `authentik-postgresql-1` (postgres:16-alpine) | `authentik-postgres` (same image) |
| redis | `authentik-redis-1` (redis:alpine = 7.2.2) | `authentik-redis` (redis:7.2-alpine) |
| worker | running | **not started at boot** (see `shadow`) |
| outposts | `Radius Outpost`, `LDAP Outpost`, created by authentik over the docker socket | declared in nix, **not enabled** |
| secrets | `/srv/authentik/.env` | sops: `authentik_env_file` → `/run/secrets/authentik_env_file` |

`shadow = true` in the module is the whole difference: it picks the shadow
vhost, keeps the worker off at boot, and keeps the nightly dump off a timer.
The worker is the half that runs authentik's scheduler, sends mail and drives
outposts, so a second one running against a copy of the real data would be a
second copy of every scheduled job.

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
4. **`authentik.int.leighhack.org`.** aibox's sssd connects to
   `ldaps://authentik.int.leighhack.org` (machines/aibox/sso.nix) and that name
   is answered at the appliance, 10.3.1.36, on purpose: it is `exempt` in
   `common/int-dns.nix` (which is how "deliberately answered elsewhere, unlike the
   front door" is written down since `dns-sync` went; docs/router-and-dns.md). It has
   to move to 10.3.1.20 with the LDAP outpost, or aibox's `ldap_uri` changes. `id.int`
   and `ldap.int` already point at services1, so HTTP needs no DNS work at all.
5. **Flip `shadow = false`** and deploy. That moves the vhost from
   `id-shadow.int.leighhack.org` onto `id.int.leighhack.org` +
   `id.leighhack.org` + `authentik.int.leighhack.org` and turns the worker and
   the dump timer on. **Delete the `proxyPass = "http://10.3.1.36:9000"` vhost
   from `machines/services1/http.nix` in the same change** — two vhosts for the
   same name is an nginx eval error, which is the safe way to fail.
6. **Take a final dump after the appliance is stopped** and restore it, so the
   copy includes everything that happened while the shadow was being built.
   Steps 5 and 6 in either order; 6 after 5 means one restart of the server.
7. Delete `id-shadow` from `common/int-dns.nix` once the name is no longer wanted, and
   deploy gw. (There is no `prune` any more: the record is the table, so removing a name
   is removing a line and switching the router.)

Rollback before the appliance is stopped: set `shadow = true` again and switch.
Rollback after: point the vhost back at 10.3.1.36, start the appliance, and
`pg_restore` the final dump onto the appliance's postgres 16 if anything was
changed in the shadow's database in the meantime — which is why the pins are
pins.

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

