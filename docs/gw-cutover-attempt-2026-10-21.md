# The first cutover attempt (2026-10-17), and what it says to do differently

The router (`machines/gw/`) was cut over live — `machines/gw/coexist.nix` set to
`enable = false`, a generation built, the box rebooted in place of the OPNsense box at
`10.3.1.1` — and rolled back within about three minutes because "the WAN did not come
up". The box was then taken off the trunk and put on aibox's second NIC, where it has
sat since, still configured as the cut-over router (`.1` on every VLAN, tunnel unit
present) but with nothing plugged into its uplink.

This is what the journal actually says, read off the box on 2026-10-21
(`journalctl -b cb96b3de7a994d6cb0fcb2a8f1a2fcb6`, the cut-over boot; and the bench
boot `00c294a2d5bc44f6b49354e7fa673012` for the current state).

## The uplink was not the failure

Everything on the WAN side came up, in order, within 20 seconds:

```
router-uplink: DHCPv4 address 192.168.20.82/24, gateway 192.168.20.1 acquired
xl2tpd: Connection established to 194.4.172.12, 1701.  Local: 3604, Remote: 12065
xl2tpd: Call established with 194.4.172.12 ... start_pppd
pppd: CHAP authentication succeeded
pppd: local  IP address 81.187.223.123
pppd: remote IP address 81.187.81.187
pppd: local  LL address fe80::a875:8f59:ba99:c726
pppd: remote LL address fe80::9e89:1eff:fe3f:0000
```

The uplink NIC took a lease from the AAISP hub, the L2TP call was established, CHAP
was accepted, and the line came up holding the network's public address. The PPP
session then stayed up for the rest of the boot: no LCP echo failure, no redial. **The
tunnel is not what failed, and nothing in `machines/gw/` needed changing for it.**

> **Update, 2026-10-21 (after the cutover).** That verdict was about the *control
> plane*, and it was right about that — but the tunnel has never been shown to pass a
> packet, and on the cut-over box it does not. `ppp0` up, CHAP accepted, table 100's
> default installed and the masquerade rules in place all held, while every packet
> inside the tunnel came back `Destination Net Unreachable` from `194.4.172.12`, and
> `pppd` had logged `CHAP authentication succeeded: Authentication failure`. The cause
> was the secret, not the config: `aaisp_ppp_password` held OPNsense's base64-encoded
> `<password>` **verbatim**, so the box dialled with 36 bytes of base64 instead of the
> 25-byte password. `machines/gw/README.md` ("Only VLAN 225 has no internet") has the
> symptom, the tell and the fix. Read "the uplink is proven" below as "the uplink is
> installed".

## What did fail: dnsmasq could not start at all

```
dnsmasq-pre-start: dnsmasq: duplicate dhcp-host IP address 10.3.1.16
  at line 30 of /etc/dnsmasq.d/network.conf
dnsmasq: FAILED to start up
```

`dnsmasq.service` crash-looped for the whole boot (restart counter 35 by the time the
box was switched off). With it went **DNS for the entire network and all DHCP**. Every
client kept its old lease from the OPNsense box and lost name resolution, so from any
machine on the LAN — including from `gw` itself, whose `/etc/resolv.conf` points at its
own dnsmasq — the network was down. That is the outage that caused the rollback.

### Why it had never been seen

`machines/gw/coexist.nix` emits the DHCP blocks **commented out**. In co-existence mode
dnsmasq therefore never parsed the `dhcp-host` lines, and the box was healthy for four
days. `enable = false` un-comments them for the first time — so the first cutover was
the first time dnsmasq had ever read the `dhcp-host` half of the generated file.

### The rule, measured

dnsmasq 2.93 refuses **any two `dhcp-host` lines naming the same IPv4 address**, and it
does not matter whether they differ by MAC, by client id or by hostname:

```
dhcp-host=44:a8:42:3f:a1:ed,10.3.1.16,H              \
dhcp-host=id:01:44:a8:42:3f:a1:ed,10.3.1.16,H        > duplicate, refuses to start
```

Multiple selectors on **one** line are fine (`dhcp-host=<mac>,<mac>,id:<client>,<ip>,<name>`
— verified with `dnsmasq --test`), and a second line carrying only an *IPv6* address for
the same host is fine too. `machines/gw/lib/network-tables.nix`'s `hostLines` emitted one
line per *identifier*, so every record with a MAC **and** a client id (aibox, services1,
yunohost, Henry-iDrac, every PI-ROOM board) produced two lines for one address.

