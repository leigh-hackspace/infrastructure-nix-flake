# Router (gw), dnsmasq and DigitalOcean DNS

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: the router is
out-of-band (no flake config), never edit the auto-generated `dnsmasq.conf`, every
`*.int` vhost needs `just dns-sync-sync` after the switch (and `dns-sync-prune`
after a rename/removal), and never print or copy off the DO token. This file is the
detail: where the router keeps its state, which names legitimately resolve
elsewhere, and the manual fallback when `dns-sync` is unavailable.

## The router

OPNsense 26.7 on FreeBSD, **10.3.1.1**, managed out-of-band — nothing here is
declarative.

**That sentence is about the running box, and it has an expiry date.**
`machines/gw/` is the flake-managed replacement for everything in this file: dnsmasq
generated from `common/vlans.nix` and `common/dns-records.nix` instead of from
`<Interfaces>` XML, nftables from `common/config.nix` instead of the web UI, the
AAISP tunnel and the tailnet client. It is now **deployed on real hardware**
(10.3.1.254) but in **co-existence mode** — every address ends in `.254` rather than
`.1`, its DHCP and router advertisement are emitted commented-out and refused by
interface name, and it advertises no tailnet routes — so the OPNsense box is still
the router and everything below is still how the network actually behaves. Read
`machines/gw/README.md` before changing either side: it says which of these facts the
new config has inherited, which are still open, and what has to be read off the old
box before the two can swap places.

```bash
ssh -i ~/.ssh/agent-hop-key root@10.3.1.1        # root IS keyed on the router
```

Web UI: `https://firewall.int.leighhack.org` (→ 10.3.1.1:60443).

**Gotcha: the root shell is tcsh.** `2>` redirection fails, and grep patterns
containing `<` or `>` must be double-quoted. (The LAN dashboard
`network-info.int.leighhack.org` — `crates/network-status/` — ssh's here every 5 s
as `root@10.3.1.1` with the machine-hop key.)

## dnsmasq — the LAN resolver

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
