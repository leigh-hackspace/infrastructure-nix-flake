# The front door on the router — how it works, and what it cost

`gw` is the network's front door. It holds the one certificate and renews it, answers the
`*.int` zone, terminates TLS for every public and internal name, runs the `nginx-sso`
gate and `anubis`, and decides which network may have which name. `services1` is an
**origin**: it keeps the vhosts whose applications are app-shaped (a `/srv` root, an auth
rewrite, a filesystem mount), on plain HTTP :80, reachable from `10.3.1.1` alone.
`headscale` and `headplane` run on `gw` too.

This was a migration, not the original design: until 2026-10-24 `services1` terminated TLS
for everything, because the old router was OPNsense and could not hold a NixOS nginx. The
move was decided on 2026-10-24 and deployed the same day. The phase-by-phase plan that got
there is not kept — it described states that no longer exist — but the lessons that cost a
deploy are, below, because each one is a failure that looked healthy.

## Where the facts live

| Fact | File |
| --- | --- |
| every name the door serves: upstream (`origin` / an address / `local`), ACL, gate, anubis, body/timeout limits, aliases | `common/front-door.nix` (DATA) |
| what a row renders to | `common/nginx/front-door.nix` (HELPER) |
| every `*.int` label, and which box the answers belong to | `common/int-dns.nix` (DATA), rendered into dnsmasq by `machines/gw/dns.nix` |
| the door's nginx, certificate, gate, anubis and the cert health check | `machines/gw/http.nix` |
| the origin vhosts | `machines/services1/http.nix` and the service modules; `common/nginx/origin-vhost.nix` is the builder |
| the origin ACL | `CONFIG.ROUTER_ONLY` in `common/config.nix`; the packet half is `originReachabilityRules` in `machines/gw/firewall.nix` |
| the tailnet control server | `machines/gw/headscale.nix` |

Two guards keep those in step, and both run at evaluation time:

* `machines/gw/int-dns.nix` — this box's `*.int` vhosts **equal** `common/int-dns.nix`'s
  `names` + `exempt`. (`frontDoor = "gw"`, so there is no subset mode any more.)
* `machines/services1/int-dns.nix` — this box's vhosts **equal** the
  `upstream = "origin"` rows in `common/front-door.nix` (keys plus `serverAliases`).
  A name that becomes a direct row and keeps a vhost behind it is a second config for one
  name, and it fails the evaluation.

The gatus endpoint guard (`machines/services1/services/monitoring.nix`) reads
`common/int-dns.nix`, not a vhost list, because the names it checks are answered by `gw`.

## Adding or moving a name

1. `*.int`: add the label to `common/int-dns.nix`'s `names`. Public names are
   hand-added in DigitalOcean; the whole public zone CNAMEs to `nginx.leighhack.org`.
2. Add one row to `common/front-door.nix`:
   * `upstream = "origin"` — services1's nginx serves it (only for app-shaped vhosts);
   * `upstream = "http://<address>:<port>"` — the door proxies straight to the box that
     runs the app. **An address, never a name**: dnsmasq runs here, and an nginx that
     resolves its own upstreams at boot is a boot-order loop.
   * `upstream = "local"`/`page` for a page this box serves.
   Add `serverAliases`, `sso`, `anubis`, `websockets`, `bodySize`, `timeouts`,
   `proxyBuffering`, `lanToken`/`strataBearer` as the name needs them. An `*.int` row is
   LAN-only unless it says `acl = "none"`; a public row is open unless it says otherwise.
3. `upstream = "origin"` means a vhost must exist on services1 too, built with
   `common/nginx/origin-vhost.nix`. Nothing else about it carries TLS or the gate.
4. Deploy `services1` first if you added an origin vhost, then `gw` (the answers are rendered
   on the router). Moving a name from origin to direct (or back) is one table edit plus the
   vhost change, in the same change.

## The invariant that makes the origin design safe

The apps behind the origins trust `X-WEBAUTH-USER`, because `services1`'s nginx is the only
thing that can reach them (`crates/switch-gui/src/routes.rs` states the old loopback version
of this). Two halves keep that true:

* **the vhost ACL** — `CONFIG.ROUTER_ONLY` at server level on every origin vhost. A direct
  LAN request to `10.3.1.20` gets 403 even from a machine that shares its VLAN.
* **the packet rule** — `originReachabilityRules` in `machines/gw/firewall.nix` drops
  forwarded `tcp/80,443` to services1 unless the source is the router. It shipped in the
  same switch as the flip; before that, clients on other VLANs and the tailnet reached
  services1 *through that forward chain*, so adding it early would have cut them off from
  the box still serving them.

Do not move an app that asserts identity onto a routable bind. Loopback is the boundary;
if it must move, it takes a shared secret instead of the header's implicit trust.

## Certificate facts that are easy to get wrong

* **`security.acme` does not fail when the CA does not issue.** It writes a minica
  self-signed certificate so dependent services still start, and `acme-*.service` reports
  `Result=success`. nginx runs, serves, `nginx -t` passes and a 200 proves nothing.
  `front-door-cert-check.service` on `gw` reads the **issuer** (and expiry at 14 days).
  Run it after any switch:
  `sudo systemctl start front-door-cert-check.service`.
