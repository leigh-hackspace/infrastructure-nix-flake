# Router (gw), dnsmasq and DigitalOcean DNS

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: the router is now
`machines/gw/` (flake-managed, cut over 2026-10-21), never edit the generated
`dnsmasq.conf`, every
`*.int` vhost needs `just dns-sync-sync` after the switch (and `dns-sync-prune`
after a rename/removal), and never print or copy off the DO token. This file is the
detail: where the router keeps its state, which names legitimately resolve
elsewhere, and the manual fallback when `dns-sync` is unavailable.

## The router

`machines/gw/` — a flake-managed NixOS box at **10.3.1.1**, cut over from the OPNsense
appliance on 2026-10-21. Nothing about the running router is out-of-band any more:
dnsmasq is generated from `common/vlans.nix` and `common/dns-records.nix`, nftables from
`common/config.nix`, and the AAISP tunnel and tailnet client live in `machines/gw/`.
Read `machines/gw/README.md` before changing it; `docs/gw-cutover-attempt-2026-10-21.md`
has the findings from the cutover and what is still open.

What still speaks the retired OPNsense box's language:

- **dns-sync** edits `/conf/config.xml` and reads `/var/etc/dnsmasq-hosts`; it cannot
  write `gw`'s `/var/lib/dnsmasq/names.hosts` yet, and its pinned host key is the old
  box's. Until it is ported (open in `docs/follow-ups.md`), the `*.int` names are a
  hand-restored copy of the old box's hosts file, and nothing refreshes them.
- **network-status** (`crates/network-status/`) ssh's to `10.3.1.1` and runs OPNsense
  commands; it needs the same port.

```bash
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.1   # gw; root is NOT keyed there
```

**Gotcha: dnsmasq's unsolicited RAs stop after a `router-lan` link flap**, so clients
lose their IPv6 default when the 1200 s router lifetime expires. `systemctl restart
dnsmasq` after cabling; `machines/gw/README.md` has the checks.

**Historical: the retired OPNsense box's root shell was tcsh** (`2>` redirection fails,
grep patterns with `<`/`>` need double quotes). That box is gone; `gw` runs NixOS and its
root is not keyed — use `leigh-admin` and `sudo`. The LAN dashboard
`network-info.int.leighhack.org` (`crates/network-status/`) still ssh's to it as
`root@10.3.1.1`, which is one of the things to port.

## dnsmasq — the LAN resolver

> **The rest of this section describes the retired OPNsense appliance** and is kept as
the reference for what dns-sync and network-status still expect. On `gw` the resolver
is `services.dnsmasq` from `machines/gw/dns.nix`: the generated conf is
`/etc/dnsmasq.d/network.conf` (from the tables), the flake-owned records are
`/etc/dnsmasq-records.hosts`, and the vhost names are `/var/lib/dnsmasq/names.hosts`
(hand-restored; dns-sync cannot write it yet). `machines/gw/README.md` is the manual.

dnsmasq on the router answers `*.int.leighhack.org` for the LAN.

- `/conf/config.xml` is the **source of truth**. Host overrides live under
  `<opnsense><dnsmasq><hosts>`.
- `/var/etc/dnsmasq-hosts` is the generated addn-hosts dump that actually answers
  queries — read it to verify what the network sees:
  `ssh root@10.3.1.1 'grep <name> /var/etc/dnsmasq-hosts'`.
- **Never edit `/usr/local/etc/dnsmasq.conf`** — it is auto-generated.

Manual fallback if `dns-sync` is unavailable: edit the host override in the
OPNsense web UI (Services → Dnsmasq DNS → Host overrides) or in
`/conf/config.xml`, then restart dnsmasq and verify against
`/var/etc/dnsmasq-hosts`.

### Reading `dns-sync check` output

`just dns-sync-check` compares three sources: the flake's expected `*.int` vhost
names (`/etc/dns-sync/expected-int-names`), the router, and DigitalOcean. Names
that are **supposed** to look wrong, so you do not "fix" them:

- `aibox.int` → 10.3.1.32 — its own record, not the nginx CNAME.
- `authentik.int` → 10.3.1.36 — its own override (nginx also serves it as an alias
  of `id.int`).
- `gitlab`, `ldap`, `mqtt`, `nginx`, `tailscale` predate dns-sync: reported "never
  expected" and left as-is.
- `filestore.int` is expected by the flake vhost and resolves to 10.3.1.20 in both
  the router and DigitalOcean.

