# The backup WAN — the hotspot that kept the network online on 2026-11-07

**What this is:** `machines/gw/wan-backup.nix`. A USB Wi-Fi adapter on the router,
associated to the organisation's `Leighhack WAN Backup` hotspot, used as the network's
egress **only when the AAISP path is not usable**. It is declarative — it survives a
reboot and a `switch` — and it is one import line to remove when the real uplink is
trusted again.

## The day it was built

Measured on `gw`, 2026-11-07:

| Check | Result |
| --- | --- |
| `ping 192.168.20.1` (the AAISP hub's handoff) | **OK** — the cable and the hub's DHCP are fine |
| `ping -I router-uplink 1.1.1.1` | **fails** — the hub has no upstream behind it |
| `journalctl -u xl2tpd-aaisp` | `Maximum retries exceeded … Will redial in 10 seconds`, forever |
| `sudo ip route show table 100` | empty — the tunnel is down, so VLAN 225 has no route |
| `ping -I wlp0s29u1u2 1.1.1.1` | **OK** — the hotspot works |

So the tunnel could not dial (it dials *through* the hub), VLAN 225 had no table-100
default, and every other VLAN's table 200 pointed at a hub with no internet. The network
was offline.

The first fix was built by hand in about an hour and is worth writing down because it is
what the declarative version replaces:

```text
/run/wpa-backup.conf                      wpa_supplicant config, mode 0600 (the PSK)
/run/systemd/network/10-wan-backup.network  DHCP, UseRoutes=no, UseDNS=no
wpa_supplicant -B -i wlp0s29u1u2 -c /run/wpa-backup.conf
ip route replace default via <gw> dev wlp0s29u1u2 metric 50
ip route replace default via <gw> dev wlp0s29u1u2 table 300 metric 100
ip rule add fwmark 1 lookup 300 priority 50      # VLAN 225
ip rule add fwmark 2 lookup 300 priority 50      # every other VLAN
nft add rule ip routerNat srcnat oifname wlp0s29u1u2 ip saddr 10.3.0.0/16 masquerade
```

Every line is in `/run` or in the live kernel. A reboot ended it, and a `switch` reloaded
nftables and **deleted the masquerade rule** — the LAN's internet was one deploy away from
disappearing while the box itself still looked fine. The gateway also had to be retyped
whenever the hotspot re-associated with a different subnet (measured: `10.112.100.0/24` at
13:48, `10.120.243.0/24` at 14:53), because a typed route does not follow a DHCP lease.

## What the flake does now

`machines/gw/wan-backup.nix` owns four things:

| Piece | Where | Why it is there |
| --- | --- | --- |
| the association | `wan-backup-wifi.service` | `wpa_supplicant` in the foreground under systemd, never-give-up restart. The passphrase is `wan_backup_wifi_psk` in `secrets/secrets.yaml`, read at start into a mode-0600 file; `wpa_passphrase` derives the PMK on stdin and it never reaches the store or a process listing |
| the *other* path | `3-wwan-usb` in `./hardware-configuration.nix` + `systemd.network.networks."21-wwan-usb"` | a USB-tethered phone, renamed to `router-wwan-usb` by matching **type** (`ether` on the USB bus) rather than a MAC or a driver name — see [The USB-tethered phone](#the-usb-tethered-phone-2026-11-08). DHCP, the same policy, and a metric one step better than the Wi-Fi's |
| the address | `systemd.network.networks."20-wwan"` | DHCP with `UseRoutes = true`. The **default route is the server's**, so a re-association onto another `/24` is followed instead of a hand-typed gateway going stale. Its metric is 900, so it never wins by itself |
| the decision | `wan-backup-route.service` + `.timer` (10 s) | picks the first entry of `./constants.nix`'s `WWAN_FALLBACK_NICS` that holds a DHCP default (cable, then hotspot), and promotes it into the main table **and** table 200 only when the primary is not usable; as a timer it is also the self-healing loop (a stale or deleted route comes back on the next tick) |
| the tunnel's path | `wan-backup-route` | while the hub is dead it puts a metric-5 route for `194.4.172.12` (the AAISP LNS) out the hotspot, so xl2tpd's next dial carries the session over LTE and the public `/29`, the `/48` and the tailnet control plane keep their addresses. Deleting it hands the endpoint back to the hub |
| the NAT | `./firewall.nix`'s `routerNat` | `oifname` for each entry of `WWAN_FALLBACK_NICS` — the cable and the stick — for the LAN blocks and the tailnet. Without it a "fallback" carries the router's own traffic and nothing else |

The interface name is pinned to the adapter's MAC (`c8:3a:35:e1:a3:f3`, a Ralink RT5370)
in `./hardware-configuration.nix`, so moving the stick to another USB port does not rename
it. It is **not** called `router-wwan` yet on purpose: the masquerade rule is keyed on the
name, so a rename is a NAT change that has to go out in a switch that keeps both names in
the rule for the changeover. A quiet-moment job (see `./constants.nix`'s `WWAN_NIC`).

### The policy, in one table

`wan-backup-route` answers two questions every 10 s: can the hub carry a packet, and is the
L2TP endpoint routed through the hub? The second matters because a dialled tunnel is only
proof about the hub while it is dialled **over** the hub — once the endpoint is routed out
the wwan (the third row), the tunnel says nothing about the hub and the probe has to run.
Then:

| Primary | Hub probe | Egress chosen | `ip -4 route get 1.1.1.1 …` |
| --- | --- | --- | --- |
| tunnel dialled over the hub | skipped — the session proves it | tunnel for 225, hub for the rest, wwan idle | `mark 1` → `dev ppp0 table 100` |
| down | answers | hub, wwan idle | `mark 1` → `via 192.168.20.1` (main) |
| down | silent | **wwan** for everything *and* the L2TP endpoint | `mark 1` → `dev ppp0` (once re-dialled), `mark 2` → `via <hotspot gw> dev wlp0s29u1u2` |
| down | silent, wwan gone | whatever the hub has (nothing) | the hub's own default |

The third row is what restores public services: `194.4.172.12/32` gets a metric-5 route out
the hotspot, xl2tpd's next redial carries the AAISP session over it, and the `/29`, the
`/48` and the tailnet control plane come back — at the hotspot's speed and data cost.

The probe is skipped only while the tunnel is dialled over the hub, so a healthy primary
costs no probe traffic. If the hub blocks ICMP while working, the hub is judged dead and
the wwan is preferred — the safe direction, and the one the probe was chosen for. What it
cannot see is a hub that answers ICMP and nothing else; that would need a TCP or DNS probe,
and three echoes are the compromise between "is it reachable" and "do not make the network
flap".

**Failover is one bad probe; return is two good ones.** The decision is hysteretic on
purpose: a marginal link that loses every other probe must not move the network's egress
back and forth every 10 s, because each move rewrites the NAT source address of every
open connection. A dead primary therefore hands over within one tick, and a recovered one
is re-adopted after ~20 s. The streak lives in `/run/wan-backup/route.state` with the
decision, so the first tick after a reboot starts clean.

**A route is not a link — the fallback is probed too (2026-11-07).** Everything above asks
whether the *primary* works; nothing asked whether the *fallback* does, and that is the bug
that made this module feel broken on its first day in service. Between 15:46 and 16:18 the
reconciler correctly reported `egress 'wwan'` over and over while the 2.4 GHz association sat
there losing beacons — 225 `CTRL-EVENT-BEACON-LOSS` events before lunch, `iw link` still
saying "Connected", 262 ms RTT to the hotspot's own gateway — so the LAN had a default route
with a dead radio behind it: the box looked healthy, the network had no internet, and the
only way back was a human running `wan-backup-emergency.sh`. So the chosen fallback now gets
probed as well, on the same 10 s tick:

| | |
| --- | --- |
| what it pings | **the path's own gateway**, not the internet. A phone with no cellular still answers its tether/hotspot address and no restart can mend that; a gateway that stops answering is the radio/driver/wedge case that a reset *does* mend. One echo per tick, so it is free on a metered link |
| when it counts | only while the fallback is *needed*: `wwan` (chosen, but not passing traffic) or `nowhere` with the hub probe down (nothing to choose — the wedged-supplicant case). While the primary is healthy it never fires, so a deliberately-off hotspot is never reset |
| when it acts | after **12 consecutive ticks** (2 min) — long enough that a re-association, a DHCP renew or one lost echo never triggers it |
| what it does | restarts `wan-backup-wifi` for the stick, bounces the link and re-runs DHCP (`ip link set … down/up`, `networkctl reconfigure`) for the cable — i.e. exactly what the runbook does by hand. The counter is `/run/wan-backup/fallback.failures` |

**The kick costs an outage, so it must never fire on a working link.** Restarting the
association drops the address and the route, so for the re-association plus DHCP the network
has no egress at all — 15–60 s. That is the right trade only when nothing is flowing anyway,
which is why the threshold is two minutes of a *silent gateway* and why the probe is not a
reachability test of the internet. (Measured the hard way on 2026-11-07: a blackhole route
for the gateway address alone, on an otherwise perfectly healthy link, duly fired the kick
and cost a real blip. The mechanism works; do not test it on a live outage.)

## Public services and IPv6 while the primary is dead

Measured on 2026-11-07, after the `/32` change, with the hub still dead:

| Test | Result |
| --- | --- |
| xl2tpd's dial over the hotspot | `CHAP authentication succeeded: IP54800`, `ppp0` = `81.187.223.123` |
| `ip -4 route show table 100` / `ip -6 route show default` | `default dev ppp0` / `default dev ppp0` |
| `ip -4 route get 194.4.172.12` | `via 10.120.243.148 dev wlp0s29u1u2` — the LNS stays on the hotspot |
| `ip -4 route get 1.1.1.1 mark 1` | `dev ppp0 table 100 src 81.187.223.123` — VLAN 225 is back on the tunnel |
| services1 → `one.one.one.one` | IPv4 `200` in 0.17 s, **IPv6 `200` in 0.21 s** |
| `curl --resolve tailscale.leighhack.org:443:81.187.195.17 …/health` | `200` in 1.25 s, `remote_ip=81.187.195.17` — a real hairpin out the hotspot, through AAISP, and back down the tunnel |
| `curl --resolve nginx.leighhack.org:443:81.187.195.17` | `404` in 0.19 s — nginx answered, so inbound works (`/` is a 404 on that vhost) |
| tunnel throughput from services1 (10 MB) | **1.59 MB/s** — the same ceiling as the raw hotspot, i.e. the tunnel adds latency, not bandwidth |

So the honest statement is: **the hotspot alone gives no IPv6 and no inbound public
services; the AAISP session dialled over the hotspot gives both back**, because the
addresses are delivered through AAISP exactly as before. The cost is all of VLAN 225's
traffic, all inbound public traffic and DERP relaying riding a metered ~2.4 GHz link.

**Coming back.** When the hub's upstream returns, the probe succeeds twice, the `/32` is
deleted and the static hub route takes over. The existing session's outer packets change
source address, the LNS drops it, and pppd's LCP echoes tear it down (≈60 s) so xl2tpd
re-dials over the hub. VLAN 225 and the inbound public services have that gap; `systemctl
restart xl2tpd-aaisp` forces it immediately if you would rather not wait. If AAISP's LNS
were ever unreachable *through the hub* while the hub still answered ICMP, the tunnel
would stay down and the `/32` would stay on the hub — the manual lever is `sudo
/root/wan-backup-emergency.sh --force --tunnel-over-wwan`.

**Turning it off without touching the LAN:** `systemctl stop xl2tpd-aaisp` (the tunnel
dies; table 100 empties; VLAN 225 falls through to the wwan default like every other
VLAN, and inbound/tailnet control go with the public addresses) — or delete the `/32`
and restart the unit, which lets it re-dial over the hub.

## Taking it over from a hand-built setup (the rollout on 2026-11-07)

The first deployment had to hand the box over **without** a reboot, because the box was
already carrying the network on the hand-built link. In order:

1. Add `wan_backup_wifi_psk` to sops and `./sops.nix` (sops-nix fails *activation*, not
   evaluation, if the key is missing — do it before the switch).
2. `just check-eval` here (the router's gate; rule 22), then `just switch && sudo
   nixos-confirm`.
3. Take the interface over from the hand-started supplicant. `wan-backup-wifi`'s
   `preStart` stops any `wpa_supplicant` holding this interface, so the unit's own start
   does it: `sudo systemctl restart wan-backup-wifi` (a few seconds of association loss).
4. Remove the stale `/run` network file and let networkd read the declarative one:
   `sudo rm /run/systemd/network/10-wan-backup.network && sudo networkctl reload &&
   sudo networkctl reconfigure wlp0s29u1u2`. **Without this the `/run` file wins** —
   networkd takes the first matching file in lexical order, and `10-` sorts before `20-`.
5. `sudo systemctl start wan-backup-route` — it should log
   `egress is now 'wwan <gw>' (primary path down)`.
6. Delete the hand-typed routes and rules, now that the reconciler owns them:
   `sudo ip route del default via <old gw> dev wlp0s29u1u2 metric 50`,
   `sudo ip route flush table 300`, `sudo ip rule del pref 50` (twice).
7. `sudo rm /run/wpa-backup.conf` — the passphrase lives in sops now.
8. Nothing else: the reconciler installs the L2TP endpoint's metric-5 route out the hotspot
   on its next tick, and xl2tpd's next redial picks it up (up to ~40 s), so the AAISP
   session moves by itself.
9. Verify (`just switch` confirmed in the same invocation, rule 4):

```bash
ip -4 route get 1.1.1.1                 # the router's own traffic
ip -4 route get 1.1.1.1 mark 1          # VLAN 225
ip -4 route get 1.1.1.1 mark 2          # every other VLAN
sudo ip route show default dev wlp0s29u1u2 proto dhcp   # the input the reconciler reads
curl -fsS -m 10 -o /dev/null -w '%{http_code}\n' https://one.one.one.one
# and through the firewall, which is the half that a router-only test misses:
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20 'curl -fsS -m 10 -o /dev/null -w "%{http_code}\n" https://one.one.one.one'
```

If step 4 is skipped and the box is rebooted instead, `/run` is cleared and the declarative
file wins by itself — that is the normal path after any future reboot.

## Break glass

`machines/gw/wan-backup-emergency.sh` is the same 2026-11-07 recipe, frozen, with no flake
and no nix in its path. It is installed at **`/etc/wan-backup-emergency.sh`** by
`./wan-backup.nix`, and a copy was taken to **`/root/wan-backup-emergency.sh`** immediately
before the switch that introduced this module — that copy is a snapshot, the repository one
is canonical. Run it on `gw` over the LAN (ssh to `10.3.1.1` survives a dead WAN) or at the
console:

```bash
sudo /etc/wan-backup-emergency.sh --check   # can it find the iface, supplicant and passphrase?
sudo /etc/wan-backup-emergency.sh           # apply; refuses if the router is already online
sudo /etc/wan-backup-emergency.sh --force   # apply anyway (e.g. router online, LAN NAT missing)
sudo /etc/wan-backup-emergency.sh --force --tunnel-over-wwan   # also move the AAISP session
```

It stops the `wan-backup-*` units, associates to the hotspot, writes the `/run` network file
that sorts before the flake's (`10-` before `20-`), installs the metric-50 default in the
main table and table 300, the two `ip rule`s at priority 50, and the `masquerade` row —
and proves the result with a `curl`. `--tunnel-over-wwan` is the manual form of what the
reconciler now does by itself: route `194.4.172.12` out the hotspot at metric 5 and
`try-restart xl2tpd-aaisp`, so the public `/29`, the `/48` and the tailnet control plane
come back. If the nftables table itself is missing it installs a bare NAT table and says
loudly that the box has no firewall. **A reboot ends it**, and the declarative version
comes up instead.

**What break-glass costs, and why it is not the first lever (measured 2026-11-07).** The
script stops the `wan-backup-*` units to take the interface over, and `wan-backup-route` is
the thing that maintains the L2TP endpoint's `/32`. So after the emergency script the LAN has
IPv4 again **and nothing else**: `ppp0` disappears, and with it the AAISP session, the public
`/29`, the delegated `/48`, IPv6 and the tailnet control plane — the network looks repaired
while the front door and every public service are down. Reach for it when the *units or
networkd* are what broke. For the two failures it is usually reached for, the levers that
leave the tunnel alone are:

```bash
sudo systemctl restart wan-backup-wifi      # the association is wedged / beacon-lost
sudo systemctl start wan-backup-route       # the routes are gone (the timer re-runs it anyway)
sudo cat /run/wan-backup/route.state        # what the reconciler thinks: <state> <streak> <dev> <gw>
sudo cat /run/wan-backup/fallback.failures  # how close the watchdog is to resetting the path
systemctl restart xl2tpd-aaisp              # force the AAISP re-dial now, over whichever path
```

## Verify the policy, not just the internet

```bash
systemctl status wan-backup-wifi wan-backup-route.timer
journalctl -u wan-backup-route -n 20        # only speaks on transitions, and on a kick
sudo ping -c1 -W2 -I router-uplink 1.1.1.1  # the primary-liveness probe, by hand
sudo ping -c1 -W1 -I wlp0s29u1u2 10.120.243.148   # the fallback probe, by hand (its own gateway)
sudo cat /run/wan-backup/fallback.failures  # 0 unless the fallback is silent; 12 is a kick
networkctl status wlp0s29u1u2               # AP, signal, lease, gateway
iw dev wlp0s29u1u2 link                      # band, signal, negotiated rate (the speed question)
sudo nft list chain ip routerNat srcnat     # the masquerade rows (both paths)

# while the hub is dead and the tunnel is carried by the hotspot:
ip -4 route get 194.4.172.12                # must name wlp0s29u1u2, not router-uplink
ip -4 route get 1.1.1.1 mark 1              # VLAN 225 → dev ppp0 table 100
ip -br addr show ppp0                       # the AAISP address
ip -6 route show default                    # dev ppp0, and IPv6 is back
```

A `wan-backup-route` that logs `primary` while the tunnel is down means the *hub* is the
chosen egress; that is correct if AAISP's LNS is the only broken part. It logs `nowhere`
when the tunnel and the hub are both down and the wwan has no lease either — at that point
there is nothing to choose.

## The limits — read these before calling it "the internet"

- **The hotspot alone gives no IPv6 and no inbound.** A link-local is all the DHCP client
  gets, and `81.187.195.17/29` is routed to the AAISP line. What restores both is the L2TP
  `/32` above — the addresses arrive through AAISP as always, just carried over LTE. If the
  tunnel cannot dial (AAISP's LNS refusing the off-net session, or the hotspot blocking UDP
  1701), the network has no IPv6 and no front door, and the RAs still advertise the VLAN
  prefixes (`machines/gw/README.md`'s RA-timer trap).
- **It is metered, and now it carries the tunnel.** All of VLAN 225, every inbound public
  request and DERP relaying ride the hotspot while the hub is dead. The policy keeps the
  wwan idle whenever the hub carries packets, and demotes it ~20 s after it comes back.
- **The hotspot's subnet moves.** That is why the default route is the DHCP client's and
  the `/32` is tied to the same gateway, not a line in this repo.
- **MTU**: 1500 on the hotspot path (measured with DF pings), so the tunnel's 1460 fits
  with L2TP's overhead; nothing to clamp. Over L2TP the inner MTU is still 1460.
- **It is slow, and the obvious probe lies.** See the section below — `https://1.1.1.1`
  redirects and times out on a high-latency link, so liveness checks use
  `https://one.one.one.one` with a 10 s budget.

### Why it is slow: the hotspot is 2.4 GHz only

Measured 2026-11-07 with the phone advertising "400 Mbit 5G":

| | Ralink RT5370 (`wlp0s29u1u2`) | Realtek RTL8821CU (`wlp0s29u1u1`) |
| --- | --- | --- |
| band the hotspot offers | **2462 MHz (2.4 GHz ch 11) only** | 2.4 and 5 GHz, but no 5 GHz BSS for this SSID |
| signal | −57 dBm | −68 dBm |
| negotiated tx / rx | 72.2 Mbit/s (MCS7 short GI) / 52 Mbit/s | 39 Mbit/s (MCS4) / 1 Mbit/s |
| measured through the hotspot | **1.75 MB/s down, 0.45 MB/s up** | not measured — worse link, not swapped in |

72.2 Mbit/s is the 1×1 HT20 ceiling, so the Ralink is already at its maximum rate; the
network's 14 Mbit/s is what a single 2.4 GHz spatial stream delivers in a crowded band.
The second dongle is a dual-band 802.11ac part, but `Leighhack WAN Backup` is broadcast on
2.4 GHz only (a scan from the Realtek found it at 2462 MHz and no 5 GHz BSS of that name),
and on 2.4 GHz it associated 11 dB weaker than the Ralink. **It is not the better adapter
for this AP.** The dongle is left in the box, unconfigured, as the 5 GHz option.

What would actually help, in order: **put the phone's hotspot on 5 GHz** (Android: hotspot →
AP band; iPhone: turn "Maximize Compatibility" off). Then the Realtek is the right dongle,
and there are two ways to use it, both cheap:

* **Move it into the Ralink's USB port (2-1.2).** Kernel interface names follow the USB
  *port*, not the device, so it comes up as `wlp0s29u1u2` and **no flake change is needed**
  at all — the pin in `./hardware-configuration.nix` matches the Ralink's MAC, so the
  Realtek simply takes the path name.
* **Keep both plugged** and pin the Realtek: set `WWAN_NIC = "wlp0s29u1u1"` in
  `./constants.nix`, change the `2-wwan` link's `MACAddress`/`Name`, and switch once — the
  masquerade rule is generated from `WWAN_NIC`, so it moves with it. Do it while the
  interface is already associated (the switch reloads nftables, and a rule naming the
  wrong interface is a LAN with no NAT).

The other option is **USB-tethering the phone** (RNDIS/CDC-ECM): that removes Wi-Fi from
the path entirely and is the only way to see the 400 Mbit cellular link. The router's
ports are USB 2.0 (480 Mbit), which is not the limit at that scale. Either way the LAN is
a 100 Mbit port (`machines/gw/README.md`), so it is not the limiter at 14 Mbit/s.

That option is now built — it was the answer to "the Wi-Fi backup is extremely
unreliable" — and it is preferred over the stick automatically whenever the cable is in.
See the next section.

## The USB-tethered phone (2026-11-08)

**Why:** two faults, two causes. The stick's association is the stick's radio, not the
phone, so a crowded 2.4 GHz band drops it — and every re-association moves the hotspot to a
new `/24`, which rewrites the NAT source address of every open connection. And the ceiling
is the radio, not the phone's cellular link: one 1×1 HT20 spatial stream on 2.4 GHz capped
the whole network at 1.75 MB/s down while the phone advertised 5G.
`docs/camera-wifi-cells-2026-10-31.md` is the same lesson measured on the camera side:
throughput is a property of the cell, not of the device. A cable has neither fault — it
cannot be deauthenticated by a neighbour's AP, and its rate is whatever the phone's radio
can do.

**What it is, mechanically:** the phone presents an ordinary Ethernet netdev — Android
`rndis_host` (or `cdc_ether` on newer releases), an iPhone `ipheth`/CDC-NCM, some phones
`cdc_ncm` — and serves DHCP over it with itself as the gateway. All of those drivers were
already in this box's kernel (`modinfo -F filename rndis_host cdc_ether cdc_ncm ipheth` on
`gw`, 2026-11-08); nothing was added to the build for this.

**How it is named** — and why it is *not* pinned the way the other two NICs are:
`./hardware-configuration.nix`'s `3-wwan-usb` matches `Type = ether` **and**
`Property = ID_BUS=usb` and renames whatever matches to `router-wwan-usb`. Deliberately not
a MAC — that is the phone's, and phones get swapped (some randomise it per session) — and
deliberately not a driver name, because which driver a phone uses depends on the phone and
its Android version. `Type = ether` excludes the Wi-Fi stick (`wlan`) and the two PCI
e1000es; `ID_BUS=usb` keeps it to the cable. Without the rule the netdev gets the kernel's
path name (`enp0s29u1u3`), i.e. the USB **port** — exactly the fact that must not decide an
interface name the firewall NATs on.

**The policy is unchanged; only the order is new.** `./constants.nix`'s
`WWAN_FALLBACK_NICS` is `[ router-wwan-usb wlp0s29u1u2 ]`, and `wan-backup-route` takes the
first entry that holds a DHCP default:

| Cable | Stick | Fallback egress |
| --- | --- | --- |
| in, tethering on | associated or not | **the phone** (`router-wwan-usb`) |
| out, or tethering off | associated | the hotspot (`wlp0s29u1u2`) |
| out | not associated | `nowhere` — there is nothing to promote |

The stick stays configured and keeps associating while the cable is in: it costs nothing
idle and it is the path that works when nobody has plugged a phone in. Both `.network`
files are DHCP with `UseDNS = false`, `UseRoutes = true` and a metric (880 for the cable, 900
for the stick) that loses to the hub's static 300 and to the promoted 100 — so the kernel's
own choice and the script's preference agree, which is what makes a tick that never runs
harmless. The L2TP `/32` follows whichever path was chosen, so the AAISP session — and the
public `/29`, the delegated `/48` and the tailnet control plane with it — moves onto the
cable on the next dial.

**Using it:** plug the phone into a USB port on `gw` (they are USB 2.0, and the phone
charges off the same port, which on the UPS's battery is a small bonus), then turn on **USB
tethering** in the phone's settings. Android allows USB tethering and the Wi-Fi hotspot at
the same time, so the stick does not have to be switched off. No deploy is needed for
either: the link rule and the DHCP client are already in the generation.

**Verify** — the last two are the ones that matter, because a link that is up but not NATed
carries nothing and the router's own `curl` says nothing about the LAN:

```bash
networkctl status router-wwan-usb        # the lease is the phone's (192.168.42.x, or an iPhone's 172.20.10.x)
ip -4 route show default dev router-wwan-usb proto dhcp   # networkd's own route — the script's input
journalctl -u wan-backup-route -n 5      # "egress 'wwan' (... via router-wwan-usb <gw> ...)"
ip -4 route get 194.4.172.12             # the AAISP LNS must name the chosen fallback device
sudo nft list chain ip routerNat srcnat | grep router-wwan-usb   # both masquerade rows
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20 \
  'curl -fsS -m 10 -o /dev/null -w "%{http_code}\n" https://one.one.one.one'
```

**The honest ceiling is still this box's LAN port**, not the uplink: `gw`'s NIC negotiates
100 Mbit (`machines/gw/README.md`), so ~11 MB/s is the wall on any path. What the cable buys
is that the 2.4 GHz cell stops being the ceiling *first*, and that the fallback stops
dropping out for radio reasons in the middle of an outage.

**What it does not change:** without the AAISP session there is still no IPv6 and no
inbound, exactly as with the hotspot — the `/32` is what restores both, and it now rides
the cable. The break-glass script (`wan-backup-emergency.sh`, installed at
`/etc/wan-backup-emergency.sh`) is still Wi-Fi only: it is a frozen 2026-11-07 recipe for
when the *units*, not the uplink, are what broke. If the phone path ever needs its own, the
manual form is simply `ip link set router-wwan-usb up` and let networkd's DHCP run — the
reconciler promotes the lease within 10 s by itself.

## Retiring it

Unplugging the adapter is safe and immediate-ish: the association dies, the DHCP route
disappears, `wan-backup-route` removes the promoted routes within 10 s, and the network
falls back to the primary exactly as it would if the hotspot had gone away. The service
keeps retrying (never-give-up) and nothing else changes.

To remove it from the flake instead: drop `./wan-backup.nix` from `./default.nix`, the two
masquerade rows from `./firewall.nix`, `wan_backup_wifi_psk` from `./sops.nix` and
`secrets/secrets.yaml`, and the `2-wwan` link rule from `./hardware-configuration.nix`,
then `just check-eval && just switch && sudo nixos-confirm`. Do not remove the sops key
while the module still reads it — that fails activation, on the router, where the network
is.
