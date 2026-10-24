# The front door on the router — decided, phase 1 in tree, **not applied**

The plan: move HTTPS termination for `*.int.leighhack.org` — and, later, `headscale` —
from `services1` to **`gw`**, the box that is already the network's front door for DNS.
**Nothing in this doc has been deployed.** Three commits are in tree: `just check` is
green (on aibox — rule 22), the gw system derivation **builds**, and both modes of the
new guard were proven by making them fail rather than by reading them (below). A full
`just check-build` is *not* green on aibox and is not green because of this change:
`door-entry-management-system` sets `__noChroot` and aibox's `sandbox = true` daemon
refuses it, which is `docs/follow-ups.md`'s known reason and lands before the gw closure
is ever reached. `git log` on both boxes still shows the generation that has services1
terminating TLS for the whole network, and `common/int-dns.nix` still answers every
`*.int` name at 10.3.1.20. Read `docs/router-and-dns.md` and `machines/gw/README.md`
first: `gw` hands out every address in the building, so a change here is a change to the
box every client depends on, not to a shadow.

## The decision, 2026-10-24

The open question this file used to end with — *should the public names terminate at the
router at all?* — is answered **no**, and the answer is load-bearing for everything
below, so it is stated before the mechanics rather than deduced from which vhosts moved.

1. **What moves to `gw` is the `*.int` namespace**: the `int.leighhack.org` certificate,
   the `*.int` vhosts, and the guard that keeps the vhosts honest with
   `common/int-dns.nix`. Those are names whose answers are LAN addresses, reached from
   the LAN and the tailnet, and `gw` is already on every path between them.
2. **What stays on `services1` is everything the internet dials**: `*.leighhack.org`,
   `*.ai.leighhack.org`, `*.test.leighhack.org`, the DNAT rows in
   `machines/gw/firewall.nix` that carry them to services1, and therefore the `nginx-sso`
   gate in front of them. `gw` is a 4-core box whose outage *is* the network's outage,
   and it is now the thing that answers DNS and DHCP for every device in the building;
   making it also the thing the internet connects to is a different bet from the one
   this doc is about.
3. That decision buys consequence 4 for free. `nginx-sso` injects `X-WEBAUTH-USER`, which
   the *upstream* trusts, and every app it gates is a public name; if the public names
   stay, the gate stays, and there is no window in which a gate on one box fronts apps on
   another. It also means `web-test`/`retro`/`robot.ai` — the three public names the old
   version of this file listed as "move by hand, forgetting one is a 409 and a 502" —
   do not move at all.
