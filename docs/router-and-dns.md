# Router (gw), dnsmasq and DigitalOcean DNS

Moved out of `AGENTS.md` on 2026-10-14. `AGENTS.md` keeps the rules: the router is
`machines/gw/` (flake-managed, cut over 2026-10-21), never edit the generated
dnsmasq conf or hosts files, a new `*.int` vhost is a name in `common/int-dns.nix`
plus two deploys, and never print or copy off the DO token. This file is the detail:
what answers what and why, which names legitimately resolve somewhere other than the
front door, how a device gets its name, and where the public zone stops being the
network's business.

**The one-sentence version, because it changed on 2026-10-22:** `*.int.leighhack.org`
is *declared*, not synchronised. The router's DNS is three files generated from
`common/vlans.nix`, `common/dns-records.nix` and `common/int-dns.nix`; adding a record
is editing one of those tables and deploying gw. There is no tool to run, and there
was: `crates/dns-sync` ssh'd from services1 and appended lines to a mutable
`/var/lib/dnsmasq/names.hosts`, `just dns-sync-sync` was the step that made a vhost
resolvable, and forgetting it was silent. What is left of it is a dead file on the
router (`machines/gw/README.md`) and a few dozen CNAMEs in DigitalOcean that nothing
maintains.

## The router

`machines/gw/` — a flake-managed NixOS box at **10.3.1.1**, cut over from the OPNsense
appliance on 2026-10-21. Nothing about the running router is out-of-band any more:
dnsmasq is generated from `common/vlans.nix`, `common/dns-records.nix` and
`common/int-dns.nix`, nftables from `common/config.nix`, and the AAISP tunnel and
tailnet client live in `machines/gw/`.
Read `machines/gw/README.md` before changing it; `docs/gw-cutover-attempt-2026-10-21.md`
has the findings from the cutover and what is still open.

What still spoke the retired OPNsense box's language, and when it stopped:

- **network-status** (`crates/network-status/`) was the last tool doing it — ssh as
  `root` and OPNsense commands (`netstat`, `pfctl`, `ifconfig`, `top -b`). It was
  ported on 2026-10-21 alongside dns-sync, and then moved: it now runs **on gw** as
  a DynamicUser (`machines/gw/network-status.nix`), reads the local `/proc` and
  `/sys` (no ssh, no sudo, no key), and services1's nginx proxies
  `network-info.int.leighhack.org` to it at `http://10.3.1.1:8091`. It charts gw's
  interfaces, conntrack table, CPU, memory, load and TCP retransmits.
- **dns-sync** edited `gw`'s `/var/lib/dnsmasq/names.hosts` over `leigh-admin` +
  `sudo -n`, which is how a new `*.int` vhost got DNS. Deleted on 2026-10-22: the same
  names are `common/int-dns.nix`, rendered into
  `/etc/dnsmasq-int-vhosts.hosts` by `machines/gw/dns.nix` and pushed into dnsmasq by
  a `restartTriggers` entry. The crate, the `/etc/dns-sync` files, the pinned router
  host keys and the `just dns-sync-*` recipes are all gone.

```bash
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.1   # gw; root is NOT keyed there
```

**Gotcha: dnsmasq's unsolicited RAs stop after a `router-lan` link flap**, so clients
lose their IPv6 default when the 1200 s router lifetime expires. `systemctl restart
dnsmasq` after cabling; `machines/gw/README.md` has the checks.

**Historical: the retired OPNsense box's root shell was tcsh** (`2>` redirection fails,
grep patterns with `<`/`>` need double quotes). That box is gone; `gw` runs NixOS and its
root is not keyed — use `leigh-admin` and `sudo`. The LAN dashboard
`network-info.int.leighhack.org` (`crates/network-status/`) was the last thing ssh'ing
there as `root`; it now runs on gw and reads the kernel's `/proc` files directly.

## Which uplink each VLAN leaves by

**The policy: VLAN 225's IPv4 goes through the AAISP tunnel; every other VLAN's IPv4 goes
straight out of the AAISP hub with a second NAT; all IPv6, on every VLAN, goes through the
tunnel.** Read off the live box on 2026-10-21:

| Traffic | Egress | `ip route get 1.1.1.1 …` |
| --- | --- | --- |
| IPv4, `vlan225` | `ppp0`, the tunnel (`81.187.223.123`) | `mark 1` → `dev ppp0 table 100 src 81.187.223.123` |
| IPv4, every other VLAN | `router-uplink`, the hub, double NAT | `mark 2` → `via 192.168.20.1 dev router-uplink table 200` |
| IPv6, every VLAN | `ppp0` | one main-table `default dev ppp0`, and **no `ip -6 rule`** |
| tailnet exit-node traffic | `router-uplink` | `oifname "router-uplink" ip saddr 100.64.0.0/10 masquerade` |
| the router's own traffic | `router-uplink` | main-table `default via 192.168.20.1` |

