# The front door on the router — prepared 2026-10-22, **not applied**

The plan: move HTTPS termination (the ACME cert, the public vhosts, the
nginx-sso gate) and `headscale` from `services1` to **`gw`**, the box that is
already the network's front door for DNS. **Nothing in this doc has been
deployed.** `git log` on both boxes still shows the generation that has
services1 terminating TLS for the whole network. Read `docs/router-and-dns.md`
and `machines/gw/README.md` first: `gw` hands out every address in the building,
so a change here is a change to the box every client depends on, not to a shadow.

## What the front door is today

| Name space | answered by | terminated by |
| --- | --- | --- |
| `*.int.leighhack.org`, `gw.int…` | `gw` dnsmasq, generated from `common/int-dns.nix` + `common/dns-records.nix` (`machines/gw/dns.nix`) | `services1` nginx (`machines/services1/http.nix`, `useACMEHost = "leighhack.org"`) |
| `*.leighhack.org`, `*.ai.*`, `*.test.*` | DigitalOcean, hand-added | `services1` nginx, same cert |
| `ts.leighhack.org`, tailscale node names | **`headscale`'s own DNS**, run on `services1` (`machines/services1/services/headscale.nix`), which tells tailnet clients to resolve `int.leighhack.org` through `10.3.1.1` | nobody — headscale serves the *web* console at `https://tailscale.leighhack.org`, which is a vhost on `services1` |

So today the network has **two** front doors: `gw` for name resolution and
nftables, `services1` for TLS and proxying. `http.nix` is one file of ~34
vhosts, the cert, and the `nginx-sso` unit (`machines/services1/lib/nginx-sso-config.nix`
generates its config); `int-dns.nix` is what keeps the vhost list and the DNS
table honest.

## What moving TLS to `gw` costs, one consequence per line

1. **The cert has to be issued on `gw`.** `security.acme` (DO `dnsProvider`,
   `environmentFile = CONFIG.ENV_FILE` — the DO token is `DO_AUTH_TOKEN` inside
   `CONFIG.ENV_FILE`, never print it) moves with the nginx that uses it. `gw`
   already imports `./sops.nix`, so the env-file secret must be declared there
   too, not only in `machines/services1/sops.nix`. `dnsPropagationCheck =
   true` still works while services1's nginx answers the ACME HTTP challenge —
   the cert covers both boxes' names at once, so the **first** switch can leave
   services1's ACME config in place; the second switch deletes it. Two boxes
   issuing one wildcard cert is the same "two vhosts for one name" shape
   `docs/authentik-container-migration.md` insists on: it must fail, not race.
2. **`gw` needs nginx, and nginx needs the ACLs.** Every internal vhost carries
   `CONFIG.LOCAL_NETWORK` (the tailnet + LAN ranges in `common/config.nix`)
   because a vhost served *on the router* is reached by traffic that is already
   on the router's interfaces. Do not delete those ACL blocks when the vhosts
   move: `gw`'s nft sets police the *network*, nginx's `LOCAL_NETWORK` polices
   the *vhost*, and the second one is what keeps a tailnet client out of
   `firewall.int` / `truenas.int` / `monster.int`. Note `firewall.int.leighhack.org`
   already proxies `10.3.1.1:60443` — a **gw-local** upstream — so after the move
   that vhost stops leaving the box at all.
3. **The guards have to be re-rooted in the same change.**
   `machines/services1/int-dns.nix` refuses to evaluate services1 unless
   `common/int-dns.nix` equals **that box's** `services.nginx.virtualHosts`
   (names + `serverAliases`, derived by `common/nginx/int-vhost-names.nix`), and
   the gatus endpoint guard is the *same* derivation read from
   `machines/services1/services/monitoring.nix`. A vhost moved to `gw` without
   moving the guards means the next `just switch` on services1 fails with a
   table/vhost mismatch, and the gatus list quietly stops checking the moved
   names. The router-side version of the guard belongs next to
   `machines/gw/lib/check-network-tables.nix` (it is the only thing on `gw` that
   cannot be "fixed by deploying" — same argument, same place).
4. **nginx-sso is a decision, not a move.** `nginx-sso` at `127.0.0.1:8082` and
   `login.int.leighhack.org` (its own vhost, `acl = ""`, `proxyHeaderBuffers`)
   are the public-names gate. If the public vhosts move to `gw`, the gate moves
   with them — its `X-WEBAUTH-USER` injection is trusted by the *upstream*, so
   it must stay in front of whatever serves the public name. Leave it on
   services1 and the public vhosts there must keep proxying to it, which is a
   second, undocumented door into the same auth.