Fixed by rendering one line per record with all its selectors, and
`machines/gw/lib/check-network-tables.nix` now asserts that each record's IPv4 appears on
exactly one `dhcp-host` line — the invariant dnsmasq actually enforces, rather than the
count of identifiers, which was what the old assertion checked and which passed while the
router could not boot its resolver.

## The other things a cutover needs, which the flake does not have

None of these says "the WAN failed"; all of them say the cutover was attempted before the
old box's WAN-facing behaviour had been reproduced. They are the reason a switch-over
today would still take public services down even with dnsmasq fixed.

1. **No inbound NAT at all.** There is no `dnat` anywhere in `machines/gw/`; the forward
   chain is `policy drop` with an allow-list for traffic *from* the LAN. The old box's
   `pfctl -sn` archive (`~/tmp/opnsense-archive-20261017/pf.txt` on services1, 128 `rdr`
   lines) forwards, on the AAISP block **81.187.195.17/29**:

   | public | → | internal | what it is |
   | --- | --- | --- | --- |
   | 81.187.195.17:80, :443 | → | 10.3.1.20 (services1) | every `*.leighhack.org` vhost, and `tailscale.leighhack.org` |
   | 81.187.195.17:3478, :50443, :8522 | → | 10.3.1.20:3479/50443/8522 | matrix/livekit, etc. |
   | 81.187.195.17:51820 (tcp) | → | 10.3.1.6 (nas) | |
   | 81.187.195.18:80, :443 | → | 10.3.1.15 (yunohost) | |

   *Post-cutover audit, 2026-10-21: the flake now forwards only 80/443 and
   3478/udp (headscale STUN) — 8522, 50443 and 51820 were removed because nothing
   listened on the first two and the third is a VPN rather than tailscale. The
   table above is what the old box ran, not the live policy; the live policy is in
   `machines/gw/firewall.nix` and rendered by `nft list chain ip routerNat dstnat`.*

2. **No NAT reflection**, which is why the box cannot reach its **own** public names —
   including its headscale control server. `tailscaled-autoconnect.service` fails on every
   boot with `dial tcp 81.187.195.17:443: connect: network is unreachable` — that address is
   its own line's, and nothing hairpins it back to services1. This is pre-existing (it
   failed in co-existence mode too, against the OPNsense box) but the cutover inherits it.

3. **DHCPv6-PD was never observed.** `systemd-networkd` configured `ppp0` from
   `10-tunnel.network` and gained its IPv6LL, then logged nothing: no `DHCPv6` client
   activity, no delegated prefix, in the ~3 minutes before the box was switched off. Until
   that is seen working, **VLAN 226 and 227 have no public IPv6**, and the `/48` the
   network is meant to serve is absent. `networking.nix` asks for `::/48` with the VLAN id
   as `SubnetId`; whether AAISP answers must be watched live (`networkctl status ppp0`,
   `ip -6 route`), not assumed.

4. **One unexplained routing event.** Two minutes after the tunnel came up,
   `tailscaled` logged `connect: network is unreachable` for a public address, i.e. the
   main table's default route had gone. There is no `reject` and no output chain in the
   ruleset, so this was a real route removal, and the only thing managing those routes is
   `systemd-networkd` on `router-uplink` — most likely the hub's link/lease went away while
   cables were being moved. It is worth not repeating the mistake of not looking.

## What to do before trying again

The lesson is not "the router does not work" — the uplink proved itself. It is **prove
each half in isolation, on the bench, before the box is the network's router.** The
bench setup that made this write-up possible is also the best test rig:

1. `just check` on the flake, then switch the box on the bench and confirm (the fix in
   this repo is already deployed and verified there: dnsmasq running, RA on
   vlan225/227, DNS answering FQDNs, a DHCP DISCOVER getting `10.3.1.221` with router and
   DNS `10.3.1.1`).
2. With the box still off the network, **plug the AAISP hub into its uplink NIC** and let
   the tunnel dial. Then check, in this order: uplink lease, `pppd` public address,
   `ip -6 route`/`networkctl status ppp0` (the delegated prefix), outbound from the box,
   and forwarded traffic from a single test client on the bench NIC (mark → table 100 →
   `masquerade` on `ppp0`).
3. Decide about the port forwards (item 1 above) **before** the swap. Without them, the
   cutover window takes every inbound service down, and the window is however long it
   takes to write them.
4. The rollback is the OPNsense box: leave it powered, cable it back, and give the leaning
   staff a rule about not running two DHCP servers on one VLAN in the meantime.