Why it is split that way: the tunnel's overhead is only worth paying for the VLAN that
hosts the public services, because those need the public `/29` and the `/48`. The `/29`
(`81.187.195.17/29`) is routed to the AAISP line and its inbound packets arrive through
the tunnel, so VLAN 225 has to egress the tunnel for *both* directions — a reply routed
out of the hub would leave with a different source address than the client dialled and
the connection would die. Everything else is ordinary internet access, which double NATs
fine.

Mechanism, in the order a packet meets it (`machines/gw/`): `firewall.nix`'s
`vlanMarkRules` sets `meta mark 1` for `vlan225` and `2` for all others (in `prerouting`,
before the route decision, and skipped for destinations inside the network —
`markLocalV4`); `networking.nix` installs the two `ip rule`s and the two tables, with
table 100's default written by `uplink.nix`'s `ip-up` script only while the tunnel is up;
and `routerNat`'s `srcnat` chain masquerades per egress interface, so LAN-to-LAN traffic
keeps its real source address. IPv6 needs no mark: `ipv6-up` writes the single default
and there is no v6 policy rule, which is also why **a dead tunnel is a network with no
IPv6 at all** (the RAs are still advertised, so clients keep their addresses and lose the
route — `machines/gw/README.md`, the RA-timer section).

IPv6 is offered by RA on the VLANs that own addressing: 225, 226 and 227 each advertise a
public `/64` (225 and 227 also a ULA). VLAN 222 has no range at all and VLAN 1337 is the
CS network's block, IPv4-only on purpose — `common/vlans.nix` is the table.

What proves it, rather than the comments:

```bash
ip rule                                    # fwmark 0x1 → 100, 0x2 → 200
sudo ip route show table 100               # default dev ppp0
sudo ip route show table 200               # default via 192.168.20.1 dev router-uplink
ip -6 route show default                   # default dev ppp0, one route, main table
sudo nft list chain ip routerNat srcnat    # one masquerade per egress
sudo nft list chain ip router prerouting   # the per-VLAN marks
ping -I 10.3.2.1 1.1.1.1                   # a non-225 source, out of the hub
```

The failure this is often mistaken for is not routing at all: "only VLAN 225 has no
internet" is the PPP password (`machines/gw/README.md`), because 225 is the one VLAN
whose path differs.

## dnsmasq — the LAN resolver

The resolver is `services.dnsmasq` from `machines/gw/dns.nix`, and everything it
answers comes from three generated store paths:

| File | Answers | Table it is generated from |
| --- | --- | --- |
| `/etc/dnsmasq.d/network.conf` | DHCP ranges, reservations, RA ranges, the netboot boot-file variants | `common/vlans.nix`, `common/dns-records.nix` |
| `/etc/dnsmasq-records.hosts` | each device's own name, bare and `<name>.int.leighhack.org` | `common/dns-records.nix` |
| `/etc/dnsmasq-int-vhosts.hosts` | each `*.int.leighhack.org` vhost name, one line per address of the front door | `common/int-dns.nix` |

`domain = int.leighhack.org` and `local = "/int.leighhack.org/"` make this box the
authority for the zone, `no-resolv` stops it forwarding its own zone back to itself,
and both hosts files are `addn-hosts` entries and `restartTriggers` values — so a
record and the restart that makes dnsmasq notice it arrive in the same activation.
(`/var/lib/dnsmasq/names.hosts`, the fourth file, is dead: see
`machines/gw/README.md`.)

### What `*.int` answers with, and why it is that address

**10.3.1.20 is services1, and services1's nginx is the network's front door.** Every
`*.int.leighhack.org` vhost is a `server_name` in that one nginx (`machines/services1/
http.nix` and `machines/services1/services/*.nginx`-shaped modules), which terminates
TLS on the wildcard cert and proxies to the box that actually runs the thing —
`grafana.int` to Grafana, `filestore.int` to filestore, `alexandria.int` to aibox:4200,
`network-info.int` to gw:8091. So a `*.int` name is not a pointer to a host, it is a
pointer to *a vhost*, and the vhost lives on services1: the answer is services1's
addresses, and `common/int-dns.nix`'s `frontDoor = "services1"` says which record in
`common/dns-records.nix` those come from. Three of them, one line each — the IPv4
(`10.3.1.20`), the public IPv6 and the ULA — because a name missing its IPv6 half is a
dual-stack client that hangs on IPv6 before it falls back, and because dnsmasq's hosts
syntax cannot put two addresses on one line (the second field onwards *is names*).

