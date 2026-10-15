# Follow-ups — the live backlog

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps a short list of the ones
that an agent can trip over or fix in passing; this file keeps each item with the
context that makes it actionable. **Add an item here (with a date and what blocks
it) rather than growing `AGENTS.md`** — that is what keeps the rules file a rules
file.

Narrative of work already finished: `docs/work-log-2026-10.md`.

---

## Nix / build

- **`crates/target/` is not excluded from `workspaceSrc` (open, one line).**
  `common/crane.nix` filters the four SPA directories out of `workspaceSrc` and
  nothing else, and `lib.cleanSourceFilter` does not read `.gitignore`, so an
  evaluation that reaches `crates/` as a **path literal** copies 3.1 GB of cargo
  artifacts into the store while holding the nix big lock — which is what wedged the
  box on 2026-10-14 (mechanics: `docs/nix-dev-workflow.md`). Harmless through the
  flake, since `self` arrives git-filtered, so this is hardening rather than a
  fault: adding `target` to that filter makes the mistake impossible to repeat.

- **The `deploy` account is dead weight and should go.** `useradd`ed on 2026-10-11
  as a workaround for the build-user bug (build as a uid that owns nothing), with
  its own copy of the repo and the private flake inputs under `/home/deploy`. The
  bug is fixed (`common/nixbld.nix`, deployed 2026-10-12), so nothing needs it, and
  it is not declarative — a switch will not recreate it. Remove it
  (`userdel -r deploy`) unless something turns out to depend on it. Do **not**
  deploy through it: builds as `leigh-admin` are safe and that path is the one
  `just switch` takes.

- **services2 has no swap partition** (services1 has 18 GB). Fine at the current
  load, worth matching before it runs anything heavier than builds.

## Machines / access

- **services2 cannot hop out.** `machines/services2/README.md` used to claim the
  shared machine-hop key was installed there; it is not. Only `authorized_keys` is
  (by `common/users.nix`), so you can hop *in* to 10.3.1.21 but it cannot ssh to
  services1, and it has no GitHub key of its own. Fetch there with `ssh -A` (agent
  forwarding — `AGENTS.md`, "SSH between machines") rather than copying a private
  key around. Install `~/.ssh/agent-hop-key` deliberately if it should hop out;
  never make it a side effect of a deploy.

- **`network-status.service` runs as `User=leigh-admin`** — it should have its own
  user. During the 2026-10-11 build-user incident that is what turned the uid-wide
  kill into a *service* outage (SIGKILLed and restarted seven times) rather than
  just a dead terminal; the sweep can no longer reach uid 1234, but a LAN-facing
  dashboard running as the admin is still its own problem. **Blocked on** the hop
  key the unit needs (`~/.ssh/agent-hop-key` to reach the router): a dedicated user
  needs that key in sops, or a dedicated router key.

## DNS / monitoring

- **Two public status vhosts have no DNS record** (found 2026-10-12 while
  sanity-checking after the `nixbld` fix).
  `machines/services1/services/status.nix` defines the SSO vhosts
  `status.leighhack.org` and `aibox.status.leighhack.org`, and
  `common/monitoring-endpoints.nix` monitors `https://status.leighhack.org`, but
  both names are **NXDOMAIN** in the DigitalOcean zone — so they are unreachable and
  that gatus check cannot be passing. The `*.int` dashboards are fine
  (`services1-status.int` / `services2-status.int` / `aibox-status.int` /
  `network-info.int` all serve 200). dns-sync only manages the `*.int` CNAMEs;
  public names are added by hand in DO DNS and these two never were. Either add the
  records (public apps CNAME → `nginx.leighhack.org`) or drop the vhosts and the
  endpoint.

- **Nothing monitors the borg server** (`nas2.int:3022`), so a dead `borg-server`
  container only shows up as a failed nightly job — and one nightly run has already
  failed with no recorded reason. Details: `docs/nas-and-borg.md`.

## Postgres

- **The application databases still record collation version 2.42** while the box
  runs 2.44, so each still logs `database "…" has a collation version mismatch` on
  connect. `template1`/`postgres` were refreshed on 2026-10-13, which is what
  unblocked `CREATE DATABASE` (the full story, including why `template0` refuses, is
  in `docs/work-log-2026-10.md`). Refreshing an application database is a one-liner
  each — `ALTER DATABASE <name> REFRESH COLLATION VERSION;` — but it is a *statement
  about their indexes*, so it wants a decision per database rather than a loop. The
  strict alternative is `REINDEX` of the collated indexes.