* **The propagation check has to stop asking.** lego validates the TXT it wrote by asking a
  resolver; `gw` is authoritative for `int.leighhack.org`, so the system resolver would
  answer "no record" forever. `machines/gw/http.nix` names public resolvers **and** sets
  `--dns.propagation.wait=120s`, because an apex and its wildcard are one TXT name and a
  public resolver keeps serving a cached negative after the record exists. An issuance on
  this box takes ~5 minutes; the unit is behind a daily timer.
* **The cert's SAN set is `leighhack.org`, `*.leighhack.org`, `*.ai.leighhack.org`,
  `*.int.leighhack.org`.** No `int.leighhack.org` (Let's Encrypt refuses it as redundant
  with the wildcard), no `*.ai.int` and no `*.test` — nothing serves those, and every extra
  SAN is another TXT name to win at each renewal.
* **The rollback timer is armed per activation.** `auto-rollback.timer` is
  `OnActiveSec=5min` with `RemainAfterElapse=false`: a switch starts it, it fires once,
  compares `/run/current-system` with `system-good`, and reboots if they differ. A switch
  that takes longer than five minutes (an ACME issuance inside activation) can therefore
  have the timer fire *during* the next activation, fail on the store lock, and fail the
  switch with a misleading exit 4. Read the failing unit's name; then `readlink -f` the two
  paths, `sudo nixos-confirm` if the running generation is the one you meant, and restart
  the timer. `systemctl is-active auto-rollback.timer` is **not** a health check — inactive
  is the normal state on a healthy box more than five minutes past a switch.

## Moving headscale, if it ever moves again

`/var/lib/headscale` is the tailnet enrolment: `db.sqlite` plus its WAL, `noise_private.key`
(the control server's identity — a fresh one unenrols every node) and
`derp_server_private.key`. Stop the units, copy the **whole** directory and
`/var/lib/headplane`, then let the destination's `systemd.tmpfiles.rules` (`d` + `Z`) own it.
A `preStart` cannot do that job: `ExecStartPre` runs as the unit's user, and a directory
belonging to another unit is read-only in its namespace — that mistake crash-looped
`headscale` on the first deploy. Verify with `headscale nodes list`: the same nodes, and the
live node connected.

The HTTP servers bind loopback (8085 headscale, 8086 headplane); every client of them is
`gw`'s nginx. The one public listener headscale needs is DERP's STUN socket
(`stun_listen_addr`, DNAT'd from 3478/udp by `machines/gw/firewall.nix`).

## Things outside the repo that move with the answers

* **DigitalOcean.** `nginx.leighhack.org` is the CNAME target of every public name and
  its AAAA must be `gw`'s address (`2001:8b0:1d14:225::1`); `nginx.int.leighhack.org`
  is the CNAME target used off-LAN and points at `10.3.1.1` /
  `fd99:dead:beef:225d::1`; `haproxy.int` follows. **The AAAA was left at services1's
  `2001:8b0:1d14:225::20` when the flip shipped** — invisible on IPv4, fatal on IPv6
  (services1 is an origin on :80, so `:443` refuses and an IPv6-first client fails before
  it falls back). The casualty was the thing this box now hosts: tailnet clients could no
  longer reach `tailscale.leighhack.org`, retrying `/ts2021` indefinitely. The API held
  the right value while individual `ns1/ns2/ns3.digitalocean.com` edges still answered
  `::20`; **re-saving the record is what pushes it to every edge** (a TTL change is
  enough), and the resolvers holding the old answer — including the four
  `machines/gw/dns.nix` forwards to — drain on their own TTL. The record is 300 s now,
  not 3600: an hour of stale answers after every move is exactly how long this lasted.
  `docs/router-and-dns.md`, "Caches, and the one that bites".
* **`headscale` routes.** `gw` is the network's tailnet subnet router. The routes are in
  `machines/gw/uplink.nix`, but a manual `tailscale up` outranks the flake and
  `extraUpFlags` are applied by a **boot**; check
  `sudo tailscale debug prefs | jq .AdvertiseRoutes` and approve them in headscale.
* **A `*.int` name with no row** still resolves at `gw` and 404s. `common/int-dns.nix`'s
  `legacy` list is the set of those; deleting one is a deliberate change.

## Known dead or incomplete names

* `firewall.int.leighhack.org` proxies `10.3.1.1:60443`; nothing listens there (the retired
  appliance's admin UI was deliberately not reopened). It is a 502, carried forward so the
  migration changed no answer. Delete the row and the `firewall` label together, or say
  what 60443 is for.
* `aibox.status.leighhack.org` is a row (and a gatus endpoint) with no DigitalOcean record,
  and the certificate's `*.leighhack.org` cannot match a two-label name. It has never
  worked from outside; it needs a record and a `*.status.leighhack.org` SAN, or deletion.
* `authentik.int.leighhack.org` is in `common/int-dns.nix`'s `exempt`: the appliance at
  `10.3.1.36` answers it for RADIUS/LDAP while the door also serves it as an alias of
  `id.int`. Which answer should win is still open (`docs/follow-ups.md`).
