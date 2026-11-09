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

- The hotspot alone still gives no IPv6 and no inbound public services. What restores
  them is moving the AAISP L2TP session over the working backup, which the daemon does
  whenever the hub cannot carry packets. If AAISP's LNS refuses the off-net session, the
  network is IPv4-only until the hub returns.
- The hotspot is metered and the 2.4 GHz path is slow; the daemon ranks by measured
  performance, not by cost.
- A Wi-Fi adapter whose driver is out-of-tree or whose firmware is missing is not made to
  work by discovery. Enumerating it and saying so is what the status does.
