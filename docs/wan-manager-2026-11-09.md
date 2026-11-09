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

- The hotspot alone still gives no IPv6 and no inbound public services. What restores
  them is moving the AAISP L2TP session over the working backup, which the daemon does
  whenever the hub cannot carry packets. If AAISP's LNS refuses the off-net session, the
  network is IPv4-only until the hub returns.
- The hotspot is metered and the 2.4 GHz path is slow; the daemon ranks by measured
  performance, not by cost.
- A Wi-Fi adapter whose driver is out-of-tree or whose firmware is missing is not made to
  work by discovery. Enumerating it and saying so is what the status does.