## Switches / RADIUS

- **Rotate the RADIUS shared secret** — it was printed into a session transcript on
  2026-10-12. Both ends: the switch (`radius-server key`) and the authentik RADIUS
  outpost. `docs/procurve-switches.md`.
- **Fix the authentik `agent` password** so RADIUS stops refusing every switch
  login; until then every GUI refresh writes two `login_failed` events and
  `login_failed` alerting is permanently noisy. `docs/procurve-switches.md`.
  Related tidy: `RadiusProvider` pk 7 has `authentication_flow = None` while the
  flow actually used is `radius-authentication-flow`.
- **`switch-gui` still needs its secret committed on services1.**
  `secrets/secrets.yaml` carries the `switch_gui_env_file` key (4 keys:
  `SWITCH_GUI_LOGIN`, `SWITCH_GUI_PASSWORD`, `SWITCH_GUI_OIDC_CLIENT_ID`,
  `SWITCH_GUI_OIDC_CLIENT_SECRET`) but the change is **written and uncommitted**,
  and services1/services2 get `git reset` to `origin/main` periodically.
- **`switch-gui`: the VLAN-management commands have never been sent** (create,
  rename, delete, the per-VLAN `tagged` endpoint).
- **`switch1.int` / `switch2.int` / `switch3.int` are not dnsmasq records**, so the
  gatus "Network Switches" pings use the addresses from `common/hosts.nix`. Add the
  names to the router's host overrides if they are wanted, or drop the idea and keep
  the IP pings.

## Print servers

- **`3d-blue` still carries the cloned SD identity** (machine-id and SSH host keys);
  lime was regenerated on 2026-10-03. Harmless while blue is off-network, but do it
  if blue is ever re-imaged. The `hackspace` password and `authorized_keys` are
  shared by the clone — rotate them if the boxes should not be interchangeable.
  `docs/printer-servers.md`.

## Authentik

- **`RadiusProvider` / `groups` ScopeMapping tidy.** The `switch-gui` provider's
  `groups` scope reuses gocardless-dashboard's ScopeMapping row rather than owning
  one, because creating a fresh child row through the ORM trips a unique-violation
  on `propertymapping.name`. Fine for now; the proper fix is the web UI or the real
  API. `docs/authentik-oidc-notes.md`.

- **The authentik cutover is not done — and the shadow is now parked.** The
  container stack was deployed, migrated from the live database and verified
  against the appliance (`docs/authentik-container-migration.md`), then switched
  off on 2026-10-15 by un-importing `machines/services1/services/authentik.nix`;
  10.3.1.36 is the IdP. **Step nought is uncommenting that import and
  `sudo authentik-migrate all --force`** (the state under `/srv/authentik` is
  still there but stale). Then the four things the checklist names: the outpost
  containers (needs the two `AUTHENTIK_TOKEN` sops secrets, then
  `enableOutposts = true`), the RADIUS clients that name 10.3.1.36 (switch NVRAM
  — ask first), `authentik.int`'s router override to 10.3.1.36 (aibox's sssd),
  and flipping `shadow = false` while deleting the `proxyPass` vhost from
  `machines/services1/http.nix` in the same change.

- **Rotate the appliance's postgres password (2026-10-15).** The first 40
  characters of `PG_PASS` and of `AUTHENTIK_SECRET_KEY` reached a transcript
  through a verification command whose redaction did not work. The database
  password is cheap: it is only read by the appliance's own compose file, so
  change `PG_PASS` in `/srv/authentik/.env`, `docker compose down && up -d`.
  `AUTHENTIK_SECRET_KEY` is not cheap — authentik cannot re-encrypt what it has
  already encrypted, so rotating it means regenerating every stored provider key
  and bind password. Both are now also in `secrets/secrets.yaml`
  (`authentik_env_file`), which is where they should have been.

- **`id-shadow.int.leighhack.org` has no DNS record.** The vhost works and was
  tested with `curl --resolve`; `sudo dns-sync sync` adds the record, and
  `just dns-sync prune` should remove it when the shadow goes away.

- **The nightly authentik dump timer is off** while `shadow = true` (a dump of a
  copy of the live database would only confuse a restore). The unit itself is
  tested; it turns on with the flag. `machines/services1/services/authentik.nix`.