## DHCP reservations

> On `gw` these are **`common/dns-records.nix`**, rendered into
> `/etc/dnsmasq.d/network.conf` by `machines/gw/dns.nix` and asserted by
> `machines/gw/lib/check-network-tables.nix`. The paragraphs below are the retired
> appliance's UI/XML method, kept for the devices whose records the tables have not
> absorbed yet.

Live in the same dnsmasq config (the router, not DNS): services1 (10.3.1.20, MACs
`c8:d3:ff:a5:b2:25` / `c8:d3:ff:a5:be:7c`), services2 (10.3.1.21, MAC
`70:85:c2:53:f1:d4`), aibox (10.3.1.32), nas1/nas2 (10.3.1.5 / 10.3.1.6), apps1
(10.3.1.30), yunohost (10.3.1.15).

Printers, same place: `voron` 10.3.2.50, `elegoo` 10.3.2.55 and `snapmaker`
(Snapmaker U1, MAC `28:2d:06:1d:bc:96`) 10.3.2.56 on the WIFI VLAN, `3d-lime`
10.3.14.61 and `3d-blue` 10.3.14.62 on the wired one — see
`docs/printer-servers.md`.

This is why `common/hosts.nix` holds **addresses, not names**: every machine in
this network is a DHCP reservation, and a table that resolved through the resolver
it is meant to be independent of would not be one.

### Adding one by hand

A reservation is a `<hosts>` entry under `<dnsmasq>` in `/conf/config.xml` (`host`,
`domain int.leighhack.org`, `ip`, `hwaddr`, `set_tag`), and three things have to
agree or it silently does nothing:

- **The IP must be in the subnet of the VLAN the device is actually on**, and
  **below that VLAN's dynamic pool** (pools start at `.100` on WIFI/`opt1`/
  `vlan0.226` = 10.3.2.0/24 and at `.150` on SHARED/`lan` = 10.3.1.0/24 and
  WIRED/`opt2` = 10.3.14.0/24, so every reservation in use is ≤ `.99`). A
  reservation in the wrong VLAN is inert: the U1 sits on WiFi, so it is a
  10.3.2.x address even though the other 3D printers are wired.
- **`set_tag` must be that VLAN's dhcp tag** (`<dhcp_tags>`: SHARED
  `0f6ba0e7-…`, WIRED `58a01f56-…`, WIFI `7965a3d1-…`). It becomes the `set:` on
  the generated `dhcp-host=` line.
- Only `hwaddr` is needed: dnsmasq matches a MAC-only entry even when the client
  sends a client-id (`voron` and `snapmaker` both do, and both match).

Then apply it — **`pluginctl dns`**, which is `plugins_configure('dns')`: it runs
`dnsmasq_configure_do`, regenerating `/var/etc/dnsmasq-hosts` and
`/usr/local/etc/dnsmasq.conf` and restarting dnsmasq. `pluginctl -s dns` is *not*
the same command: `-s` is service mode, it exits 0 and changes nothing (check the
mtime of `/usr/local/etc/dnsmasq.conf` when in doubt). Verify with
`grep <name> /usr/local/etc/dnsmasq.conf /var/etc/dnsmasq-hosts` and a lookup.

Leases are 24 h, and dnsmasq only re-reads its config on restart, so a device that
already holds an address keeps it until it renews (≈12 h, NAKed into a rebind) or
reconnects — the reservation is already correct, the lease table just still shows
the old address.

## DigitalOcean zone `leighhack.org`

- The DO token is `DO_AUTH_TOKEN` in the shared env-file secret — `CONFIG.ENV_FILE`,
  rendered by sops-nix to `/run/secrets/env_file` (its shape is
  `machines/services1/secrets/.env.sample`); read+write scope. **Never print it or copy it
  off services1.**
- `*.int` names are CNAMEs → `nginx.int.leighhack.org`, managed by dns-sync.
- Public apps CNAME → `nginx.leighhack.org`; the apex points at GitHub Pages;
  `yunohost` → A 81.187.195.18.
- **Public names are added by hand** — dns-sync only manages the `*.int` CNAMEs.
  That is how two live vhosts ended up with no record at all
  (`docs/follow-ups.md`).
- The wildcard ACME cert covers `*.leighhack.org` **and** `*.int.leighhack.org`
  (DNS challenge), so a new vhost needs a DNS record and no cert work.
