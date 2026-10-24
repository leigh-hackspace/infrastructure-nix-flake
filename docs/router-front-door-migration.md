# The front door on the router — **deployed 2026-10-24**: `gw` terminates everything, `services1` is an origin

**The migration is done.** `gw` holds the network's one certificate and renews it, serves
every public and `*.int` name from `common/front-door.nix`, runs the `nginx-sso` gate and
`anubis`, and answers the `*.int` zone (`frontDoor = "gw"` in `common/int-dns.nix`).
`services1` is an **origin**: the 22 vhosts whose apps are app-shaped stay there on plain
HTTP :80 behind `CONFIG.ROUTER_ONLY`, with no ACME config, no gate, no anubis and no
public name of its own. `headscale` and `headplane` moved to `gw` in phase 4 with their
state copied, and the public AAAA/`nginx.int` records in DigitalOcean moved with the
answers. The deploys, in order and with receipts, are under "Phase 3 and phase 4 are
deployed" below.

What the four phases were for, and the findings that shaped them, are kept below because
the reasoning is the part that is expensive to reconstruct. The plan was: move **all**
HTTPS termination — the certificate, every vhost, the `nginx-sso` gate and `anubis` — from
`services1` to **`gw`**, the box that is already the network's front door for DNS, and
`headscale` too. Phase 1 was the two commits `034d7aa` and `e31884c`, deployed at `75e210a`
on 2026-10-24 with the two commits after them (`6363ada`, `8197c88`, `75e210a`) being the
deploy's own findings.
A full `just check-build` is *not* green on aibox and is not green because of this change:
`door-entry-management-system` sets `__noChroot` and aibox's `sandbox = true` daemon refuses
it, which is `docs/follow-ups.md`'s known reason and lands before the gw closure is ever
reached. `git log -1` on services1 and gw both read `75e210a` after the deploy, and
everything still answering `*.int` is still on 10.3.1.20. Read `docs/router-and-dns.md` and
`machines/gw/README.md` first: `gw` hands out every address in the building, so a change
here is a change to the box every client depends on, not to a shadow.

## The decision, 2026-10-24 (operator): **all of it, on `gw`**

Every piece of nginx that the network has **one** of terminates on the router: TLS and
its certificate renewal, the `*.int` and public vhosts, the `nginx-sso` gate, `anubis`,
the `LOCAL_NETWORK` ACLs, and the DNS answers. The services behind `gw` are to be
*ignorant of where their traffic comes from* — they serve, and they know they are already
protected. The reason SSL termination ever lived on services1 was that the old router was
OPNsense and could not hold a NixOS nginx; it is `machines/gw/` now, so that reason is
gone and the arrangement is kept only by habit.

An earlier revision of this file argued the opposite (public names stay on services1, the
cert splits in two, the gate stays put) on blast-radius grounds. That argument is
overridden, and it is recorded here rather than deleted because the *reason* it was
written is the list of things the new decision has to engineer rather than inherit:

1. **`gw` is already the single point of failure, so making it the single policy point
takes no new availability risk and removes a class of drift.** `gw` leases every address,
answers every name, holds the AAISP tunnel, the tailnet and the nftables sets. The outage
that used to argue against putting the front door here is the outage the network already
agreed to. What the move actually *buys* is that the four things which must not be
duplicated — the ACME issuer, the gate, the bot gate, the ACL — stop being duplicated
*config* that two boxes can disagree about.
2. **One issuer means the TXT race stops existing** — see consequence 1. It goes from
   "a window to minimise, quantified in Findings" to "closed, because services1 ends with
   no `security.acme` at all". Phase 1 deliberately keeps two for a while and says why.
3. **"Ignorant of where the traffic comes from" is a security claim, and it is only true
   if the origin is reachable *only* by the front door.** The apps that assert identity do
   it from a header. `crates/switch-gui/src/routes.rs` says the current guarantee out
   loud: *"`X-WEBAUTH-USER` … is trusted because the vhost is LAN-only and the app binds
   loopback: there is no path to this socket that does not go through nginx"* — the same
   reasoning `status-dashboard` uses for its writes, the same reasoning behind `--no-auth`
   being refused on a non-loopback bind (AGENTS.md). Move the terminator and that sentence
   is the thing at risk, not a nicety: if a client can reach the origin directly it can
   assert any username to it. That is why phase 2 is an **origin** design and not a
   rebind-everything design (§Phase 2), and why the origin-reachability rule ships in the
   same change as the first moved vhost, not after it.