That is also why the front door's addresses are never written down again: they are read
out of the reservation table, the same three strings `common/hosts.nix` and the nginx
proxies use.

**One field says which box that is, and it can say `gw`.** `frontDoor` names either an
entry of `common/dns-records.nix` (`services1`, today) or the router — and the router has
no reservation and cannot have one, because it is the box handing out leases, so its three
addresses are read from `common/vlans.nix`'s VLAN whose `ipv4` is `hosts.router`
(`machines/gw/lib/network-tables.nix`'s `doorAddresses`). Nothing else in the tree notices:
the answers move, each box's guard reads the same string and changes mode, and
`machines/gw/lib/check-network-tables.nix` refuses a `frontDoor` that resolves to no
addresses — an empty hosts file is not an error to dnsmasq, it is a zone that answers
nothing. `docs/router-front-door-migration.md` is the plan; as it writes, not applied.

**The names that answer with something else, and why they are exceptions:**

- **A device answers its own name.** Every entry in `common/dns-records.nix` answers
  `<name>.int.leighhack.org` with its own address (`aibox.int` → 10.3.1.32, `nas2.int`
  → 10.3.1.6, `cam1.int`, `voron.int`, the PI-ROOM boards…), and a lease's own hostname
  answers inside the zone too. The front-door table therefore *must not* contain a
  device name, and `machines/gw/lib/check-network-tables.nix` refuses the pair if one
  appears in both: dnsmasq unions every `addn-hosts` file, so one name in two files is
  one name with two answers.
- **`authentik.int` is served by the appliance**, 10.3.1.36, even though nginx also
  serves the name (as an alias of `id.int`). `common/int-dns.nix`'s `exempt` list is
  where that is written down, with the reason: the switches' RADIUS and pi-room-sys's
  LDAP dial `ldaps://authentik.int.leighhack.org` directly and the appliance's `:443`
  is not fronted. Which answer *should* win is an open decision
  (`docs/handover-2026-10-21.md`); what is no longer open is that the table says which
  one does.
- **`switch1`/`switch2`/`switch3` answer themselves** (`common/hosts.nix`) without being
  DHCP clients, so they have no reservation to hang a record on — that is what
  `fromHosts` in the same table is for.
- **Five names answer at the front door and are served by no vhost**: `nginx.int`,
  `mqtt.int`, `tailscale.int`, `gitlab.int`, `ldap.int`. They are `legacy` in the
  table, each with its reason, and they were kept rather than dropped so that the
  deploy which made DNS declarative changed no answer. `nginx.int` is the one that
  cannot go: it is the CNAME target of every public `*.int` record in DigitalOcean.
  `gitlab.int` is a redirect vhost whose container is disabled; `mqtt.int` and
  `ldap.int` have never been HTTP-fronted and 404; `tailscale.int` predates the
  headscale UI's real name (`tailscale.leighhack.org`). The drift guard is what keeps
  `legacy` from quietly becoming where you park a typo: a name in it is a name
  *nothing serves*, by definition.

### Adding, moving, removing a name

| You are adding… | You edit | Then |
| --- | --- | --- |
| an internal app (a `*.int` vhost) | the vhost in `machines/services1/services/<app>.nix`, **and** its label in `common/int-dns.nix`'s `names` | deploy services1 and gw. The two lists are compared at eval (`machines/services1/int-dns.nix`), so forgetting the second is a failed deploy, not a silent NXDOMAIN |
| a device with a fixed address | `common/dns-records.nix` (its MAC or client-id, its address, its VLAN) | deploy gw |
| a device that is not a DHCP client but whose name must resolve | `common/hosts.nix`, then its label in `common/int-dns.nix`'s `fromHosts` | deploy gw |
| a name the front door serves but does not answer | `common/int-dns.nix`'s `exempt`, with the reason | deploy gw |

Nothing in this list is a command that edits a running box. `nix eval --impure --raw -f
machines/gw/lib/check-network-tables.nix` (run by `just check`) proves the tables agree
with each other, and `just switch-gw` is the whole "sync".

### Reading what the router answers

```bash
dig +short @10.3.1.1 <name>.int.leighhack.org A AAAA  # there is a dig on gw (bind is
getent ahosts <name>.int.leighhack.org                # in its systemPackages; the
sudo -n grep <name> /etc/dnsmasq-int-vhosts.hosts     # older claim that there is not
sudo -n grep <name> /etc/dnsmasq-records.hosts        # is stale, and getent works too)
sudo -n journalctl -u dnsmasq -n 30
```

Three states worth recognising in that output, because each has its own cause:

- **Nothing at all, and the name is a vhost** — the deploy that added the vhost did not
  deploy gw, or `common/int-dns.nix` disagrees with nginx and services1 refuses to
  build. `cat /etc/int-dns/served-int-names` on the box that terminates the name and
  `dig <name>.int @10.3.1.1` are the two halves of that diff. Both files render the same
  list from the same table, so once `frontDoor` says `gw` the box to `cat` on is the
  router, and `machines/gw/int-dns.nix` is the guard that refuses the mismatch.
- **An A and no AAAA for a front-door name** — impossible from the generated file
  (`intHosts` emits one line per address of the record, and the check module counts the
  lines per name), so look for a hand edit or a stale `names.hosts` that something was
  pointed back at.
- **A device name answering with 10.3.1.20** — the name is in *both* tables. dnsmasq
  returns both answers and some client somewhere picks the wrong one; the check module
  refuses this combination rather than documenting it.

### The fourth source of answers, and the one that surprises people

dnsmasq reads **`/etc/hosts`** as well as its `addn-hosts` files, so on gw anything in
`networking.extraHosts` is published to the whole network. That is where
`gw.int.leighhack.org` → 10.3.1.1 comes from (`machines/gw/networking.nix` puts it there
so the router can resolve the names it serves without asking itself) — it is in neither
hosts file, and `dig @10.3.1.1 gw.int` answers anyway.

The mirror-image trap is on the other boxes: `machines/services1/networking.nix` has
`extraHosts` lines of its own (`id.int`, `unifi.int`, `tailscale.leighhack.org`), and
those are **local to services1**. So a gatus check or a systemd unit on services1 can
resolve a name that no client on the LAN can, and nothing in the flake's DNS tables
mentions it — `unifi.int` is the example: services1 resolves it at 10.3.1.21 because of
its own `/etc/hosts`, and the router answers NXDOMAIN for everyone else. Before adding a
`*.int` name to a `proxyPass`, a `ldap_uri` or a check URL, ask which of the two is
answering: `dig @10.3.1.1` for the network, `getent` on the box for that box.

### Historical: the OPNsense box's dnsmasq

Kept because the tables were read out of it and because "why is the generated file
shaped like *that*" has this answer. `/conf/config.xml` was the source of truth (host
overrides under `<opnsense><dnsmasq><hosts>`, reservations as `<hosts>` entries with a
`set_tag` per VLAN), `/var/etc/dnsmasq-hosts` was the generated addn-hosts dump that
answered queries, and `/usr/local/etc/dnsmasq.conf` was generated — never edited. The
`*.int` names in it carried three addresses each (v4, public v6, ULA), which is the
shape `common/int-dns.nix` reproduces, and the file restored onto gw from it on
2026-10-21 (`/var/lib/dnsmasq/names.hosts`) is the file this section replaces.

