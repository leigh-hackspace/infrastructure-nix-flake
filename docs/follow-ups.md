# Follow-ups — the live backlog

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps a short list of the ones
that an agent can trip over or fix in passing; this file keeps each item with the
context that makes it actionable. **Add an item here (with a date and what blocks
it) rather than growing `AGENTS.md`** — that is what keeps the rules file a rules
file.

Narrative of work already finished: `docs/work-log-2026-10.md`.

---

## Nix / build

- **`nix flake check` is not warning-free: services2 prints a Grafana one** (noticed
  2026-10-14 while checking the `gw` machine; unrelated to it). Checking
  `nixosConfigurations.services2` emits `evaluation warning: Grafana passwords will
  be stored as plaintext in the Nix store! Use file provider or an env-var instead.`,
  which by rule 9 is a finding: either the Grafana provisioner moves to the file
  provider / `GF_SECURITY_ADMIN_PASSWORD__FILE`-style env, or the warning is the
  documented exception. `just check` still cannot reach it, but for a different
  reason since 2026-10-15: `nix flake check` now evaluates past `aibox` (its
  inputs are in-tree) and dies on `services1` with the pre-existing
  `error: path '…-sops-secret-<name>.drv' is not valid` that AGENTS.md documents —
  so until that is settled, run `nix flake check --no-build` per machine when you
  need the warnings for one of them.

- **`crates/target/` is not excluded from `workspaceSrc` (open, one line).**
  `common/crane.nix` filters the four SPA directories and `whisper-ws` out of
  `workspaceSrc` and nothing else, and `lib.cleanSourceFilter` does not read
  `.gitignore`, so an evaluation that reaches `crates/` as a **path literal**
  copies 3.1 GB of cargo artifacts into the store while holding the nix big lock —
  which is what wedged the box on 2026-10-14 (mechanics:
  `docs/nix-dev-workflow.md`). Harmless through the flake, since `self` arrives
  git-filtered, so this is hardening rather than a fault: adding `target` to that
  filter makes the mistake impossible to repeat. (`crates/whisper-ws/` is its own
  cargo workspace, so it writes a second `target/` one level down; that one is
  already excluded, as a side effect of `whisper-ws` being a non-member.)

- **The `deploy` account is dead weight and should go.** `useradd`ed on 2026-10-11
  as a workaround for the build-user bug (build as a uid that owns nothing), with
  its own copy of the repo and the private flake inputs under `/home/deploy`. The
  bug is fixed (`common/nixbld.nix`, deployed 2026-10-12), so nothing needs it, and
  it is not declarative — a switch will not recreate it. Remove it
  (`userdel -r deploy`) unless something turns out to depend on it. Do **not**
  deploy through it: builds as `leigh-admin` are safe and that path is the one
  `just switch` takes. (Since 2026-10-15 the "private flake inputs" half of why it
  existed is gone entirely: the flake has no `~/Projects` inputs to copy.)

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

- **`machines/pi-room-sys/ssh_host_ed25519_key` is a private host key in plain
  git** (moved in with the tree on 2026-10-15; it was committed in
  `leigh-hackspace/pi-room-sys` before that, so this is not a new leak — it is a
  new place to fix it). `netboot.nix` puts it in the netboot initrd as
  `boot.initrd.network.ssh.hostKeys`, so it is needed at *build* time: it should
  become a sops secret rendered through `config.sopsSecretText`
  (`common/sops.nix`, which already exists for exactly this) and then be
  **rotated**, because anyone who can PXE can read the initrd and the same file is
  in git history in two repositories. While it is a plain file it is also why
  `nix flake check` cannot run without a checkout of `machines/pi-room-sys/`:
  the path literal is copied at evaluation. Related, same file: `emergencyAccess =
  true` gives an unauthenticated root shell in the initrd, which is a lab
  convenience on the Pi Room VLAN and still worth knowing about
  (`machines/pi-room-sys/README.md`).

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

## Router (gw)

- **`machines/gw/` is a router with no hardware** (started 2026-10-14). It is in the
  flake so its config is evaluated by `nix flake check` and its tables checked by
  `just check`, and so the eventual cutover is `nixos-generate-config` plus reading
  facts off the old box rather than writing five files under pressure. Blocked on:
  the board existing. `machines/gw/README.md` owns the whole list — what is proven
  from the old box's generated files, what is inferred, and the table of "fact → the
  file it goes in → the command that reads it". Do not deploy it, and do not add it
  to `just check-build` (that recipe means "about to deploy").

- **`aaisp_ppp_chat_secrets` does not exist** (found while writing
  `machines/gw/sops.nix`, 2026-10-14). The tunnel's credentials have never been in
  `secrets/secrets.yaml`: the old box kept them in OPNsense's own config and nobody
  has read them off it. sops-nix fails *activation*, not evaluation, on a missing
  key, so `gw` builds fine and would refuse to come up. Read them from the old box's
  `/conf/config.xml` and `echo … | sops edit secrets/secrets.yaml` — never into a
  transcript (rule 13).

- **Nothing obtains the network's IPv6 prefix** (`machines/gw/networking.nix`,
  2026-10-14). The RA ranges in `common/vlans.nix` and the fixed v6 addresses in
  `common/dns-records.nix` are correct and dnsmasq advertises them, but no part of
  the new router's config asks the uplink for the /48, so IPv6 to the outside world
  will not work on a cutover today. How the old box got its prefix (DHCPv6-PD from
  AAISP, or a static assignment) is the first thing to read off it.

- **The new firewall's rules are shape-correct, not value-verified**
  (`machines/gw/firewall.nix`, 2026-10-14). nixpkgs validates the ruleset against a
  user-mode kernel at build time — which caught two real mistakes (ICMPv6's ND types
  are `nd-neighbor-solicit`, not `neighbour-solicitation`; `packet-too-big` is ICMPv6
  only) — but "parses" is not "is what the old box did". The old box's
  `/var/etc/nftables.conf` and `nft list ruleset` have never been read; do that
  before trusting any rule here, and note the per-VLAN anti-spoof rule emits *two*
  match forms (`vlan id N` and `iifname "vlanN"`) because which one the kernel
  actually matches on is a question about the real box.


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