4. **The certificate splits instead of moving.** `gw` issues `int.leighhack.org` +
   `*.int.leighhack.org`; services1 keeps the rest. Two boxes issuing one *name* is one
   `_acme-challenge` TXT record with two writers, and this repo's rule for that shape
   (see `docs/authentik-container-migration.md`) is that it must fail, not race. A split
   nameset makes the steady state race-free and puts a hard limit on the blast radius:
   the router *cannot* answer for a public name, whatever a firewall rule does later.
   The overlap is not zero during the migration and is quantified under
   [Findings](#findings-2026-10-24).
5. **`headscale` follows later, not now.** Its state directory has to be *moved* (see
   phase 4), which is an operation on live tailnet enrolment and does not belong in the
   same change as a new TLS endpoint.

## What the front door is today

| Name space | answered by | terminated by |
| --- | --- | --- |
| `*.int.leighhack.org`, `gw.int…` | `gw` dnsmasq, generated from `common/int-dns.nix` + `common/dns-records.nix` (`machines/gw/dns.nix`) | `services1` nginx (`machines/services1/http.nix`, `useACMEHost = "leighhack.org"`). Phase 1 puts a second, *provable* terminator for these names on `gw`; no client is sent there until `frontDoor` changes. |
| `*.leighhack.org`, `*.ai.*`, `*.test.*` | DigitalOcean, hand-added | `services1` nginx, same cert, reached through `gw`'s DNAT rows. **Not moving.** |
| `ts.leighhack.org`, tailscale node names | `headscale`'s own DNS, run on `services1` (`machines/services1/services/headscale.nix`), which tells tailnet clients to resolve `int.leighhack.org` through `10.3.1.1` | nobody — headscale serves the *web* console at `https://tailscale.leighhack.org`, a vhost on `services1` |

So today the network has **two** front doors: `gw` for name resolution and nftables,
`services1` for TLS and proxying. `http.nix` is one file of ~34 vhosts, the cert, and the
`nginx-sso` unit (`machines/services1/lib/nginx-sso-config.nix` generates its config);
`int-dns.nix` is what keeps the vhost list and the DNS table honest.

## What moving TLS to `gw` costs, one consequence per line

1. **The cert has to be issued on `gw`, and issuing it there has a trap of its own.**
   `security.acme` moves with the nginx that uses it (`dnsProvider = "digitalocean"`,
   `environmentFile = CONFIG.ENV_FILE` — the DO token is `DO_AUTH_TOKEN` inside
   `CONFIG.ENV_FILE`, never print it), so the env-file secret is declared in
   `machines/gw/sops.nix` too. The trap is not the token, it is the *propagation check*:
   lego asks the system resolver whether the TXT it wrote has appeared, the system
   resolver on this box is dnsmasq, and dnsmasq is authoritative for
   `int.leighhack.org` — `local = "/int.leighhack.org/"` in `machines/gw/dns.nix` — so it
   answers `_acme-challenge.int.leighhack.org` from its own data, forever, with a
   negative:

   ```
   dig @127.0.0.1 _acme-challenge.int.leighhack.org TXT
   -> status: NXDOMAIN, flags: qr rd ra        (measured on gw, 2026-10-24)
   ```

   The challenge would fail at propagation and never issue. So
   `machines/gw/http.nix` sets `defaults.dnsResolver = "1.1.1.1:53,8.8.8.8:53"`, and the
   line is load-bearing on a 60-day timer, because the *renewal* asks the same question.
   An earlier draft of this file explained the same situation as "still works while
   services1's nginx answers the ACME HTTP challenge", which is wrong twice over: the
   challenge here is DNS-01 — that is the whole reason the DO token is needed — and
   services1's nginx has never been in the loop for it. (An HTTP-01 challenge *would* be
   a problem of the sort that comment was reaching for: port 80 on the router is not
   DNAT'd to anything, so an HTTP-01 issuance on `gw` could not work at all. It does not
   have to.)
2. **`gw` needs nginx, and nginx needs the ACLs.** Every internal vhost carries
   `CONFIG.LOCAL_NETWORK` (the tailnet + LAN ranges in `common/config.nix`) because a
   vhost served *on the router* is reached by traffic that is already on the router's
   interfaces. Do not delete those ACL blocks when the vhosts move: `gw`'s nft sets police
   the *network*, nginx's `LOCAL_NETWORK` polices the *vhost*, and the second one is what
   keeps a tailnet client out of `firewall.int` / `truenas.int` / `monster.int`. Note
   `firewall.int.leighhack.org` already proxies `10.3.1.1:60443` — a **gw-local** upstream —
   so after the move that vhost stops leaving the box at all; and note the finding under
   [Findings](#findings-2026-10-24) about what answers 60443 today, which is nothing.
3. **The guards have to be re-rooted in the same change.**
   `machines/services1/int-dns.nix` refuses to evaluate services1 unless
   `common/int-dns.nix` equals **that box's** `services.nginx.virtualHosts` (names +
   `serverAliases`, derived by `common/nginx/int-vhost-names.nix`), and the gatus endpoint
   guard is the *same* derivation read from
   `machines/services1/services/monitoring.nix`. A vhost moved to `gw` without moving the
   guards means the next `just switch` on services1 fails with a table/vhost mismatch, and
   the gatus list quietly stops checking the moved names. The router-side guard is now
   `machines/gw/int-dns.nix` (imported by `machines/gw/default.nix`); it is a module rather
   than a third `just check` evaluator because it compares against a config, and
   `machines/gw/lib/check-network-tables.nix` remains the table-only half that `just check`
   can run on the router. It has **two modes**, because a staged move and an equal-set
   assertion are incompatible otherwise: while `frontDoor` names another box this one may
   serve a subset (pre-staging, which is how a front door gets proven) but may not invent a
   name; the moment `frontDoor` names `gw` the assertion is equality in both directions. The
   mode is read from the same string `doorAddresses` renders the answers from, so the flip
   and the tightening cannot be two separate changes.