5. **headscale on the router puts two DNS servers on one box.** `gw` answers
   `*.int` from dnsmasq (`/etc/dnsmasq-int-vhosts.hosts`, a store path and a
   `restartTriggers` — see `machines/gw/README.md`, and *never* edit
   `/etc/dnsmasq.d/network.conf`). headscale runs its own resolver for
   `base_domain = "ts.leighhack.org"` + `search_domains = ["int.leighhack.org"]`
   and re-asks `int.leighhack.org` at `10.3.1.1`. Moving it means: dnsmasq stays
   authoritative for `*.int`; headscale must **not** also carry those names in
   its own `addn`-style records; and one box now owns both the address
   assignment (dnsmasq/dhcpd) and the node ACLs (headscale's state in
   `/var/lib/headscale`, which must be *moved*, not recreated — a fresh state
   dir mints a new `registration` secret and unenrols every device in the
   building).
6. **headscale's HTTP/DERP facts stop being `services1`'s.**
   `ip_prefixes = [CONFIG.TAILNET_V4 CONFIG.TAILNET_V6]`, the DERP endpoints
   (`81.187.195.17` / `2001:8b0:1d14:225:d::1020` — the AAISP tunnel's
   addresses, `machines/gw/uplink.nix` + `machines/gw/constants.nix`) and the
   OIDC/headplane issuer/client/secret paths are all already router facts; the
   first two read as `services1` config today because that is where the unit ran.
7. **The DNS records do not move.** `common/int-dns.nix` says which *address* a
   name answers with; nothing there cares which box serves the name once it
   arrives. That is the reason the move is DNS-neutral, and it is what makes the
   guard (consequence 3) the only DNS work in the change.

## The order to apply it in, and the gates

`gw` cannot be switched from `gw` (rule 22): `just switch-gw` builds on aibox and
pushes. So:

1. Write `machines/gw/http.nix` (ACME + the vhosts that become public-on-the-router)
   and the router-side `int-dns` guard; keep `machines/services1/http.nix`
   intact for one more switch (consequence 1).
2. `just check` **on aibox** — `just check` is still broken on `gw`
   (rule 22 / `docs/nix-dev-workflow.md`, "Reopened 2026-10-22"), and a red
   guard is exactly what consequence 3 exists to catch.
3. `just check-build` (everything compiles, nothing activates) from a box that
   builds — this is what catches a bad `programs.ssh.knownHosts` shape too
   (`common/build-farm.nix` now pins host keys, see
   `docs/nix-dev-workflow.md` "The hop itself was broken, separately").
4. Switch **gw** first (`just switch-gw` from services1), then switch services1
   to delete the moved vhosts/cert. GOLDEN RULE in both cases: `&& sudo nixos-confirm`
   in the same shell invocation, and `readlink /run/current-system
   /nix/var/nix/profiles/system-good` afterwards — auto-rollback is armed on all
   four deployable boxes and a rollback reboots the *router* out from under the
   next call.
5. Verify from rendered files, not from evaluation (rule 10/21):
   `/etc/nginx/nginx.conf` on **both** boxes (`grep -c server_name` per box,
   `grep -c acme_certificate`), `/etc/dnsmasq-int-vhosts.hosts` on gw
   (unchanged — prove it), `/etc/ssh/ssh_known_hosts` on gw (now non-empty), and
   the DNS canary end to end: `curl -fsS https://bar.int.leighhack.org/` is the
   shortest test that the record, dnsmasq, the vhost and the cert all still agree.

## Still open when this doc was written

- Whether the **public** names should terminate at the router at all: `gw` is a
  4-core box that answers DNS and DHCP for the building, and rule 22 exists to
  keep it from compiling. A TLS-terminating nginx on it is not a compile, but it
  is a public entry point on the one box whose outage is the whole network's
  outage — say which you want before flipping, rather than deducing it from
  which vhosts you moved.
- headscale's state directory: find it (`/var/lib/headscale`) on services1 and
  plan its move before the unit unenrols the building.
- `*.test.leighhack.org` and `*.ai.int.leighhack.org` in the cert's
  `extraDomainNames`: `web-test.leighhack.org`, `retro.leighhack.org` and
  `robot.ai.leighhack.org` are served by services1 today and are not `*.int`
  names, so they are **not** in `common/int-dns.nix` and no guard checks them.
  They move by hand, and forgetting one is a 409 on the cert and a 502 at the
  name.