## Attempt 2, same day (16:59–17:00): the LAN and the tunnel both came up

A second attempt was made later the same day, this time with the trunk *and* the hub on
the new box. It was reverted after about 80 seconds. The journal of that boot
(`00c294a2d5bc44f6b49354e7fa673012`, the box was never rebooted) shows the following,
and it is worth reading carefully because it clears most of the suspects:

| | |
| --- | --- |
| 16:59:14 | `router-uplink: Gained carrier` — the AAISP hub is plugged in |
| 16:59:19 | `DHCPv4 address 192.168.20.82/24, gateway 192.168.20.1 acquired` |
| 16:59:20 | clients on VLAN 226 are already resolving through dnsmasq (it forwards to `1.1.1.1`/`8.8.8.8` and gets answers, so the box's own path to the internet works through the hub) |
| 16:59:22 | `DHCPACK(vlan226) 10.3.2.152` … i.e. **the LAN trunk was on `router-lan` and dnsmasq was serving leases** |
| 16:59:27 | `pppd: CHAP authentication succeeded`, `local IP address 81.187.223.123` |
| 16:59:27 | `RTM_NEWROUTE … table=100` — the tunnel's default route **was** installed by the ip-up script |
| 16:59:27 | tailscaled joins the tailnet: `tailscale0:[100.64.0.28/32 …]`, DERP connected |
| 17:00:37 | `vlan226/227/1337: Lost carrier` — the LAN cable is pulled (giving up) |
| 17:00:42 | `router-uplink: Lost carrier`, `DHCP lease lost`, `ip rule … Table:200` removed |

So on attempt 2: DNS and DHCP served the LAN, the uplink took a lease, the tunnel dialled
and authenticated, the public address was assigned, the policy route for the tunnel went
in, the NAT table's two `masquerade` rules are present, and the tailnet joined. The
plumbing that attempt 1 left unproven is now proven installed.

**What is still missing is IPv6 prefix delegation.** There is no `DHCPv6` activity at all
in the window — no SOLICIT, no prefix — and 70 s after `ppp0` came up `vlan226` still
held only its link-local address (`vlan226:[10.3.2.1/24 fe80::…]` in tailscaled's
LinkChange dump). The consequence is not cosmetic: `dns.nix` advertises the *public* RA
range for every VLAN from `common/vlans.nix`'s `ra` list, so WiFi clients on VLAN 226 are
handed addresses in `2001:8b0:1d14:226::/64` — a prefix this router neither has an
address in nor routes — and a default route via a gateway that cannot reach it. That is
the failure mode `machines/gw/networking.nix` warns about in its own comment ("clients
that configure an address and cannot reach their gateway"), and for a phone or laptop it
looks exactly like "the internet is down" even though IPv4 is fine.

The old box did not have this problem because its DHCPv6 client actually worked:
`~/tmp/opnsense-archive-20261017/ppp-ipv6.txt` on services1 shows `interface l2tp0 {
send ia-na 5; send ia-pd 5; }` with `prefix ::/48` and per-VLAN `sla-id 550`/`551`
(decimal — i.e. `0x226`/`0x227`, which is where `2001:8b0:1d14:226::/64` comes from;
systemd's `SubnetId=226` is hexadecimal too, so the flake's intent matches).

Two ways out, and the second is the one that makes a cutover safe today:

1. **Find out why systemd-networkd's DHCPv6 client never solicited** on `ppp0`. It needs
   the link to be up with an IPv6LL (it was), and AAISP to delegate. Next attempt: watch
   `networkctl status ppp0`, `ip -6 route`, and the journal for `DHCPv6`.
2. **Stop advertising a prefix the router does not have**, until PD is proven. Either
   give VLANs 226/227 their /64s statically (the old box's values are in the archive, and
   static + PD agree by construction), or drop the public RA ranges for them and let
   those clients be IPv4-only. Either is better than advertising a black hole.

## The 20-second check to run during the next attempt

The two attempts so far were reverted before anything could be measured, so each one
cost a full outage and produced a hypothesis. Run this from any LAN machine the moment
the new router comes up — it answers "is it the tunnel, the forwarding, or only IPv6?"
in one screen:

```bash
# 1. did the tunnel and the policy route come up?
ssh leigh-admin@10.3.1.1 'ip -4 addr show ppp0; ip route show table 100; ip rule show'
# 2. is there a delegated prefix (the thing that was missing both times)?
ssh leigh-admin@10.3.1.1 'ip -6 route show; ip -6 addr show vlan226; networkctl status ppp0 | tail -20'
# 3. does client forwarding actually work, and which way does it go?
curl -4 -m 5 -sS -o /dev/null -w 'ipv4 %{http_code}\n' http://1.1.1.1/
curl -6 -m 5 -sS -o /dev/null -w 'ipv6 %{http_code}\n' http://[2606:4700:4700::1111]/
# 4. and the same from the router itself, to separate "router" from "forwarding"
ssh leigh-admin@10.3.1.1 'curl -4 -m 5 -sS -o /dev/null -w "router-ipv4 %{http_code}\n" http://1.1.1.1/'
```

If (4) works and (3) does not, the fault is the forward/mark/NAT path. If only (2) is
empty, the fault is as above: IPv6 advertising, not the WAN.

## The cutover that was actually made (2026-10-21, evening), and what it broke

`machines/gw/coexist.nix` was left at `enable = false` and the box became the network's
router at `10.3.1.1`: the tunnel dialled (`81.187.223.123`), DHCP and DNS served, the
policy routes were installed, and the public services answered **over IPv6** and on the
LAN. Three things did not, and all three are consequences of the cutover rather than of
the tunnel:

1. **Every `*.int.leighhack.org` name went NXDOMAIN** (so `filestore.int` was "down"
   even though `curl --resolve` to `10.3.1.20:443` returned 200). The zone is
   `local=/int.leighhack.org/`, and the vhost names live in the mutable
   `/var/lib/dnsmasq/names.hosts`, which a fresh box creates **empty**. dns-sync is
   still OPNsense-shaped (it edits `/conf/config.xml` and `cat`s
   `/var/etc/dnsmasq-hosts`), and its pinned host key is the old box's, so it refused
   with `REMOTE HOST IDENTIFICATION HAS CHANGED`. Restored by copying the old box's
   `/var/etc/dnsmasq-hosts` (archived under `~/tmp/opnsense-archive-20261017` on
   services1) over `names.hosts` and `kill -HUP`ping dnsmasq; the durable fix is the
   dns-sync item in `docs/follow-ups.md`.

2. **No inbound NAT**, which is item 1 of "the other things a cutover needs" above and
   was still open. Every `81.187.195.17` / `.18` service timed out from the internet
   **and** from the LAN, because there was no `dnat` chain and no NAT reflection. IPv6
   was unaffected (no NAT66), which is why the same vhost answered at
   `2001:8b0:1d14:225::20`. Fixed in `machines/gw/firewall.nix`: a `dstnat` chain
   reproducing the old box's `rdr` table (`DMZ_SERVICES1` 80/443/8522/3478→3479/50443/
   51820 and `DMZ_YUNOHOST` 80/443), `ct status dnat accept` in the forward chain, and
   a hairpin `masquerade` for LAN-sourced translated packets. `DMZ_NET` also had to join
   `markLocalV4`, because the mark is set at priority `mangle`, i.e. *before* `dstnat`:
   without it a LAN client's packet to `81.187.195.17` was marked for the hub and sent
   out the uplink instead of back onto the VLAN.

3. **dnsmasq stopped sending unsolicited RAs** after `router-lan` lost carrier at
   17:14:30 (the cutover cabling). Solicited RAs still worked, so a probe made everything
   look fine, but clients lost their IPv6 default when the 1200 s router lifetime ran
   out — 20 minutes later — and a dual-stack name then hung on IPv6 before falling back
   to IPv4. That is the "WiFi hangs, wired is fine" report. `systemctl restart dnsmasq`
   restored the periodic advertisements; the trap and its checks are in
   `machines/gw/README.md`, "The third way dnsmasq breaks the LAN silently".

One failure that is **not** the router: `yunohost.leighhack.org` presents an **expired**
Let's Encrypt certificate (`CN=yunohost.leighhack.org`, `notAfter=Oct 21 16:59:23 2026
GMT`). The DNAT sends it to the right place (`10.3.1.15`, where port 80/443 are open and
the renewing HTTP-01 challenge can now reach it again); the certificate has to be renewed
on the Yunohost appliance, which is out-of-band and not keyed by the machine-hop key.

Verified after the fixes, from services1 (a LAN client, so the public addresses exercise
the hairpin): `filestore.int` 200 over IPv4 and IPv6, `id.leighhack.org` 302 over both,
`tailscale.leighhack.org` 301, `yunohost.leighhack.org` 302 (`-k`, because of the
certificate above).