4. **nginx-sso is decided, not deferred**: it stays on services1 with the public names it
   gates (§3 of the decision). One loose end this leaves, and it is a phase-3 problem:
   `login` is in `common/int-dns.nix`'s `names`, i.e. `login.int.leighhack.org` is a
   *front-door* name, and its upstream is the gate at `127.0.0.1:8082` on services1. When
   the door flips there are three answers and none of them is free — (a) gw forwards
   `login.int` to services1:8082, which carries the OIDC authorization code in clear text
   over the LAN, the hop that is loopback today; (b) the gate moves to gw, which drags the
   public vhosts' `auth_request` upstream off the box they run on; (c) the table gains a
   per-name owner so one name can answer at the door and be served by the origin. Pick one
   in phase 3 and write it here; do not discover it as a redirect loop.
5. **headscale on the router puts two DNS servers on one box.** `gw` answers `*.int` from
   dnsmasq (`/etc/dnsmasq-int-vhosts.hosts`, a store path and a `restartTriggers` — see
   `machines/gw/README.md`, and *never* edit `/etc/dnsmasq.d/network.conf`). headscale runs
   its own resolver for `base_domain = "ts.leighhack.org"` + `search_domains =
   ["int.leighhack.org"]` and re-asks `int.leighhack.org` at `10.3.1.1`. Moving it means:
   dnsmasq stays authoritative for `*.int`; headscale must **not** also carry those names in
   its own `addn`-style records; and one box now owns both the address assignment
   (dnsmasq/dhcpd) and the node ACLs (headscale's state in `/var/lib/headscale`, which must
   be *moved*, not recreated — see phase 4 for what is in there and which 72-byte file
   unenrols the building).
6. **headscale's HTTP/DERP facts stop being `services1`'s.** `ip_prefixes =
   [CONFIG.TAILNET_V4 CONFIG.TAILNET_V6]`, the DERP endpoints (`81.187.195.17` /
   `2001:8b0:1d14:225:d::1020` — the AAISP tunnel's addresses, `machines/gw/uplink.nix` +
   `machines/gw/constants.nix`) and the OIDC/headplane issuer/client/secret paths are all
   already router facts; the first two read as `services1` config today because that is
   where the unit ran.
7. **The DNS records do not move.** `common/int-dns.nix` says which *address* a name
   answers with; nothing there cares which box serves the name once it arrives. That is the
   reason the move is DNS-neutral, and it is what makes the guard (consequence 3) the only
   DNS work in the change. What *does* change is one field — `frontDoor` — and
   `machines/gw/lib/network-tables.nix`'s `doorAddresses` is what lets that one field name
   the router at all: the router has no reservation and cannot have one (it is the box
   handing out leases, and `check-network-tables.nix` rightly refuses a record with no MAC
   and no client-id), so its three addresses are read from `common/vlans.nix`'s VLAN whose
   `ipv4` is `hosts.router`. Read, not written: the addresses of a front door are one fact
   in one place exactly as long as that stays true.

## Phase 1: what is in the tree, and what it does not do

Two commits carry phase 1 (a third is this doc). Verified before writing any of this down,
on aibox — never on `gw`, AGENTS.md rule 22: `just check` is green (four assertion evals,
the four known warnings, alejandra clean), and `nix build
.#nixosConfigurations.gw.config.system.build.toplevel` completes. A full `just check-build`
is red on aibox for the pre-existing `__noChroot` reason in `docs/follow-ups.md` and does
not reach the gw closure, so it is not the gate for this change and pretending otherwise
would make the gate meaningless:

| Commit | Files | What it does |
| --- | --- | --- |
| `int-dns: the front door's addresses come from whichever box the table names` | `common/int-dns.nix`, `machines/gw/lib/network-tables.nix`, `machines/gw/lib/check-network-tables.nix`, `machines/gw/dns.nix` | `frontDoor` resolves through `doorAddresses`, so it may name `gw`; the check module refuses a door with no addresses and a door with no IPv6 half. Rendered `*.int` records are unchanged: `frontDoor` still says `services1`. |
| `gw: nginx, an int-only cert, and the guard that reads this box's vhosts` | `machines/gw/http.nix`, `machines/gw/int-dns.nix`, `machines/gw/default.nix`, `machines/gw/firewall.nix`, `machines/gw/sops.nix` | nginx + the `int.leighhack.org` cert + two `*.int` vhosts (`bar`, the canary, and `network-info`, whose upstream is this box) + the two-mode guard + 80/443 to the router from `vlan*`/`tailscale0` only + the env-file secret for the DO token. |

Two facts about phase 1 are only visible by breaking it, and both were done on aibox at
the commits above rather than asserted here:

* flipping `frontDoor` to `gw` while only two vhosts are served **fails the evaluation**
  with the 32 names it answers and nothing serves — the 31 labels still unmoved plus
  `authentik.int`, which is in `exempt` and so is expected to be answered by a device
  rather than served. The answer and the guard move together or not at all, which is what
  makes phase 3's last step one edit instead of a checklist.
* adding one vhost whose name is not in the table (`typo.int.leighhack.org`) fails it too,
  in the other direction, while `frontDoor` still says `services1`.

What it **does not** do, on purpose:

* change any answer. `frontDoor` is `services1`, so `/etc/dnsmasq-int-vhosts.hosts` is
  byte-identical before and after — and proving *that* is half of the deploy.
* serve the public names, or the other 31 `*.int` names. Two vhosts are enough
  to prove a cert, an ACL, a listener and a guard; the rest is a move, and a move is
  phase 3.
* listen anywhere the internet can reach. `portForwards` in `machines/gw/firewall.nix`
  still DNATs the public 80/443 to services1; the new rules are `iifname "vlan*"` and
  `iifname "tailscale0"` — the interfaces dnsmasq serves — and *not* `ip saddr
  ${aclV4}`, because the ACL contains public addresses and this box has public addresses of
  its own. See the comment on `frontDoorRules`.

### Deploying phase 1 — the runbook (not yet run)

`gw` cannot be switched from `gw` (rule 22): `just switch-gw` builds on the box it is run
from and pushes, so run it from services1.

1. `just check` **on aibox**, and `just check-build` (everything compiles, nothing
   activates). Both were green for the two commits above; re-run them at whatever HEAD you
   are about to deploy.
2. **`git log -1` on the box you are deploying from.** `just switch-gw` runs `sync-gw`
   first, which rsyncs *that box's* tree onto the router with `--delete` — so a stale
   checkout does not merely deploy the wrong generation, it mirrors the wrong generation
   onto `gw` and removes the files under review from the router's copy of the tree. On
   2026-10-24 services1's checkout was ten commits behind `origin/main` and aibox's three;
   the deploy is from whichever box now has the commit named in "Phase 1" above, and the
   rule generalises the other way too (AGENTS.md rule 20: a box can be *running* newer
   than it reads).
