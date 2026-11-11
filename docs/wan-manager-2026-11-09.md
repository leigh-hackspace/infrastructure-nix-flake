# The WAN manager — discover, rank and relentlessly repair the router's uplink

**What this is:** `crates/wan-manager` + `machines/gw/wan-backup.nix`. It replaces the
2026-11-07 backup-WAN module (now archived at
[`docs/archive/wan-backup-uplink-2026-11-07.md`](archive/wan-backup-uplink-2026-11-07.md)).
That module worked, but every fact in it was a name in the configuration: one Wi-Fi
interface, one USB netdev, one hub gateway, one list of fallback paths, and a 10-second
shell reconciler whose liveness was a separate timer. This one takes **no WAN name,
address or gateway at all**. It discovers what is there, measures it, and keeps doing so.

## The rule that created it

- No WAN interface, address, gateway or metric is hardcoded in the firewall or in
  `machines/gw/networking.nix`.
- The firewall NATs an nftables interface **set** (`wan_ifaces`) that the daemon fills.
- The daemon runs as early as systemd allows, with `Restart=always`,
  `StartLimitIntervalSec=0`, `Type=notify` and a watchdog; a tick that panics does not
  stop the loop, and a daemon that stops petting the watchdog is restarted.
- It never assumes "the WAN is configured". Every tick re-discovers, re-probes,
  re-ranks, re-installs the routes and re-fills the NAT set.

## What it tries, every tick

| Source | How it is found | How it is addressed |
| --- | --- | --- |
| **Direct NIC / hub** | any non-LAN Ethernet interface that offers a DHCP lease | the daemon writes a networkd `.network` file (`DHCP=ipv4`, `UseRoutes=yes`, high metric) and reads back the gateway the server offered. No `192.168.20.1` anywhere. |
| **USB-tethered phone** | any Ethernet netdev whose device path is on the USB bus — Android RNDIS, CDC-ECM, CDC-NCM, iPhone CDC-NCM all present the same shape | same DHCP path; no MAC, no port name, no driver name |
| **USB Wi-Fi adapters — any number, any brand** | every netdev with `/sys/class/net/<if>/wireless` (or `phy80211`) | one `wpa_supplicant` per interface, each scanning for the organisation's SSID, each ranked separately |
| **Wi-Fi APs for that SSID** | `iw dev <if> scan`, every `BSS` carrying `Leighhack WAN Backup` | the scan is sorted by signal; a chosen BSSID is pinned and rotated on repeated association failure |
| **AAISP tunnel** | `ppp0` when pppd has made it | its table-100 default is verified, and removed when the tunnel exists but carries nothing |

The SSID is the one WAN *identity* in the flake (`machines/gw/wan-backup.nix`). It is an
organisation name, not an interface; the passphrase is `wan_backup_wifi_psk` in sops and
is read at use time, never put in a store path or an argv.

## Viability and ranking

A non-tunnel path is eligible only when **its own gateway answers** and an internet anchor
answers *through that interface*. The gateway probe is the 2026-11-07 lesson: a link with a
lease and a carrier is not a link that carries packets. Two anchors (`1.1.1.1`, `8.8.8.8`)
mean one blocked ICMP destination is not an outage.

The score is measured throughput (dominant), latency, a small class tie-break (tunnel >
wired > USB tether > Wi-Fi), and a hysteresis bonus for the incumbent. Failover is
immediate; returning to a previous path needs the challenger to beat the incumbent by a
clear margin, because every switch rewrites the NAT source address of every open
connection.

Throughput is measured with a small HTTP GET through the interface (`curl --interface`),
not inferred from the negotiated rate. The 2026-10-31 camera measurement is the reason:
identical links in the same building differed by 22× because of the radio cell.

## Repair

- A selected USB or Wi-Fi path whose own gateway has been silent for **12 ticks (60 s at
  the 5 s interval)** is reset: the Wi-Fi supplicant is restarted and rotated to the next
  ranked BSS, the USB netdev is bounced and re-DHCPed.
- The daemon writes networkd files itself; a lease that moves a hotspot to another `/24`
  is followed on the next tick, and a deleted route or an emptied nft set is restored.
- If the flake's `ip routerNat` table or its `wan_ifaces` set is missing, the daemon makes
  a working NAT table rather than leaving the LAN with no internet. The next switch
  restores the declarative firewall.