## DHCP reservations

**`common/dns-records.nix`.** One entry per device that keeps an address: its `vlan`,
`ipv4`, `ipv6`, `macs` and `clientIds`, and the key is the name it answers.
`machines/gw/dns.nix` renders the reservations and the DHCP lines from it,
`recordsHosts` renders its names into the hosts file, and
`machines/gw/lib/check-network-tables.nix` refuses a table that would render something
dnsmasq cannot load — a reservation naming a VLAN that does not exist, two devices on
one address, a range outside its VLAN's block, a record with no MAC and no client-id
(which renders as no line at all), a router address inside a dynamic range, a name
answered from two tables.

Leases are 24 h and dnsmasq re-reads its config only on restart, so a device already
holding an address keeps it until it renews (≈12 h) or reconnects. The reservation is
already correct; the lease table is just behind.

This is why `common/hosts.nix` holds **addresses, not names**: every machine in this
network is a DHCP reservation, and a table that resolved through the resolver it is
meant to be independent of would not be one.

### Historical: the retired appliance's method

A reservation there was a `<hosts>` entry under `<dnsmasq>` in `/conf/config.xml`
(`host`, `domain int.leighhack.org`, `ip`, `hwaddr`, `set_tag`), and three things had to
agree or it silently did nothing:

- **the IP had to be in the subnet of the VLAN the device is actually on, and below that
  VLAN's dynamic pool** (pools started at `.100` on WIFI and at `.150` on SHARED and
  WIRED, so every reservation in use is ≤ `.99`). A reservation in the wrong VLAN was
  inert: the Snapmaker sits on WiFi, so it is a 10.3.2.x address even though the other
  3D printers are wired.