3. **Pre-flight, all read-only:**
   * `sudo -n ls -l /var/lib/acme/leighhack.org/ && sudo -n openssl x509 -enddate -noout -in
     /var/lib/acme/leighhack.org/cert.pem` on services1. The point is the *renewal date*:
     gw's new cert and services1's wildcard both write
     `_acme-challenge.int.leighhack.org` while the migration is half-done, so a renewal of
     either within minutes of the other is a failed validation (retried daily — see
     [Findings](#findings-2026-10-24)). At the 2026-10-24 cert (issued Sep 13, expires Dec
     12) services1 renews ~Nov 12 and a cert issued now renews ~Nov 23: 11 days apart, and
     the window is closed long before then by phase 3.
   * `sudo -n ss -lnt` on gw — nothing may already be listening on 80/443. (Read
     2026-10-24: nothing is. The listeners are 53 on the wildcard (dnsmasq), 22, 8091 on
     10.3.1.1 (network-status) and tailscaled's two high ports on `tailscale0` only.)
   * The hub and the tunnel are up: lego needs HTTPS out (via `router-uplink`) and UDP/TCP
     53 out to 1.1.1.1. `dig @1.1.1.1 txt _acme-challenge.int.leighhack.org` should return
     **nothing at all** (no record at rest — checked 2026-10-24, and the zone has no `*.int`
     wildcard that a TXT could collide with, which is the failure
     `docs/router-and-dns.md` warns about).
4. `just switch-gw` from services1 — which chains `sync-gw` and `&& sudo nixos-confirm`
   (GOLDEN RULE). Then, on gw, in the same session:
   `readlink -f /run/current-system /nix/var/nix/profiles/system-good`.
5. **Verify from rendered files, not from evaluation** (rule 10/21):

   * `systemctl is-active nginx`, first, because it is the only check here with an answer
     that fast. What nixpkgs renders for a box that uses `useACMEHost` (read off services1:
     `After=network.target acme-leighhack.org.service`, `Restart=always`, `RestartSec=5s`)
     is an ordering, not a dependency: a lego failure does not stop the router — it leaves
     **nginx restarting forever** while dnsmasq, the tunnel and the tailnet carry on. In
     phase 1 a front door that never came up is otherwise indistinguishable from a deploy
     that did nothing at all, which is the whole point of phase 1.
   * `sudo -n systemctl list-units 'acme*' --no-pager` — the units are named after the
     cert, so expect `acme-int.leighhack.org.service` ("Ensure certificate for
     int.leighhack.org"), `acme-order-renew-int.leighhack.org.service` and
     `acme-renew-int.leighhack.org.timer`, exactly as services1 has its `leighhack.org`
     trio. The request unit's `Result=success` and
     `sudo -n ls -l /var/lib/acme/int.leighhack.org/` are the proof; if lego failed, read
     the unit's journal before anything else, because the failure will be the propagation
     check in consequence 1.
   * `sudo -n nginx -t -c /etc/nginx/nginx.conf` and
     `grep -c server_name /etc/nginx/nginx.conf` — expect the two `*.int` servers plus the
     redirect servers, and no `leighhack.org` (public) name anywhere on this box.
   * `sudo -n nft list chain ip router input | tail` and
     `sudo -n nft list chain ip6 router6 input | tail` — the four front-door rules present,
     `vlan*`/`tailscale0`-scoped, and **no new `dstnat` row**.
   * `sudo -n ss -lnt` — now also `0.0.0.0:80`, `0.0.0.0:443`.
   * `sudo -n sha256sum /etc/dnsmasq-int-vhosts.hosts` **before and after**: identical.
     This is the proof that nothing a client resolves has moved, and it is the reason the
     deploy is safe: the front door is up, and nobody is sent to it.
6. **Prove the door answers, by IP:**

   ```
   curl -fsS --resolve bar.int.leighhack.org:443:10.3.1.1 https://bar.int.leighhack.org/
   curl -fsS --resolve network-info.int.leighhack.org:443:10.3.1.1 \
        https://network-info.int.leighhack.org/api/summary | head -c 200
   ```

   The canary page says which box served it, so phase 3 has a way to ask that question at
   both doors. From a client, the same two URLs with `--resolve …:10.3.1.20` still reach
   services1's copies, which is the state the network is in until the flip.
7. Roll back the same way as anywhere else (`system.autoRollback` is armed;
   `nixos-rebuild switch --system` to a previous generation). The worst case is a router
   with a listener nobody resolves to, and — if lego cannot issue — an nginx that will not
   start, which is *also* nobody resolving to it. Neither takes DNS, DHCP, the tunnel or
   the tailnet with it; that is the property phase 1 was scoped to keep.

## Phase 2 (not written): the origin question

Phase 3 cannot be written before this is answered, because most of the `*.int` upstreams
are on services1's loopback and `gw` cannot reach a loopback. From the tree, the 33 labels
in `common/int-dns.nix`'s `names` split:

* **12 name a `127.0.0.1` bind on services1**: `login` (8082, the gate), `doors` (8473),
  `api-doors` (8472), `filestore` (8096), `gocardless` (8095), `gatus` (8999), `grafana`
  (3000), `prometheus` (9091), `services1-status` (8088), `switch` (switch-gui),
  `unifi-admin` (11443), `zigbee2mqtt` (8282).
* **1 has no upstream at all**: `retro.int` serves `/sas-16tb/ds-downloads/Retro`, a
  directory that exists on services1 because services1 mounts the NAS. The router
  deliberately never mounts anything (`flake.nix`'s `routerModules`, and
  `machines/gw/configuration.nix` on why), so this name is either left behind — with
  `frontDoor` an exception the guard has to know about — or it stops being a filesystem
  root. Decide, do not inherit.
* **the other 19 already point off-box** and can move as they stand: aibox (6 — `ai`,
  `mcp`, `whisper`, `alexandria`, `frigate-monitor`, `aibox-status`), the 10.3.1.30 VM
  (`access-api`, `api`, `jenkins`, `user-tweaker`, `ha`), the appliance (`id`), monster,
  the NAS (`truenas`), services2 (`strata`, `services2-status`), gw itself
  (`network-info`, `firewall` — one of them dead, see Findings), `frigate` →
  `10.3.1.20:5000` (services1, but on a LAN bind), and `bar`.

The three shapes for the 12, with what each costs:

| Shape | Cost |
| --- | --- |
| Rebind each app to the LAN address (or `0.0.0.0` + a firewall rule) | 11 service modules, 11 exposure changes — `prometheus.int` and `grafana.int` listening on a LAN interface is a real widening, and every one of them needs its bind proven in the rendered unit. No new nginx on either box. |
| services1 keeps the per-name vhosts as an **origin** on a router-only HTTP listener; `gw` generates one forward per name from `common/int-dns.nix` | Nothing rebinds and modularity survives (a service still defines its own vhost next to itself), but there is a plaintext hop on the LAN and it must be *closed to everyone but the router* — an `allow 10.3.1.1; deny all;` origin ACL, or the forward-chain drop that amounts to the same thing — or the origin is a second door into every internal app, minus TLS. The per-name tuning (body sizes, `proxy_buffering off`, the 50 GB `immich` upload) has to be right on the outer proxy too, or the request dies at the router. |
| Move the apps off services1 (the direction the rest of the repo is already going) | The most work and the fewest compromises; not a precondition for anything. |

Whichever wins, consequence 2's ACL note has teeth in this phase: the `LOCAL_NETWORK`
check has to live where the client's real address is, which after any hop means the router,
and `$remote_addr` on the origin stops meaning anything. That is a security change and it
should be reviewed as one, not shipped as a side effect of moving a vhost.

## Phase 3 (not written): the flip

One pair of switches, minutes apart, with the DNS answer last:

1. `gw`: every name in the table served, the cert already issued (phase 1), the guard
   still in subset mode.
2. `common/int-dns.nix`: `frontDoor = "gw"` — and the guard flips to equality **in the same
   change**, because it reads the same string. `just switch-gw`, which rewrites
   `/etc/dnsmasq-int-vhosts.hosts` and restarts dnsmasq in the same activation
   (`restartTriggers`). `local-ttl = 1` means the network stops being sent to services1
   within a second of that restart, not within a lease.
3. services1: delete the `*.int` vhosts it no longer serves, drop `*.int` and `*.ai.int`
   from its cert's `extraDomainNames` (closing the shared-TXT window of consequence §4),
   and delete `machines/services1/int-dns.nix` — its guard is what the router-side one
   replaced, and leaving it would be a check that compares a table against a box that
   serves nothing from it. Its gatus endpoint guard moves too, or it fails:
   `machines/services1/services/monitoring.nix` subtracts `intVhostNames
   config.services.nginx.virtualHosts` from the endpoint list, so the moment the vhosts go
   the internal endpoints become un-derivable there.

Do not sit between 1 and 3: the overlap of two issuers of `_acme-challenge.int` is the
thing this migration should be minimising, and the window is measured in *weeks* if it is
left open.

## Phase 4: headscale

`/var/lib/headscale` on services1, read 2026-10-24:

```
drwxr-x--- headscale:headscale  0750
  db.sqlite      147 KiB     + db.sqlite-shm, db.sqlite-wal (4.1 MiB)
  derp_server_private.key      72 B
  noise_private.key            72 B
  .local/
```

which says four things. It is the unit's `StateDirectory=headscale`
(`StateDirectoryMode=0750`), so the *whole* directory moves, WAL included — copying
`db.sqlite` alone while a 4 MiB write-ahead log stays behind is a database missing its
recent state. `noise_private.key` is the file that unenrols the building: a fresh one is a
new node identity for the control server, which is the outcome consequence 5 was warning
about, so the move is stop-services → copy → `chown headscale:headscale` → start, and the
proof is `headscale nodes list` answering the same node list on both sides *before* the old
unit is allowed to start again. `derp_server_private.key` is the DERP server's identity and
travels with it. The config half (consequence 6) is already router-shaped facts.

## Findings, 2026-10-24

* **`firewall.int.leighhack.org` proxies `https://10.3.1.1:60443` and nothing listens
  there.** `ss -lntp` on gw answers 53 (dnsmasq), 22 (sshd), 8091 on 10.3.1.1
  (network-status) and tailscaled's own ports; `60443` appears in the whole tree exactly
  once — as that `proxyPass` — and the retired appliance's admin UI was deliberately *not*
  reopened on this box (`machines/gw/firewall.nix`'s ssh-rule comment, `machines/gw/README.md`).
  So the vhost is a 502 today and consequence 2's "after the move that vhost stops leaving
  the box" describes a move from one dead upstream to another. Either there is something
  meant to be listening on 60443 or the vhost and the `firewall` row in
  `common/int-dns.nix` both go; tracked in `docs/follow-ups.md`.
* **Two boxes, one TXT record.** While services1's cert still covers `*.int`, both it and
  gw's `int` cert validate through `_acme-challenge.int.leighhack.org`, and two writers of
  one TXT name is the shape this repo's rule says must fail rather than race. What "fail"
  costs is bounded and worth stating: lego creates and deletes the TXT it needs, and
  whether the second writer *joins* the first's value or replaces it is lego's business
  (it is not worth an experiment on a live zone either way), so the worst outcome of a
  collision is a failed validation — which nixpkgs re-runs daily through
  `acme-renew-<cert>.timer`, and which is silent until the 30-day mark if nobody looks.
  That is why the deploy step reads the cert's `notBefore`/`notAfter` before starting a
  second issuer, and why phase 3 closes the window instead of leaving it. Let's Encrypt's
  per-registered-domain limit counts `leighhack.org` for both boxes' certs, so two boxes
  renewing a 90-day cert each is ~1 certificate a fortnight against a limit of 5 a week:
  nothing to engineer around, and worth knowing before anyone force-renews twice.
* **The DigitalOcean zone is clean for a second issuing box**, checked by reading the zone
  rather than reasoning about it (the recipe, without printing the token, is in
  `docs/router-and-dns.md`): 112 records, **no `*.int` wildcard** and no `_acme-challenge*`
  row at rest, so a TXT under `_acme-challenge.int` cannot collide with a CNAME. The
  wildcards that do exist are `*.ai` and `*.yunohost`, neither of which is under
  `_acme-challenge`. This is the check `docs/router-and-dns.md` says must be made against
  a real renewal; the `_acme-challenge.int` renewal on services1 has been happening since
  before this repo had a word for "front door", so the half about `*.int` is already
  proven.
* **`*.ai.int.leighhack.org` and `*.test.leighhack.org` are in services1's cert and in
  nothing else**: no vhost in the tree serves a name in either space (the two names that
  look like they do — `web-test.leighhack.org`, `robot.ai.leighhack.org` — are single-label
  names in the other two namespaces, and they stay on services1 with the public names).
  `machines/gw/http.nix` therefore does *not* inherit them, and phase 3 should delete them
  from services1's `extraDomainNames` rather than carry them.
* **`immich.leighhack.org` is a public name built with the internal vhost helper**
  (`machines/services1/http.nix`, `mkIntVhost { acl = ""; … }`). It is a small thing, but it
  is the shape that turns "the internal helper is only used by internal names" into a lie,
  and the guard cannot tell: `int-vhost-names.nix` filters on the *suffix*, so an `acl =
  ""` public name under `.int` would be silently at home in the front-door table.
* **A phase-1 deploy is the only one in this plan that cannot break a name.** Every later
  one either moves an answer or moves an upstream; worth being explicit about before the
  third gets rushed.