- `/run/wan-manager/last-run` is written every tick; `/run/wan-manager/decision` is the
  last `<state> <egress> <tunnel>` line; `/run/wan-manager/status.json` is the page's
  source and `metrics.prom` is appended to the existing Prometheus scrape.

## Four faults met on 2026-11-09, and what each one actually was

All four were found by reading the box after a switch, and none of them is visible from
`just check`: they are interactions between a running tunnel, a restarted `networkd`, this
daemon's own liveness contract and one string parser.

**1. A `nixos-rebuild switch` flushed the AAISP tunnel's IPv4 address, and nothing said
so.** `switch-to-configuration` restarts `systemd-networkd`, and a restarted networkd
*configures* every link it matches. `ppp0` matches `10-tunnel.network`, so the address
pppd had negotiated (`81.187.223.123/32` by IPCP) and the routes `ip-up-aaisp` had
installed were removed — `tailscaled` logs it as one line (`ips ppp0:
[81.187.223.123/32 fe80::…]->[fe80::…]`). The L2TP session stayed up and authenticated, so
nothing redialled: no LCP failure, no `xl2tpd` restart, no event. The observable state was
a healthy `ppp0`, an empty table 100, no IPv6 default on the box at all, and this daemon
correctly reporting the tunnel as `no internet through this path` while the network fell
through to a USB Ethernet backup.

The fix is `KeepConfiguration = true` on `10-tunnel.network` (`machines/gw/networking.nix`):
networkd must not reconfigure a link whose IP owner is pppd. `KeepConfiguration` deliberately
comes *before* `linkConfig.Unmanaged = true` as the choice, because the file's
`routingPolicyRules` (mark 1 → table 100) are the declarative half of VLAN 225's policy
routing and an unmanaged link would not have them applied. PD has never worked on this box
and the per-VLAN public `/64`s are static in `common/vlans.nix`, so declining to re-request
the prefix on a relaunch loses nothing.

Two consequences worth keeping:

- **The regression test is a `systemctl restart systemd-networkd` on a box whose tunnel is
  up.** Afterwards `ip -4 addr show ppp0` must still show the public address, table 100 must
  still have its default and `ip -6 route show default` must still say `dev ppp0`. If a
  future systemd stops honouring the setting, that is the measurement to re-take.
- **A tunnel that is up with no address needs a redial, and nothing does that
  automatically.** The address is not recoverable from software once flushed — `pppd` has
  already finished IPCP. `systemctl restart xl2tpd-aaisp` redials and the address and the
  IPv6 default come back in seconds. (That is also why the emergency script restarts the
  session rather than editing routes.)

The daemon now reports this state as what it is — `no IPv4 address (the link is up but
IPCP has given it none)` — rather than probing for 20 s and concluding `no internet`:
`Probe::error` and `Candidate::reason` both carry it, and the Uplink tab prints it in the
candidate's verdict column.