- **`set_tag` had to be that VLAN's dhcp tag** (`<dhcp_tags>`), which became the `set:`
  on the generated `dhcp-host=` line. On `gw` the tag is the interface name, and
  `machines/gw/lib/network-tables.nix` generates it from the record's `vlan` — one of
  the ways a per-VLAN fact stopped being typeable wrong.
- only `hwaddr` was needed: dnsmasq matched a MAC-only entry even when the client sent a
  client-id (`voron` and `snapmaker` both do, and both matched).

Applying it was `pluginctl dns` (`plugins_configure('dns')`), **not** `pluginctl -s dns`
— the latter is service mode, exits 0 and changes nothing.
## DigitalOcean zone `leighhack.org`

The public zone is **not** the network's DNS and nothing in this repo writes it any
more. It is the answer a client gets when it is not on the LAN, and off the LAN the
`*.int` answers are private addresses — so treat it as a list of hand-made records for
public names, plus a pile of `*.int` CNAMEs that are still load-bearing for anything
resolving over public DoH and are maintained by nobody.

- The DO token is `DO_AUTH_TOKEN` in the shared env-file secret — `CONFIG.ENV_FILE`,
  rendered by sops-nix to `/run/secrets/env_file` (its shape is
  `machines/services1/secrets/.env.sample`); read+write scope. **Never print it or copy it
  off services1.** Reading the zone without printing the token:

  ```bash
  ssh -A leigh-admin@10.3.1.20 'sudo -n sh -c ". /run/secrets/env_file; curl -sS -m 20 \
    -H \"Authorization: Bearer \$DO_AUTH_TOKEN\" \"
    https://api.digitalocean.com/v2/domains/leighhack.org/records?per_page=200\""'
  #  -o a file and jq it; never put the token in a command line you will keep, and
  #  note the payload key is `domain_records`, not `records`.
  ```

- **Public apps CNAME → `nginx.leighhack.org`; the apex points at GitHub Pages;
  `yunohost` → A 81.187.195.18.** Public names are added by hand, and that is how two
  live vhosts once ended up with no record at all (`docs/follow-ups.md`).
- **A new `*.int` vhost needs no record here.** The LAN gets it from the router
  (`common/int-dns.nix`) and the tailnet gets it from headscale's split DNS
  (`machines/services1/services/headscale.nix`: `int.leighhack.org` → 10.3.1.1, with
  `int.leighhack.org` as a search domain), which is what the zone's ~40 `*.int` CNAMEs
  → `nginx.int.leighhack.org` were for before a tool was written to maintain them.
  They are left in place because deleting forty working records to prove a point is not
  a trade; `docs/follow-ups.md` carries pruning them, and the one-record wildcard
  alternative, with its caveat.
- **The wildcard ACME cert covers `*.leighhack.org` and `*.int.leighhack.org`**
  (`machines/services1/http.nix`, `dnsProvider = "digitalocean"`), so a new vhost needs
  no cert work. It also means the zone's `_acme-challenge*` names are load-bearing on a
  60-day timer: anything that puts a record *under* `_acme-challenge` — a `*.int` CNAME
  wildcard being the tempting one — has to be tested against a real renewal, not
  reasoned about. That is why the wildcard is a follow-up and not tonight's change.
- **DigitalOcean stores record names as labels, not FQDNs.** Create a record named
  `gatus.int.leighhack.org` and you get `gatus.int.leighhack.org.leighhack.org`, which
  answers nothing. `gatus.int` was exactly that — resolvable on the LAN, missing to the
  world — and is **fixed (2026-10-22)**: the CNAME `gatus.int` → `nginx.int` exists now
  (ttl 3600, like its neighbours, verified against `@1.1.1.1`) and the doubled record is
  deleted. Two such records are still in the zone: `AAAA yunohost.leighhack.org` and the
  `_cs-verify` TXT named `leighhack.org` (`docs/follow-ups.md`). Also learned the hard
  way: the API requires a CNAME's `data` to end in `.` on **create** while it stores it
  without one, so the value you read back is not the value you post.
- Anything scripted against this zone should read it back and assert no stored name ends
  in `.leighhack.org` — that is how the three were found.