4. **Today's exposure list is wider than the vhost ACLs imply** — several "loopback"
   upstreams are on `0.0.0.0` already (podman publishes, `nginx-sso`'s own listener is
   `*:8082`), which is receipts-not-theory under [Findings](#findings-2026-10-24). The
   design in phase 2 fixes those as a side effect rather than depending on them being
   harmless.
5. **`headscale` follows** (phase 4), because its state directory has to be *moved* and
   that is an operation on live tailnet enrolment, not a config change.

## What the front door is today (after the flip)

| Name space | answered by | terminated by |
| --- | --- | --- |
| `*.int.leighhack.org`, `gw.int…` | `gw` dnsmasq, generated from `common/int-dns.nix` + `common/dns-records.nix` (`machines/gw/dns.nix`); `frontDoor = "gw"` | `gw` nginx, from `common/front-door.nix` — the single certificate, the gate and `anubis` all on the router |
| `*.leighhack.org`, `*.ai.leighhack.org` | DigitalOcean, hand-added; the shared CNAME target is `nginx.leighhack.org`, whose A/AAAA point at `gw` | `gw` nginx, same certificate, via the DMZ DNAT rows that now translate onto the router |
| `ts.leighhack.org`, tailscale node names | `headscale`'s own DNS, now on `gw` (`machines/gw/headscale.nix`), which tells tailnet clients to resolve `int.leighhack.org` through `10.3.1.1` | `gw` nginx — `tailscale.leighhack.org` and `headplane.leighhack.org` are rows in the table |

The network now has **one** front door. `services1` keeps an nginx forever, but it is an
**origin**: 22 vhosts on plain HTTP :80, `CONFIG.ROUTER_ONLY`, no certificate, no gate, no
anubis. `common/front-door.nix` is the table both boxes are rendered/checked against —
which rows stay origins is a fact in one file, and `machines/services1/int-dns.nix` fails
the evaluation if its vhosts and the table's origin rows disagree.

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
   **It is not sufficient on its own**, though — naming the resolvers does not un-teach them
   what they already cached — and the deploy found that out: see finding 2 under "Phase 1 is
   deployed" for `--dns.propagation.wait`, which is the thing that issues the certificate.
   An earlier draft of this file explained the same situation as "still works while
   services1's nginx answers the ACME HTTP challenge", which is wrong twice over: the
   challenge here is DNS-01 — that is the whole reason the DO token is needed — and
   services1's nginx has never been in the loop for it. (An HTTP-01 challenge *would* be
   a problem of the sort that comment was reaching for: port 80 on the router is not
   DNAT'd to anything, so an HTTP-01 issuance on `gw` could not work at all. It does not
   have to.)

   Under "one issuer" this consequence ends well: services1 finishes with **no
   `security.acme` at all**, so there is one box writing `_acme-challenge*` records and the
   race in [Findings](#findings-2026-10-24) is a phase-2 transition detail rather than a
   standing condition. What has to be true while that happens:

   * the cert on `gw` grows to the whole SAN set services1 holds today
     (`leighhack.org`, `*.leighhack.org`, `*.ai`, `*.int`, `*.ai.int`, `*.test`) in the
     **same change that takes the public vhosts**, and the one re-issue that implies is
     worth checking for rate limits on (`leighhack.org` is one registered domain for both
     boxes: 5 certificates a week).
   * `machines/services1/http.nix` loses its `security.acme` block in the same change that
     the last `useACMEHost` disappears from it — not one change later, because a box with
     no cert and a `useACMEHost` is a box whose nginx will not start.
   * the env-file secret is declared on both boxes only for the window (`env_file` is
     already in `machines/gw/sops.nix`); services1's copy stays because it holds nine other
     secrets, but `DO_AUTH_TOKEN` in it becomes unread-by-anything on that box.
2. **The ACL moves with the terminator, and the origin keeps one rule.** Every internal
   vhost carries `CONFIG.LOCAL_NETWORK` (the tailnet + LAN ranges in `common/config.nix`)
   because a vhost served *on the router* is reached by traffic that is already on the
   router's interfaces — and once the router terminates, the router is also the last place
   `$remote_addr` still means the client. So the ACL is not copied to `gw` and left behind:
   it moves, and the origin's rule becomes the one that actually protects anything —
   *reachable from `10.3.1.1`, nothing else*. Keep `gw`'s nft sets and nginx's ACL distinct
   in the reading too: the sets police the *network*, `LOCAL_NETWORK` polices the *vhost*,
   and the second is what keeps a tailnet client out of `firewall.int` /
   `truenas.int` / `monster.int`. `firewall.int.leighhack.org` already proxies
   `10.3.1.1:60443` — a **gw-local** upstream — so after the move that vhost stops leaving
   the box at all; see the finding under [Findings](#findings-2026-10-24) for what answers
   60443 today, which is nothing.

   The mechanism for the real client address, since the origin will see only the router:
   `X-Forwarded-For`/`X-Real-IP` set by `gw` (`recommendedProxySettings` already does), and
   if a backend genuinely needs the peer rather than the header, nginx's realip module has
   to be switched on per-vhost — the package is built `--with-http_realip_module`
   (nginx 1.30.5, read off services1's store path) but this nixpkgs' nginx **module** has no
   option for it (no `real_ip` anywhere in `nixos/modules/services/web-servers/nginx/`), so
   `set_real_ip_from 10.3.1.1; real_ip_header X-Forwarded-For;` goes in `extraConfig`.
   Which is also the trap in header form: `set_real_ip_from` names the *only* address whose
   forwarded header may be believed, and an origin that believes XFF from anywhere else is
   `--bind 127.example.com` again (AGENTS.md rule 17).
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
4. **`nginx-sso` moves, and moving it fixes a hole the current layout cannot.** The gate
   belongs at the terminator because its `X-WEBAUTH-USER` injection is trusted by whatever
   serves the name — and, once `*.int` is served here too, `login` is a *front-door* name
   (`common/int-dns.nix`'s `names`), so a gate left on services1 would mean the login flow
   is served by a box that no longer answers the name. The three options an earlier
   revision of this file listed — forward `login.int` to services1's gate and carry the
   OIDC code in clear text over the LAN; keep a second gate; add a per-name owner to the
   table — all collapse: the gate is on `gw`, `login.int` and `login.leighhack.org` are
   vhosts on `gw`, and the OIDC callback is loopback again.

   Which is the argument for doing it this way beyond tidiness. `nginx-sso`'s config
   (`machines/services1/lib/nginx-sso-config.nix`) sets `listen.addr = "0.0.0.0"`, and
   `ss -lntp` on services1 reads `*:8082`: **the auth endpoint the whole network's
   authentication depends on answers any LAN client today** (401 to a bare
   `curl http://10.3.1.20:8082/auth` from aibox, 2026-10-24 — refused, but *answered*),
   while its own ACL decides "is this caller LAN or tailnet" from an `X-Real-IP` field that
   only our nginx sets honestly — i.e. the caller that is not our nginx can put anything in
   it. Co-locating the gate with the nginx that calls it is what lets `listen.addr` be
   `127.0.0.1`, and that is a bug class removed rather than a service relocated. Two
   details for the change: `nginx_sso_auth` (the cookie key) and the OIDC client secret are
   read through `config.sopsSecretText` into the generated config — world-readable store
   paths, already true today, and `common/sops.nix`'s `infra.buildTimeSecrets` is where the
   sentence justifying that has to be re-justified for `gw`, not inherited — and the cookie
   domain is `.leighhack.org`, so a rename of the login host would invalidate every session
   at once, which is fine once and surprising twice.
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

## Phase 1: what was in the tree, and what it did not do

*(Everything below is deployed; this section is kept as the description of the first step,
because phase 2 and 3 were built on top of its two vhosts and the reasoning about what a
first step may safely leave undone is what still governs the flip.)*

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
* serve the public names, or the other 31 `*.int` names. Two vhosts are enough to prove a
  cert, an ACL, a listener and a guard; the rest is a move, and a move is phase 2 (the
  table) then phase 3 (the answers).
* take any traffic it is not yet addressed for. `portForwards` in
  `machines/gw/firewall.nix` still DNATs the public 80/443 to services1, and the new rules
  are `iifname "vlan*"`/`tailscale0` — the interfaces dnsmasq serves — rather than
  `ip saddr ${aclV4}`, because the ACL contains public addresses and this box has public
  addresses of its own, so an ACL-shaped rule in phase 1 would put the *internal* vhosts on
  the internet while nothing is meant to reach them. That is a phase-1 scoping decision, not
  a permanent property: phase 3 step 4 turns the public 80/443 into input rules on the
  internet-facing side, and `machines/gw/http.nix`'s cert grows to cover the public names in
  the same change so that "this box cannot answer a public name" stops being a safety net
  and becomes a fact with a certificate behind it. See the comment on `frontDoorRules`.

### Deploying phase 1 — the runbook (as run 2026-10-24; step 5 is what the run needed)

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
     is an ordering, not a dependency, and the acme unit is `WantedBy=multi-user.target`
     rather than `RequiredBy`: **a failed issuance does not fail the switch.** It leaves
     nginx running forever behind a router that looks healthy — and *running*, not
     crash-looping, which is what this paragraph used to get wrong: nixpkgs writes a
     **minica self-signed certificate** when lego fails, so nginx starts, serves, and
     stays up. Check the unit `After=` line here too — it is one grep and it proves the
     pairing came from `useACMEHost` and not from hope:
     `systemctl cat nginx | grep ^After` should name `acme-int.leighhack.org.service`.
   * `sudo -n systemctl list-units 'acme*' --no-pager` — the units are named after the
     cert, so expect `acme-int.leighhack.org.service` ("Ensure certificate for
     int.leighhack.org"), `acme-order-renew-int.leighhack.org.service` and
     `acme-renew-int.leighhack.org.timer`, exactly as services1 has its `leighhack.org`
     trio.
   * **`sudo -n systemctl start front-door-cert-check.service` — the check that actually
     decides whether the cert is the network's.** `Result=success` and a populated
     `/var/lib/acme/int.leighhack.org/` are **not** the proof, and this deploy is the
     receipt for that sentence: lego failed, both acme units still reported
     `Result=success`, the directory was full, `nginx -t` was happy, the canary answered
     200, and the certificate was `issuer=CN=minica root ca 56254e`. Only the issuer
     distinguishes the two states, so the issuer is what the unit checks (and it checks
     expiry at 14 days too, on a daily `OnCalendar` — see the two comments in
     `machines/gw/http.nix` for why that timer may not be monotonic). Its failure branch
     is testable without waiting for a failure: point a copy of the rendered script at any
     self-signed cert and it exits 1 with the `why:`/`fix:` lines. Expect
     `int.leighhack.org is CA-issued and not near expiry: issuer=C=US, O=Let's Encrypt, …`.
   * If it is red: read `journalctl -u acme-order-renew-int.leighhack.org.service -n 50`
     before anything else, because the failure will be the propagation check (consequence
     1 and the one-writer finding). The path back is
     `sudo -n systemctl start acme-order-renew-int.leighhack.org.service && sudo -n
     systemctl restart nginx` — say it here rather than have it be a discovery, because
     the renewal timer is daily and "wait for the timer" is a 24-hour answer to a
     20-second command. (`start`, not `restart`: the unit is a oneshot that has already
     run.) Re-ran exactly this on 2026-10-24 after `--dns.propagation.wait` landed and it
     issued in ~5 min — two SANs × the 120 s wait, which is the new cost of an issuance
     here.
   * `nginx -t` needs the **store path** on this box: `nginx` is not in gw's
     `environment.systemPackages`, so `sudo -n nginx -t` is `command not found` and looks
     like a missing service. Take it from the unit —
     `NGX=$(sudo -n systemctl show -p ExecStart --value nginx | sed -E 's/^[^/]*//' | cut -d' ' -f1)`
     then `sudo -n $NGX -t -c /etc/nginx/nginx.conf` — and
     `grep -oE 'server_name [^;]*;' /etc/nginx/nginx.conf | sort -u`: the two `*.int`
     servers, and **no `leighhack.org` public name** anywhere on this box (measured:
     exactly `bar.int…` and `network-info.int…`).
   * `sudo -n nft list chain ip routerIn input | tail` and
     `sudo -n nft list chain ip6 router6 input | tail` — the four front-door rules present
     (two per family), `vlan*`/`tailscale0`-scoped, and **no new `dstnat` row**: count them
     (`nft list chain ip routerNat dstnat | grep -c 'tcp dport'` → 4 before, 4 after).
   * `sudo -n ss -lnt` — now also `0.0.0.0:80`, `0.0.0.0:443` (and the `[::]` pair).
   * `sudo -n sha256sum /etc/dnsmasq-int-vhosts.hosts` **before and after**: identical.
     This is the proof that nothing a client resolves has moved, and it is the reason the
     deploy is safe: the front door is up, and nobody is sent to it. (`b20c264cded8…` at
     the 2026-10-24 deploy, unchanged across three switches.)
6. **Prove the door answers, by IP:**

   ```
   curl -fsS --resolve bar.int.leighhack.org:443:10.3.1.1 https://bar.int.leighhack.org/
   curl -fsS --resolve network-info.int.leighhack.org:443:10.3.1.1 \
        https://network-info.int.leighhack.org/api/summary | head -c 200
   ```

   No `-k`, and that is the point once the issuer check is green: these two curls failing
   on trust *is* the minica state, seen from the client side. The canary page says which
   box served it, so phase 3 has a way to ask that question at both doors. From a client,
   the same two URLs with `--resolve …:10.3.1.20` still reach services1's copies, which is
   the state the network is in until the flip — verified from services1 after this deploy:
   `grafana.int` 200 from 10.3.1.20, `login.int` 302 off the gate, both untouched.
7. Roll back the same way as anywhere else (`system.autoRollback` is armed;
   `nixos-rebuild switch --system` to a previous generation). The worst case is a router
   with a listener nobody resolves to, and — if lego cannot issue — an nginx serving a
   certificate no client trusts, which is *also* nobody resolving to it. Neither takes DNS,
   DHCP, the tunnel or the tailnet with it; that is the property phase 1 was scoped to keep.

   **The five-minute window is a countdown, and on this box it counts down to a router
   reboot.** `auto-rollback.timer` is `OnActiveSec=5min`/`RemainAfterElapse=false`: an
   activation arms it, it fires once, and it stops — so `inactive` is the *normal* state on a
   box more than five minutes past a switch (gw and the untouched services1 both read
   `inactive`), and `is-active` is therefore not a health check. The comparison it makes is:
   `readlink -f /run/current-system /nix/var/nix/profiles/system-good`. What bit on the way
   in: a switch that fails after `/run/current-system` has flipped leaves the new generation
   current against an older `system-good`, which is exactly the state the timer waits to
   reboot for. Decide inside the window — fix the activation and confirm, or let it roll
   back — and note that the next activation re-arms it, so leaving it stopped saves nothing.

## Phase 2: the origin — services1 stops being a front door

*(Written and deployed since this heading was drafted — see “Phase 2 is in the tree and
phase 3 step 1 is deployed” below for what was built, the two near-misses the table diff
caught, and the places where the design below was amended: `origin` is one of **three**
kinds of row, not two, and the public names went into `common/front-door.nix` rather than a
separate `common/public-names.nix`.)*

The shape is **front door + origin**, not "rebind every app": `gw` terminates and routes
by name, and services1 keeps its per-service vhosts as *origin* vhosts — no certificate,
no `forceSSL`, no gate, no `LOCAL_NETWORK`, reachable from the router alone. Apps stay
exactly where they bind today. That is also the answer to the question this file used to
call "the origin question": three of the four reasons to rebind-everything are reasons
not to.

1. **Rebinding breaks the invariant the identity headers depend on** (consequence 4 and
   §3 of the decision). `switch-gui` on `0.0.0.0:8097` is a switch-configurator that trusts
   `X-WEBAUTH-USER` from anyone who can reach it, and its doc comment says the trust *is*
   the bind. Under the origin design the bind never moves: gw → services1's origin vhost
   (loopback) → app. The guarantee is preserved by construction instead of re-created by a
   firewall rule that someone will widen at 04:00.
2. **`retro.int` dissolves.** It is a `root =` on `/sas-16tb/ds-downloads/Retro`, a
   directory that exists on services1 because services1 mounts the NAS — and the router
   deliberately never mounts anything (`flake.nix`'s `routerModules`,
   `machines/gw/configuration.nix`). Under the origin design the filesystem root stays
   where the mount is and `gw` only needs the name. What was going to be an exception in
   the guard is now a row that says "origin".
3. **"Loopback" is already not what it looks like, and the apps that are not loopback are
   not loopback for the front door's benefit.** `ss -lntp` on services1 (2026-10-24) reads
   `0.0.0.0:` on the podman-published ports — `8004` (the speedtest anubis fronts), `8282`
   (zigbee2mqtt), `11443` (`unifi-admin`'s appliance UI), `5000` (frigate), `5671`/`1883`
   (rabbitmq/mosquitto), `5432` (postgres), plus the UniFi controller's `8080/8443/8843/8880`
   — while `grafana`(3000), `mattermost`(8065), `status-dashboard`(8088),
   `gocardless`(8095), `filestore`(8096), `switch-gui`(8097), redis(8401) and
   synapse(`[::1]:8008`) are real loopback binds. So "the app is private" is currently a
   per-service accident, and the origin-reachability rule below is what turns it into a
   property. It is **not** a licence to tighten all of them: several of those ports have
   consumers that are not nginx (the UniFi devices dial the controller directly, the
   cameras dial MQTT), so each port gets checked against its service's doc first.

**The split to render.** `common/int-dns.nix`'s 33 labels:

* **13 are origin** — the 12 loopback ones (`login` moves with the gate rather than being
  forwarded: see consequence 4), `doors` (8473), `api-doors` (8472), `filestore`,
  `gocardless`, `gatus`, `grafana`, `prometheus`, `services1-status`, `switch`,
  `unifi-admin`, `zigbee2mqtt`) and `retro`.
* **20 are direct** and move as rows naming an address: aibox 6 (`ai`, `mcp`, `whisper`,
  `alexandria`, `frigate-monitor`, `aibox-status`), the 10.3.1.30 VM 5 (`access-api`,
  `api`, `jenkins`, `user-tweaker`, `ha`), the appliance (`id`), monster, `truenas`,
  services2 2 (`strata`, `services2-status`), gw 2 (`network-info` — already here — and
  `firewall`, whose upstream is dead), `frigate` → `10.3.1.20:5000`, and `bar` (already
  here).

**The files.** A vhost is now exactly one box's concern, so the definitions leave the
service modules: `common/front-door.nix` (DATA) — one row per name the door serves,
`*.int` **and** public, with `upstream = "origin" | <address:port> | "local"`, the
per-name policy it owns (`sso`, `anubis`, `websockets`, `bodySize`, `timeouts`,
`proxyBuffering`, `aliases`) — rendered by `common/nginx/front-door.nix` (HELPER) into
`machines/gw/http.nix`. The alternative (20 service modules each importing a router fact)
is how two boxes end up with an opinion about one vhost, which is the bug being removed.
The guards follow the data: `machines/gw/int-dns.nix` already checks `*.int` against the
rendered vhosts, and the public names have no table today at all — which is why
`docs/router-and-dns.md` can tell a story about "two live vhosts with no DNS record".
`common/public-names.nix` is that table, and its guard can check every fact the evaluator
is allowed to see (uniqueness, one upstream per name, the `*.int` subset matching
`common/int-dns.nix`, aliases not colliding). **What it must not do is reach the network:**
"this name has a DigitalOcean record" is a runtime check — gatus, which already checks the
endpoints — not a `builtins.fetchurl` at eval time (AGENTS.md: evaluation must not build,
and here it would not even be allowed to talk).

**The reachability rule, which is the phase.** It lives on `gw` because `gw` is where every
non-router client's packet to an origin is *forwarded* — including the tailnet's, and
including services1's own clients — and the router's own traffic is not forwarded at all,
which is exactly the exemption required. One pair of rows per origin host in
`machines/gw/firewall.nix`'s forward chain (drop forwarded `tcp/{80,origin ports}` to the
host unless `ip saddr` is the router), plus the origin nginx keeping
`allow 10.3.1.1; deny all;` as the app-layer half. Two notes for whoever writes it: the
origin's port list is a *table*, not a guess — it comes from the same rows
`common/front-door.nix` renders, or it drifts; and a dropped forward is silent by design
(AGENTS.md's anti-spoof rules say why), so `counter` on those rows is not decoration.

**`anubis` moves with it**, because it is a front-door function and there is one of them:
`machines/services1/services/librespeed.nix` runs the instance today with
`TARGET = http://127.0.0.1:8004` and a unix socket that nginx proxies through
(`http://unix:…`), plus `users.users.nginx.extraGroups = [ anubis ]` "required due to unix
socket permissions" — a line that has to exist on `gw` too or the socket is a 502. On `gw`
the target becomes `10.3.1.20:8004` (already published), the `METRICS_BIND` socket moves
with it, and whatever scrapes anubis has to be retargeted in the same change.

## Phase 3: the flip, in the order that leaves a rollback

Five steps, each one a switch of one box, each one reversible without touching the next
box. Read them as "what is briefly worse" rather than "what is briefly broken":

1. **`gw` serves everything** — full SAN set (consequence 1), all vhosts from the table,
   the gate, anubis, the ACLs. Nothing points here; prove it with
   `curl --resolve grafana.int.leighhack.org:443:10.3.1.1 https://…` and
   `curl --resolve status.leighhack.org:443:10.3.1.1 https://…` (the second is the gate
   path). Guard still in subset mode.
2. **`gw`: the origin-reachability rows**, tightening only (no traffic is served from here
   yet, so dropping forwarded `tcp/80` to an origin can only cost a redirect nobody
   needs). Check the counter, then check nothing else broke: the UniFi controller and the
   MQTT broker are the two that will complain first if the port list is wrong.
3. **services1: origin mode.** `forceSSL = false` with `addSSL = true` — 443 keeps
   serving, 80 now *answers* instead of redirecting — and the vhost ACL narrows to the
   router (`CONFIG.ROUTER_ONLY`, the 15-word sibling of `LOCAL_NETWORK`). Briefly worse: a
   LAN client typing `http://foo.int` gets a 301 from a box that is about to stop
   answering, or nothing. Do 3 and 4 in one sitting.
4. **`gw`: the two answers move.** `common/int-dns.nix` `frontDoor = "gw"` (dnsmasq
   rewrites `/etc/dnsmasq-int-vhosts.hosts` and restarts in the same activation;
   `local-ttl = 1`, so the network follows in a second, not a lease), **and**
   `portForwards` retargets the public 80/443 from services1 to the router — with the IPv6
   half done properly, because v6 has no NAT to hide behind:
   `publicV6ForwardRules` (a forward rule for services1's addresses) becomes an input rule
   for this box's own. Roll back either half by reverting this one switch; services1 is
   still serving TLS with a valid certificate until step 5.
5. **services1: delete the front door.** `security.acme`, the `useACMEHost`s, the
   `nginx-sso` unit and its config (moved, not copied, to `common/nginx-sso-config.nix`),
   the anubis instance, `machines/services1/int-dns.nix`, and the `*.int` vhosts that are
   now only rows in a table on another box. The gatus endpoint guard moves here too, or the
   next switch on services1 fails: `machines/services1/services/monitoring.nix` subtracts
   `intVhostNames config.services.nginx.virtualHosts` from the endpoint list, so deleting
   the vhosts without moving the guard leaves the internal endpoints un-derivable. Also
   verify from services1 after this step: `curl -ksS https://grafana.int.leighhack.org/`
   now hairpins services1 → gw → services1, and it has to return 200 for that to be the
   monitoring topology from now on.

Steps 1–4 are where the two-issuer window from [Findings](#findings-2026-10-24) lives; do
not park between 4 and 5, and do not park between 1 and 4 either — every week between them
is a week the two configs can drift apart with only the guard on `gw` noticing.

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

## Phase 3 and phase 4 are deployed — what the flip cost, and the two records outside the repo

Deployed on 2026-10-24 as one commit (`d145505`) rather than the five switches the plan
listed, because the intermediate states are exactly the ones the plan says not to park in
and both closures were built on aibox first, so neither switch had to compile. The order
was still the one that leaves a rollback, compressed to minutes:

1. `services1` → origin mode (`just switch`, confirmed in the same invocation). This stops
   `headscale`/`headplane`, removes `security.acme`, the `nginx-sso` unit and `anubis`, and
   leaves nginx serving 22 vhosts on :80. From this moment until step 2 every name that was
   terminating on services1 is broken, which is why the window is a build-free switch and
   not a deploy.
2. `/var/lib/headscale` and `/var/lib/headplane` copied from services1 to `gw` (the source
   was already stopped) before the router started a new one. A fresh `noise_private.key`
   would have been a new control-server identity, i.e. an empty tailnet.
3. `gw` (`just switch-gw` from services1, confirmed by the recipe). `dnsmasq` rewrites
   `/etc/dnsmasq-int-vhosts.hosts` and restarts in the same activation, so the answers move
   with the nginx that serves them; `frontDoor = "gw"` makes `machines/gw/int-dns.nix`
   tighten from subset to equality in the same evaluation.
4. `services2` and `aibox` pulled the commit and switched too (their configs did not change;
   the generations are only for consistency).
5. DigitalOcean records, outside the repo and therefore easy to forget: `nginx`'s AAAA
   (`2001:8b0:1d14:225::20` → `2001:8b0:1d14:225::1`), `nginx.int`'s A and AAAA
   (`10.3.1.20` → `10.3.1.1`, `fd99:dead:beef:225d::20` → `fd99:dead:beef:225d::1`), and
   `haproxy.int`'s AAAA. `nginx.leighhack.org` is the CNAME target of every public name
   (and `nginx.int` the private one the off-LAN `*.int` CNAMEs use); leaving either on
   services1 is a public IPv6 site that connects to a box with no TLS listener. TTL is
   3600, so cached resolvers keep the old answer for up to an hour after the API says 200.

What was measured after the switch, not evaluated:

* `bar.int` and `grafana.int` resolve to `10.3.1.1` (the dnsmasq hosts file hash moved from
  `b20c264…` to `aa35bcf…`), the canary says `Served by gw`, `speed` answers 200 **through
  anubis** on `gw`, `login.int` is a 302 off the gate, and every origin row in
  `common/front-door.nix` answers through the door (doors 200, element 200, mattermost 200,
  filestore 200, switch 302, matrix/api-doors 404 at `/` as they always did).
* `front-door-cert-check.service` reports `Let's Encrypt, CN=YE2` — the real certificate,
  not minica. It is the check from phase 1 and it is the only one that can tell.
* a direct `curl -H 'Host: grafana.int…' http://10.3.1.20/` from the LAN is **403**, which
  is `CONFIG.ROUTER_ONLY` doing its job; the door's own request to the same vhost is 200.
* `headscale nodes list` shows the same node set (and the `gw` node connected), so the
  copied database is the live enrolment, not a fresh one.
* `ss -lnt` on services1 no longer shows :443, :8082, :8085 or :8086; `acme*` units are
  absent. On `gw` the gate is `127.0.0.1:8082`, headplane `127.0.0.1:8086`, headscale
  `127.0.0.1:8085`, and only nginx answers 80/443.

Two findings from this deploy, both the same shape as phase 1's — *the check passed and the
thing was still wrong*:

1. **A `preStart` chown cannot fix a copied state directory.** The first version re-owned
   `/var/lib/headscale` and `/var/lib/headplane` in `ExecStartPre`. `ExecStartPre` runs as
   the unit's `User=headscale`, so the chown fails (`Read-only file system` for headplane's
   state dir, which belongs to *another* unit's namespace; `Permission denied` inside it),
   and `headscale` crash-looped while `headplane` sat in `dependency failed`. The fix in
   `machines/gw/headscale.nix` is `systemd.tmpfiles.rules` with `d` + `Z`: it runs as root at
   activation, recursively, before the units, and is idempotent on every later boot.
2. **The state is only half the move; the names are the other half.** `headscale` bound
   `0.0.0.0:8085` on services1 and inherited that line. Every client of that port is `gw`'s
   nginx, so it now binds loopback like the gate (the public listener that genuinely needs
   one, DERP's STUN socket, is a separate `stun_listen_addr` bind and is DNAT'd). The
   `tailscale.leighhack.org` row's `/admin` split lives in `common/front-door.nix` as a
   `locations` map, so the control endpoints (`/key`, `/machine/...`) still reach 8085 while
   `/admin` reaches headplane on 8086.

One dead name was carried through and should be deleted or fixed deliberately:
`aibox.status.leighhack.org` is in `common/front-door.nix` and was a services1 vhost, but it
has no DigitalOcean record and the certificate's `*.leighhack.org` does not match a
**two-label** name (`aibox.status.leighhack.org`), so it has never worked from outside. It is
not a regression: the old single cert did not cover it either.

## Phase 1 is deployed — the three things it took, and the one it exposed in itself

Deployed on 2026-10-24 from services1 at `8197c88`, three switches deep rather than one, and
the extra two were both the certificate. What is written here is what the first deploy could
not see, because the shape of all three failures is *a check that passes*:

1. **`Result=success` means "a certificate is on disk", never "the CA issued it."** This
   nixpkgs' `security.acme` writes a **minica self-signed certificate** into the cert
   directory when lego exits non-zero — deliberately, so dependent services start — so on
   the first deploy nginx came `active`, served TLS, answered the canary with 200, passed
   `nginx -t`, and reported both acme units `Result=success` over a certificate issued by
   `CN=minica root ca 56254e`. Every check step 5 of the runbook listed was green. The only
   fact that separates the two states is the issuer, so `machines/gw/http.nix` now carries
   `front-door-cert-check.service`/`.timer` to read the issuer (and the expiry, at 14 days)
   and the runbook runs it. Both branches are tested: the success path against the real
   cert, and the failure path against a throwaway self-signed one — a check whose failure
   branch first runs on the day the cert goes wrong is a check nobody has ever run.
2. **The TXT race is real with *one* writer, because a cert's apex and its wildcard share
   one challenge record.** The finding below is about two boxes writing
   `_acme-challenge.int`; on the router the collision is *inside a single issuance*. lego
   validated `*.int.leighhack.org` (+38 s, LE said so), then validated `int.leighhack.org`
   — same TXT name, still present, cleanup happens after the run — and spent its whole 60 s
   window being told `NXDOMAIN` by 1.1.1.1: a public resolver answering from a negative it
   had cached earlier, about a record the CA had just confirmed. `dnsResolver` fixes *which*
   resolvers are asked and cannot fix *what they remember*, so `security.acme.defaults` now
   sets `--dns.propagation.wait=120s`: present the TXT, let the zone settle, then ask the
   CA, and depend on no resolver's cache at all. Cost: ~5 min an issuance on a unit with
   `TimeoutStartSec=infinity` behind a daily timer. The issuance that had failed issued on
   the first try after it.
3. **A health check must not be able to arm a router reboot.** The commit that added the
   check deployed it with `OnBootSec=15min` — and a monotonic timer whose window has already
   passed fires the moment it is created, so on a box two days into its uptime the first
   switch started the check *inside the activation*, the check exited 1 (correctly: the cert
   on disk was then minica's), and `switch-to-configuration` counts a unit failing during
   activation as a failed switch: exit 4, rollback armed, and on `gw` the rollback is
   `reboot`, five minutes later, taking DNS, DHCP, the tunnel and the tailnet with it. The
   timer is a wall-clock `OnCalendar` with no `Persistent` now, because a catch-up is the
   same trick from the other direction. Read `machines/gw/http.nix` before adding any timer
   to this box. (Recovering from that failed switch, I stopped `auto-rollback.timer` and
   wrote a rule here saying it then stays disarmed forever. That is wrong, and the corrected
   version is in step 7 below and in AGENTS.md's GOLDEN RULE: the timer is armed per
   activation, `inactive` is its resting state on every healthy box, and that is exactly why
   `is-active` proves nothing — read the two paths it compares.)

Two smaller facts from the same deploy, both now in the runbook: `nginx` is not in gw's
`systemPackages`, so the verification command has to take the binary's store path from the
unit; and this nixpkgs has no `systemd.services.<name>.startAfter` (it wants `after`, which
is also the semantics that were meant — ordering with no dependency).

## Phase 2 is in the tree and phase 3 step 1 is deployed — and four things it taught

At `1ae2e59`, `gw` renders **every one of the network's 59 names** from one table, holds a
certificate with the whole public SAN set (`leighhack.org`, `*.leighhack.org`,
`*.ai.leighhack.org`, `*.int.leighhack.org` — Let's Encrypt, expires 2027-01-22), runs the
`nginx-sso` gate and `anubis`, and has done it without moving an answer: `/etc/dnsmasq-int-
vhosts.hosts` is still `b20c264cded8…`, `*.int` still resolves at 10.3.1.20 and still returns
200 from services1. The gate is on `127.0.0.1:8082` here, which closes the `*:8082` finding
for the box that will own it.

Five lessons, all of them from being wrong in a way the review had already passed over:

1. **The check that made this safe was a diff of rendered configs, not a review of the
   table.** Two comparisons, both worth keeping in the toolkit:
   * *name set*: the names the table renders (row keys **plus their `serverAliases`**,
     because aliases share a server block and an ACL) against the `server_name`s in
     services1's live `/etc/nginx/nginx.conf`, `comm`'d — 59 to 59, nothing added, nothing
     dropped. Comparing keys only would have “found” two names that are really aliases.
   * *per name*: for each TLS server block, `proxy_pass` / `client_max_body_size` /
     `proxy_read_timeout` / number of `allow` lines / presence of `auth_request`, joined on
     the name, printing only the differences. Everything below came out of that, and none of
     it would have come out of reading the table.
2. **A vhost's policy is not always visible at the vhost literal, and the table inherits
   every blindness in it.** `ai.leighhack.org` is built by `lib.mkMerge` over an
   `mkSSOVirtualHost` in `machines/services1/ai.nix`, so a reader looking for
   `mkSSOVirtualHost {` does not see it — and my first table row for it had **no gate**.
   That is an unauthenticated proxy onto aibox's inference endpoint, shipped as a
   refactor. Its twin, in the other direction: `doors.int` and `api-doors.int` carry **no
   `LOCAL_NETWORK` today** (0 `allow` lines), because the door hardware and members' phones
   dial them from outside the LAN; the table's “`*.int` means LAN-only” default quietly
   added an ACL that would have locked the doors. Both are recorded on their rows, because
   the row is where the next reader will be.
3. **`*.int` defaulting to LAN-only is a rule with public aliases behind it.** Any
   `*.int` row with a public `serverAlias` inherits the LAN ACL for the public name too —
   which is what services1 does today, so it is *faithful*, but it is now a decision
   written in a table rather than an accident of nginx server blocks.
4. **Let's Encrypt refuses an apex that a wildcard in the same order already covers.**
   Listing `int.leighhack.org` beside `*.leighhack.org` is a `400 malformed` (“redundant
   with a wildcard domain in the same request”), and services1's cert does not contradict
   that — `leighhack.org` is not matched by `*.leighhack.org`. Full text on `certDomains`.
5. **Step 1 is not a provable end state on its own, for the origin rows.** They proxy to
   services1's port 80, which answers `301 → https://the same name`, and the client's next
   request comes back to `gw` — a redirect loop, visible as a 301 to itself on
   `grafana.int`/`doors.int` and harmless only because nothing resolves here. The direct
   rows tell the truth: `network-info` 200, `monster` 200, `id` 302 into authentik, `speed`
   200 through anubis, `aibox.status` 200 with the gate's LAN rule, `firewall` a 502 exactly
   as it is on services1 today. So **steps 3 and 4 are the change that makes the door
   serve**, and step 1 is the half that can sit indefinitely because nobody is sent to it.

### Correction to the plan's ordering, from the same reading

Step 2 — “`gw`: the origin-reachability rows, tightening only” — is **not** safe before the
flip, and the one-sentence justification next to it is wrong. Dropping *forwarded* `tcp/80`
and `tcp/443` to 10.3.1.20 while services1 is still the live front door does not cost “a
redirect nobody needs”: clients on VLAN 226/227 and every tailnet node reach 10.3.1.20
**through this box's forward chain**, so the rule cuts them off from every service, and only
same-subnet VLAN-225 clients are unaffected because their traffic is never forwarded. The
rows belong in the same switch as the flip (step 4), which is also where `X-Forwarded-For`
starts being the only way an origin sees a client. Revised order:

```
1  gw serves everything                      (deployed — this change)
2  services1 → origin mode                   ┐
3  flip the answers + DNAT + v6, and the     ├ one sitting, minutes apart
   origin-reachability rows, together        ┘
4  services1: delete the front door          (immediately after)
```

**And the window the deploy has now opened is real:** services1 and `gw` both hold a
certificate covering `*.int.leighhack.org`, so both write `_acme-challenge.int`. services1's
cert expires 2026-12-12 (renews ~Nov 12) and gw's 2027-01-22 (renews ~Nov 23): the first
collision risk is ~Nov 12, and phase 3's last step — services1 losing `security.acme`
altogether — has to land before it. This is the “two boxes, one TXT record” finding with a
date on it, and the date is the reason not to park between step 1 and step 3.

## Findings, 2026-10-24

* **A switch that outlasts the auto-rollback window fails with a lie.** `security.acme`'s
  issuance runs *inside* activation (`nginx` is `After=acme-<cert>`, and the "order and
  renew" unit is `Wants`ed by it), so with `--dns.propagation.wait=120s` across four SANs
  the switch takes ~5–6 minutes — longer than the 5-minute `auto-rollback` window. The timer
  therefore fires **during** the next activation, finds `/run/current-system` and
  `system-good` differing (which is what an in-flight activation always looks like from
  there), tries to act, and loses the store lock to the switch it is supervising:

  ```
  auto-rollback-start: Could not acquire lock
  auto-rollback.service: Main process exited, code=exited, status=11/n/a
  warning: the following units failed: auto-rollback.service
  switching to system configuration … failed (status 4)
  ```

  Nothing was wrong with the system, and **the box did not reboot** (a second reason to read
  the failing unit's name before believing a rollback happened) — but `nixos-rebuild` exits
  4 and the chained `nixos-confirm` is skipped, so the new generation is left current and
  unconfirmed with `auto-rollback` spent and inactive. What to do: `readlink -f
  /run/current-system /nix/var/nix/profiles/system-good`; if the first is the generation you
  meant and the box is healthy, `sudo nixos-confirm` by hand and `systemctl restart
  auto-rollback.timer`. Two ways to stop it recurring: keep the wait small enough that
  activation stays inside the window, or move the first issuance out of the switch (start
  `acme-order-renew-<cert>.service` deliberately, then switch). This is the same class as
  the health-check finding above — **the safety net and the thing it supervises need to
  agree about how long the thing takes** — with the twist that here the net was armed by the
  *previous* switch, not this one.

* **The TXT race is not between boxes; it is about one TXT name.** Written before the
  deploy, this section argued that a second issuing box would create the race. The router's
  first issuance lost the same race **alone**, because an apex and its wildcard are both
  solved by `_acme-challenge.<apex>` — so this cert's two SANs are two writers of one name
  whatever the second box does — and because lego asks a *public recursive resolver* whether
  the TXT has arrived, and a resolver that has recently been told NXDOMAIN about a name
  keeps saying so from cache after the record exists. Measured: wildcard validated at +38 s,
  then 60 s of `NXDOMAIN` for the apex from 1.1.1.1, then `time limit exceeded`, with the
  record still up (lego cleans up after the run). The fix is to stop asking: `--dns.
  propagation.wait=120s`. Consequence for phase 3, when services1's SAN set moves here:
  **every apex+wildcard pair in the SAN set is one TXT name and one chance to lose this
  race**, which is a second reason not to carry `*.ai.int`/`*.test` forward.
* **A failed ACME issuance on this nixpkgs is invisible, not red.** `security.acme` falls
  back to a minica self-signed certificate so "dependent services will still start", and
  every consequence of that is written up under "Phase 1 is deployed" above and in
  `machines/gw/http.nix`, where the check that catches it now lives (`front-door-cert-check`).
  The general form, for any box in this fleet rather than just this one: `Result=success`
  plus a listening service plus a 200 is a statement about *state*, and the failure this
  one hid was a state that is legitimately healthy-looking. Ask for the fact that differs.

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
* **`*.ai.int.leighhack.org` and `*.test.leighhack.org` are wildcards in services1's cert
  that no vhost serves**: nothing in the tree answers a name in either space (the two names
  that look like they do — `web-test.leighhack.org`, `robot.ai.leighhack.org` — are
  single-label names in the other two namespaces, and they move to `gw` in phase 3 with the
  rest of the public set). So the single cert on `gw` should carry the namespaces that have
  vhosts, not the union of what the old cert happened to include: every extra SAN is an extra
  `_acme-challenge` TXT at renewal time, and `*.ai.int` cannot even be resolved — dnsmasq is
  authoritative for `int.leighhack.org` and `common/int-dns.nix` has no two-label names, so a
  client dialling `foo.ai.int` is told NXDOMAIN by the box that *is* the zone.
* **The gate's auth endpoint is listening on every interface.**
  `machines/services1/lib/nginx-sso-config.nix` sets `listen.addr = "0.0.0.0"`, port 8082,
  and services1's `ss -lntp` reads `*:8082`. So `curl http://10.3.1.20:8082/auth` from
  another box answers (401 from aibox, 2026-10-24) rather than refusing the connection,
  and the gate's own ACL decides "LAN or tailnet" from an `x-real-ip` *field* — honest when
  our nginx sets it from `$remote_addr`, whatever the caller likes when it is not our
  nginx. Fixing it is not a patch to that listener: it is consequence 4's move, which
  co-locates the gate with the nginx that calls it and then `listen.addr` can be
  `127.0.0.1`. Tracked in `docs/follow-ups.md` with the phase.
* **Which "loopback" upstreams are not.** Receipts in Phase 2's point 3; the short version
  is that podman publishes on `0.0.0.0`, so the speedtest sits in front of anubis at the
  nginx layer and nowhere else, `unifi-admin`'s appliance UI and zigbee2mqtt's UI answer
  the LAN without the vhost ACL, and postgres and rabbitmq do too. None of this is the
  migration's fault and none of it survives phase 3 step 2 — but each port needs its
  service's doc read before it is narrowed, because the UniFi devices and the cameras dial
  some of these directly.
* **nginx's realip module is built and un-optioned**: `--with-http_realip_module` in the
  package, no `real_ip` option in this nixpkgs' nginx module. Noted because phase 3 makes
  the origin see only the router, and the sentence "nothing needs the real client IP at the
  origin" is a claim about every app that logs, rate-limits or ACLs, not a free assumption.
  The audit is in `docs/follow-ups.md`.
* **`immich.leighhack.org` is a public name built with the internal vhost helper**
  (`machines/services1/http.nix`, `mkIntVhost { acl = ""; … }`). It is a small thing, but it
  is the shape that turns "the internal helper is only used by internal names" into a lie,
  and the guard cannot tell: `int-vhost-names.nix` filters on the *suffix*, so an `acl =
  ""` public name under `.int` would be silently at home in the front-door table. Under
  `common/front-door.nix` the helper disappears and the row says what it is.
* **A phase-1 deploy is the only one in this plan that cannot break a name** — and phase 2's
  step 1 is nearly as safe, because it is provable with `--resolve` before anything is sent
  there. Every step from phase 3 onward either moves an answer, moves an upstream or opens
  the router to the internet; worth being explicit about before any of them gets rushed.
  The one to schedule deliberately is phase 3 step 4, which is the only moment in the whole
  plan where the public internet and the network's whole DNS zone point at the router at the
  same time.