**2. The watchdog was firing on a slow tick, not a wedged one.** `WatchdogSec=45s` with a
pet only at the end of a tick, while one tick probes every candidate serially — a dead path
costs three 2 s echoes twice and then a 10 s HTTP verdict, so two dead paths (the broken
tunnel and the USB dongle's own uplink) put a normal tick at 35-50 s. The daemon was killed
and restarted about every 90 s while doing exactly its job; the coredump even shows a
`curl` mid-probe, which reads as a hang and is not one. It now pets **between the steps of a
tick** (per candidate, per throughput sample, per wireless adapter, after the USB
refresh), so 45 s is the budget for one step and a wedge inside one is still caught.
`util::run` also gained `--kill-after=5s`: `timeout` alone sends one TERM and then waits,
so a child that ignores it held the daemon rather than being reported as bounded.

**3. `status.json` did not survive a restart, so the Uplink tab read "unavailable".**
systemd removes a unit's `RuntimeDirectory` when it stops, so every watchdog restart wiped
`/run/wan-manager/status.json` and `/api/wan` answered 503 for the ~40 s the first tick
takes to rewrite it. `RuntimeDirectoryPreserve = "restart"` keeps it across a restart and
still clears it on a deliberate stop. The page also stops replacing a good last tick with
the error: no data still means "unavailable"/"waiting", data plus an error means the last
tick plus a banner (`crates/network-status/frontend/src/wan.rs`).

**4. The daemon could not parse a point-to-point address, so the tunnel has always looked
addressless.** `net::ipv4_addresses` read `inet A.B.C.D/NN` and nothing else, but `ip -4 -o
addr show dev ppp0` prints `inet 81.187.223.123 peer 81.187.81.187/32` — the local half has
no prefix, so `split_once('/')` found nothing and ppp0's address list came back empty while
the tunnel was up and carrying the network. It only cost the dashboard's address column
until the "no IPv4 address" check above was added, at which point it made a healthy tunnel
unselectable *and* the daemon deleted table 100 after three failed ticks, dropping VLAN
225's IPv4 onto a backup (measured within minutes of the first deploy: `curl -4
v4.ident.me` from `services1` answered with the hotspot's address, not AAISP's). The parser
now takes the local field and the peer's mask, is a pure function, and has a unit test for
both shapes (`net::parse_ipv4_addrs`).

Re-verify all four in one pass, on the box, with the tunnel up:

```bash
ip -4 -o addr show dev ppp0          # inet 81.187… peer …/32  (and the daemon sees it)
ip -4 route show table 100           # default dev ppp0
ip -6 route show default             # default dev ppp0
sudo systemctl restart systemd-networkd && ip -4 addr show dev ppp0   # survives
curl -s http://10.3.1.1:8091/api/wan | jq -c '.tunnel.interface, [.candidates[]|{i:.interface,reason}]'
ssh services1 'curl -4 -s https://v4.ident.me'   # must be 81.187.223.123
```

## A fifth fault, measured 2026-11-11: the daemon's own route churn

**The report was "still getting lost TCP connections", and this time the cause was the
daemon's route writes — not the ranking.** `apply_routes` called `clear_ours()`
unconditionally, which **deleted** the selected path's main default, table-200 default and
L2TP `/32`, and then re-installed them with `ip route replace`. `replace` is atomic in the
kernel; delete-then-replace is not, and they are two separate processes.

Measured on `gw` (`sudo timeout 12 ip monitor route`), on a box that was otherwise healthy:
0.02 % retransmits in a spot sample, a silent 36 Mbit/s download, both USB paths with zero
errors, and `router-uplink` irrelevant because it had had no carrier since 10:25 that day.

```text
18:05:01.418  Deleted default via 192.168.1.1 dev enp0s29u1u2u1 metric 100
18:05:01.422  Deleted default via 192.168.1.1 dev enp0s29u1u2u1 table 200 metric 100
18:05:01.469  default via 192.168.1.1 dev enp0s29u1u2u1 metric 100
18:05:01.473  default via 192.168.1.1 dev enp0s29u1u2u1 table 200 metric 100
```

**~50 ms with no route, on every 5 s tick.** The main table survived it because networkd's
own `proto dhcp` default at metric 2000 stayed put, but **table 200 has no second route**, so
every IPv4 packet from VLAN 226/227/222/1337 (cameras, printers, guest) that arrived in the
window was dropped — roughly 1 % uniform loss, injected on purpose, for as long as the daemon
had been running. VLAN 225 (the tunnel, table 100) and IPv6 (main table) were untouched,
which is why it looked like "some connections" rather than an outage.

The fix is `clear_ours(cfg, candidates, keep)`: `keep` is the selected device and is
**skipped**, so the only writes to the live path are `replace`s and the route is never absent.
Routes belonging to a path that is *not* selected are still cleared, and `clear_ours` now
reports only deletions that actually happened — which also stops `repairs` printing "cleared
daemon routes" on every tick whether or not anything was cleared.

Re-verify after any change to `crates/wan-manager/src/apply.rs`:

```bash
sudo timeout 20 ip monitor route | grep -c Deleted   # must be 0
f=0; for i in $(seq 2000); do ip route get 1.1.1.1 mark 2 >/dev/null 2>&1 || f=$((f+1)); done; echo fails=$f  # must be 0
jq -r '.repairs[]' /run/wan-manager/status.json       # no "cleared daemon routes"
```

Measured after the fix: 9,494 `mark 2` lookups with 0 failures (was ~1 %), 0 `Deleted` route
events in 20 s (was ~4 in 15 s), 50/50 pings through the tunnel from `services1`, and
`networkd` was **not** restarted by the switch, so `ppp0` kept `81.187.223.123` and table 100
kept its default.

## The throughput column was measuring latency (2026-11-11)

**`probe::throughput` used curl's `%{speed_download}`, which is bytes over the *whole*
request.** The probe fetches a 56 KB page, so on a fast path almost all of the measured time
is the server answering, and the number moved with the *server's* time-to-first-byte instead
of the link's capacity. Measured on `gw`:

```text
56 KB from one.one.one.one:  size 56614  total 0.124 s  ttfb 0.109 s  -> 457 597 B/s  (reported)
10 MB from speed.cloudflare: size 10 MB  total 2.414 s  ttfb 0.662 s  -> 4 141 702 B/s  (truth)
```

**8.7x wrong, and it wandered** — five consecutive samples of the old probe spanned
368-506 KB/s purely on TTFB jitter. That also means the numbers in "Sessions were dying"
above ("456 kbit/s and 80 kbit/s") are ~8.7x low: they were 56 KB fetches, not bandwidth.

The fix has two halves, and the first one alone was not enough — which is the part worth
keeping:

1. **Measure the transfer phase**, `size_download / (time_total - time_starttransfer)`,
   instead of `%{speed_download}`.
2. **Use a payload big enough to have a transfer phase.** The first attempt kept the 56 KB
   homepage and floored the transfer time at 20 ms as a divide-by-zero guard. On a fast path
   the transfer phase is ~11 ms, so every sample clamped to exactly `56614 / 0.02` —
   **2 830 700 B/s for every candidate at once, to the byte.** A plausible-looking constant
   that measured nothing, and the identical number on all three rows was the tell. The floor
   is a pure guard (1 ms) now, and `--probe-url` is
   `https://speed.cloudflare.com/__down?bytes=1000000`: one megabyte puts the transfer phase
   at ~0.2 s even on a 5 MB/s path, and `--max-time 5` still returns a valid partial-transfer
   rate on a slow one.

Two samples, **max**, not mean: congestion can only make a sample slower, so the larger one
is the better estimate of capacity. Each measurement is therefore 2 MB per eligible
candidate, which is why `--throughput-interval` is 600 s in `machines/gw/wan-backup.nix` —
every byte of it is metered on a cellular path, and ten minutes still refreshes inside the
600 s failback dwell.

**The general lesson: a floor on a measured duration turns the measurement into
`payload / floor`.** Any clamp meant only to avoid a division by zero has to be small
compared with the real quantity, or it silently becomes the answer — and it does so most on
the fastest paths, where the transfer phase is shortest.

**The unit is bytes per second, and always was** — `format::fmt_rate` on the page renders
KB/s and MB/s, so the display was right and only the magnitude was wrong. `rank::score`'s
local was called `mbps`, which is how the wrong assumption survived; it is `mbytes` now. The
score is only compared as a *ratio* (`score * policy.margin`), so fixing the magnitude does
not change the failback rule, but it does make throughput dominate the additive latency term
— which is what the doc comment always claimed it did.

Re-verify (compare the page against a real transfer on the same interface):

```bash
curl --interface enp0s29u1u2u1 -s -o /dev/null \
  -w 'real %{speed_download} B/s\n' --max-time 25 'https://speed.cloudflare.com/__down?bytes=10000000'
jq -c '.candidates[]|{i:.interface,bps:.throughput_bps}' /run/wan-manager/status.json
```

## The phone's greyed-out checkbox (measured 2026-11-09)

The Android USB-tethering checkbox is greyed out because the phone never gets a data
link, not because gw lacks a capability. On the live box the phone appears as a
high-speed device on `2-1.1` and then fails *before* any USB class or driver is
negotiated:

```text
usb 2-1.1: new high-speed USB device number 5 using ehci-pci
usb 2-1.1: device descriptor read/64, error -71
usb 2-1.1: device not accepting address 7, error -71
usb 2-1.1: unable to enumerate USB device
```

`-71` is `EPROTO`. The host sees the phone's pull-up and then cannot read its
descriptors, so the phone sees power and no data. Android greys out USB tethering exactly
when it has no data connection to offer. The kernel already has `rndis_host`,
`cdc_ether`, `cdc_ncm` and `ipheth`; the daemon loads them at boot anyway, along with the
common USB Wi-Fi drivers and the redistributable Linux/regulatory firmware. The remaining causes are physical:

1. a charge-only or damaged USB cable (most common),
2. a port that cannot source enough current for the phone (the internal EHCI header is
   the suspect; the UPS and the Wi-Fi stick use the other port),
3. a phone whose USB mode is still "charging only" until it is unlocked and the mode is
   chosen.

`lsusb -t`, `usb-devices`, `dmesg | tail`, `uhubctl` and `adb devices` are installed
because that is the diagnostic path. The daemon's status includes the USB tree and the
last 15 minutes of kernel enumeration failures, and the Uplink tab prints the same
diagnosis, so the next check does not need a shell.

**What was then measured, and what it ruled out (2026-11-09, same day).** The three
physical causes above were worked through on the box, and the answer is none of them:

- Both of `gw`'s sockets fail with this phone, 100 % of ~45 attempts, while the same
  socket carries a 450 mA RT5370 radio without complaint. `uhubctl` shows the port reach
  `0501 power highspeed connect` — it chirps at high speed and never gets the `enable`
  bit — so the failure is after signalling and before the first descriptor.
- `usbcore.old_scheme_first=1` and `usbcore.autosuspend=-1` **are** in the boot entries of
  generations 61-64 but were not in the running `/proc/cmdline` (booted from an older
  generation), so they had never applied. Applying both at runtime through
  `/sys/module/usbcore/parameters/` and cycling VBUS changed nothing — the proof of
  whether they helped is `/proc/cmdline`, not the module parameter file.
- The port latches dead after `unable to enumerate`: only a VBUS off/on re-arms it, which
  is why a stuck phone needs a human. `crates/wan-manager` cannot power-cycle a port it
  cannot enumerate; there is no repair for this from software.

**USB/IP was the obvious way round it, and it does not work for an Android tether.**
`gw` is happy as a `vhci-hcd` client (the `usbip` userspace from the booted kernel's
`kernelPackages.usbip` builds and loads), and `aibox` claims the phone with `usbip bind`
— the kernel logs `usbip-host 7-1: register new device` — but the moment `usbip-host`
claims the device, Android re-enumerates: `usbip-host 7-1: USB disconnect, device number
11`, and the phone comes back with its tether function gone (`22d9:2769`, one
vendor-specific interface, no `cdc_ncm`). Twice, and `usbip list -r localhost` answered
`no exportable devices found` both times. Restoring it needed a hand on the phone (or
`adb shell svc usb setFunctions ncm` with USB debugging authorised). So there is no
USB-level path for this phone on this router.

## The phone that enumerates somewhere else (measured 2026-11-09)

The phone enumerates on `aibox` — SuperSpeed 5000M, `cdc_ncm`, `enp118s0f3u1`, a DHCP
lease and a gateway that answers in 2.5 ms — so the path that works is a *network* path
from `aibox`, not a USB one on `gw`. Both halves are one module each:
`machines/gw/wan-relay.nix` and `machines/aibox/wan-relay.nix`, and the daemon learns
about the interface from `--relay-candidate wan-aibox` in
`machines/gw/wan-backup.nix`.

It is an IP-in-IP tunnel (`wan-aibox` on the router, `wan-gw` on the box) whose far end is
whatever uplink `aibox` has plugged in. `wan-manager` treats it as a candidate of kind
**`relay`**: probed, ranked, selected and NATed like any other, and refused for the same
reason as any other when there is no internet through it.

Three things make it work, and each was measured rather than assumed:

1. **The tether's lease lands in table 900, not in the main table.** Every DHCP route the
   phone offers is installed with `RouteTable = 900`, so plugging a phone into the build
   machine does not move the build machine's egress. `ip route show default` on `aibox`
   must keep saying `via 10.3.1.1 dev vlan225` while the phone is plugged in.
2. **Only forwarded packets look in table 900** (`IncomingInterface = wan-gw`), so the
   only traffic that ever leaves through the tether is traffic the router handed over.
3. **`aibox` masquerades what arrives on the tunnel** (`iifname "wan-gw" masquerade`).
   The phone rejects a source outside its own /24: an un-NATed inner source gets
   `From 10.111.16.77 icmp_seq=1 Destination Net Unreachable`, and with this one rule the
   same probe through the tunnel returns **0 % loss at 2.6 ms**.

On the router the same shape appears twice: the tunnel's own address is declared into
table 950 with a `From = 10.255.255.1/32` rule, so a path that is only being *measured*
is never a path that is being *used*; and when the daemon selects the relay it writes
`default dev wan-aibox` into both the main and table-200 routes, or the box's own sockets
and every VLAN would disagree about the way out.

**What it does not currently deliver, and whose fault that is.** The phone's own tether
firewall refuses forwarded traffic: with the tunnel up and NAT correct, ICMP gets
`Destination Net Unreachable`, TCP to `1.1.1.1:80` never connects, and the phone's
resolver does not answer over the tether either — while its *hotspot* carries this
network's traffic fine. So the relay shows in the Uplink tab as `relay` with "no internet
through this path" and is correctly not selected. That is a measurement, not a
configuration: if the phone starts sharing its uplink over USB, the next tick uses it.

Reading it back:

```bash
# on aibox: the lease, the forwarding rule, the NAT rule
ip -4 route show table 900; ip rule show | grep 900; nft list table ip wan-relay
# on gw: the candidate, its kind and its verdict
jq -c '.candidates[] | {i:.interface,kind,internet_ok,reason}' /run/wan-manager/status.json
```

MTU is 1400 on both ends (1500 minus the 20-byte outer header, with headroom): the phone
tethers at 1500, and an inner packet that has to fragment on a link nobody is watching
is a stall rather than a slow page.

**Two traps met on the way, both worth the sentence.** Restarting `systemd-networkd` on
`aibox` while a hand-written `.network` file sat in `/run/systemd/network` stranded
`vlan225`'s DHCP lease — the box stayed up, answered IPv6, and was unreachable over IPv4
for ~15 minutes (`ip neigh` on both `gw` and `services1` said `FAILED`/`INCOMPLETE`) until
networkd was restarted again over IPv6. And a `systemd-run --unit=... --collect` client
holds the caller's pipe for the life of a `Type=oneshot` payload, which is what made a
diagnostic command in this investigation look like a hung build.

## Break glass

`machines/gw/wan-backup-emergency.sh` is installed at `/etc/wan-backup-emergency.sh`.
It now discovers every candidate (USB Ethernet, every wireless interface, every other
Ethernet), tries each, installs the routes and NAT, and **retries until something works**
(`--once` for a single pass, `--watch` to keep watching). It moves the AAISP session onto
the working path by default (`--no-tunnel` opts out). It stops `wan-manager` so the two
cannot fight; a reboot ends it, and `just wan-backup-handback` hands the box back without
one.

## Sessions were dying because the ranking moved the network (2026-11-09, later)

**The report was "connections are very flaky", and the daemon was most of the cause.** With
the AAISP hub still without an upstream (the 2026-11-07 open item, `docs/follow-ups.md`) the
whole network was running on backup paths, and every few minutes the selected egress moved
between them. Measured on the box, 2026-11-09 19:40-20:25:

```bash
journalctl -u wan-manager --since '-6h' | grep 'wan-manager: backup'   # the moves
journalctl -u xl2tpd-aaisp    --since '-6h' | grep -E 'disconnected|Connecting'
```

```text
19:53:13  backup (egress enp0s29u1u2u3i1, tunnel ppp0)   # the move
19:54:12  pppd: Serial link appears to be disconnected    # 59 s later, the session dies
19:54:54  xl2tpd: Connection established …                # 42 s after that, IPv6 is back
20:15:01  backup (egress enp0s29u1u2u3i1, tunnel ppp0)   # and again
20:16:04  pppd: Serial link appears to be disconnected
```

39 such moves in 24 h. Each one: (1) rewrites the NAT source address of every forwarded
flow, so every long-lived TCP session in the building dies; (2) strands the L2TP socket on
the old path, so `pppd` waits out six missed LCP echoes and then redials — 40-90 s in which
there is **no IPv6 on any VLAN** and, after three failed ticks, no table-100 default, so
VLAN 225 loses its public `/29` too. The dashboard's numbers also show why the moves were
not even buying anything: the two paths were 456 kbit/s and 80 kbit/s.

**Why it moved.** Two decisions in `rank::update`, both of which were "immediate" on
purpose and both of which cost more than they saved:

- failover happened on the **first** tick the incumbent's probe failed (three echoes at two
  anchors — one lost burst on a 2.4 GHz cell is enough), and
- the challenger needed only **20 %** more score to take the seat back, which the daemon's
  own 60 s throughput samples fluctuate around on a cellular path.

**What it does now** (`rank::History` + `rank::Policy`, `crates/wan-manager/src/rank.rs`):

| Option | Default | What it decides |
| --- | --- | --- |
| `--failover-ticks` | `3` | consecutive ineligible ticks before the network moves. A link that loses **carrier** is not a probe blip and still fails over on the first tick. |
| `--failback-dwell` | `600` | how long a challenger must stay better before it takes a *healthy* incumbent's seat. Failing back is the only voluntary move. |
| `--failback-margin` | `1.5` | how much better, as a factor of the incumbent's score. |
| `--tunnel-unit` | `xl2tpd-aaisp` | the session to redial when the egress does move. |
| `--tunnel-redial` | `10` | grace after the move, so the new default route is in place first (`0` disables). |
| `--tunnel-redial-cooldown` | `300` | never redial more often than this: a redial storm is the failure mode the first version was avoiding, and the loop was the *ranking*, not the restart. |

The two halves are complementary and both are needed: the durations stop the network moving
for no reason, and the redial makes a move that *is* warranted cost ~15-25 s instead of
40-90 s. The restart is safe from the daemon's sandbox (measured: `systemd-run
-p ProtectSystem=strict … systemctl is-active xl2tpd-aaisp` answers `active`).

**Re-verify** (with the unit running and a backup selected):

```bash
journalctl -u wan-manager --since '-2h' | grep -E 'redialled|egress'
readlink -f /run/current-system      # the daemon's own unit, unchanged by a service restart
jq -r '.actions[]' /run/wan-manager/status.json 2>/dev/null; jq -c '.candidates[]|{i:.interface,score}' /run/wan-manager/status.json
ip -4 route show table 100; ip -6 route show default
```

The counter to watch is the journal line count of `wan-manager: backup` over an hour: before
this change it was 2-5 an hour on a single dead upstream, and each one was a tunnel outage.
`docs/follow-ups.md` still owns the real fix — the hub has no upstream and that is AAISP's.

**Forcing everything through the tunnel is not that fix.** `ppp0` is currently carried *over*
a backup (`ip route get 194.4.172.12` → `via 192.168.1.1`), so it has nothing better to offer;
moving every VLAN's mark to 1 would concentrate the whole network on one 0.5 Mbit path and
make each of these outages total instead of confined to VLAN 225. The split in
`docs/router-and-dns.md` ("Which uplink each VLAN leaves by") exists because only VLAN 225
needs the tunnel's public addresses, and it is worth revisiting only once the hub carries
traffic again.

## Status

`https://network-info.int.leighhack.org/` has an **Uplink** tab (`/api/wan`) showing the
selected egress, the tunnel, every candidate with its gateway/internet/latency/throughput/
score/verdict, every wireless adapter's association and BSS list, the USB diagnosis, and
the repairs applied on the last tick. `/metrics` gains `wan_manager_*` series.

## What it cannot do

- A phone that cannot enumerate on the router's own USB controller is not made to work by
  this daemon on that controller; the *path* through another box's controller is what
  `--relay-candidate` (above) adds, and a phone whose tether refuses forwarded traffic is
  still refused traffic.

- The hotspot alone still gives no IPv6 and no inbound public services. What restores them is
  the AAISP L2TP session dialling over the working path. Moving the selected egress rewrites
  the `/32` to the LNS, but the L2TP socket is already bound to the old path's source
  address, so the session is stranded until something redials it — measured 2026-11-09 19:54:
  an egress move at 19:54:25 (this daemon's own journal: `tunnel down`) and a new session at
  19:54:54, thirty seconds with no IPv6 and VLAN 225's IPv4 on the fallback. **The daemon now
  redials the session itself on an egress move** ("Sessions were dying…" below); the
  break-glass script still does it too (`--no-tunnel` opts out). If AAISP's LNS refuses the
  off-net session, the network is IPv4-only until the hub returns.
- The hotspot is metered and the 2.4 GHz path is slow; the daemon ranks by measured
  performance, not by cost.
- A Wi-Fi adapter whose driver is out-of-tree or whose firmware is missing is not made to
  work by discovery. Enumerating it and saying so is what the status does.
