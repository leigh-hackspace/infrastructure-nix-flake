# Thread border router on services1 — 2026-11-26

**Status: planned, guarded, and deliberately not deployed.** The flake gained the
plan — a radio table, an eval-only guard, two disabled services
(`infra.thread.enable`, `infra.matter.enable`, both false), the firmware pins
(`radios/firmware-pins.json`) and the verbs (`just radio-status`, `just radio-plug`,
`just thread-pins`, `just thread-fw-fetch`, `just thread-fw-verify`,
`just thread-status`) — and it gained the measurement that says the dongle this
network was **offered** is not the radio the plan needs. Nothing here was flashed
and nothing here was switched. If that sentence is what you came to re-read, read
the two sections under it before you try the dongle again.

The plan: a Thread border router on `services1` so that Matter-over-Thread devices
(Home Assistant's `matter` integration, a Matter camera, the mesh that carries
Matter traffic) have a network to send IPv6 into, next to the Zigbee coordinator
that already runs there.

## The dongle the network was offered is not a Thread radio

The spare dongle is a Sonoff **ZBDongle-P**, TI **CC2652P**. It is *not* a
typo — a Sonoff **Dongle Plus (CC2674P10)** is a different, Thread-certified
device, and the two are one product line apart. Four receipts, three read today,
one read a year ago:

1. **The firmware for it exists, and this network has it.** `Koenkk/`
   `OpenThread-TexasInstruments-firmware` release `2025.3.1` (its only release)
   carries `CC1352P2_CC2652P_launchpad_ot_rcp_2025_3_1.zip`, and that repository's
   `docs/adapters.md` puts SONOFF ZBDongle-P in the launchpad row. Pulled and
   hashed on 2026-11-26 into `radios/firmware-pins.json`: 92 423 bytes, the zip,
   and the `.hex` inside it (228 114 bytes) each have their own digest pinned,
   because the file that gets flashed is not the file that gets downloaded.
2. **Upstream's own issue tracker says it fails on this dongle, in this way.**
   `Koenkk/OpenThread-TexasInstruments-firmware` discussion #4 is titled
   "Sonoff Dongle-P firmware not working when joining device": correct image,
   correct dongle, and `otbr-agent` dies with `RadioSpinelNoResponse` and
   "no response from RCP during initialization". That is also the signature any
   wrong-URL config produces, which is why it reads as a config error and is not
   one (`P-RadioSpinel` answers `Wait for response timeout` then blames either
   the URL or the RCP image, and it is the image here).
3. **The reason is a size, not a bug.** The current Home Assistant Thread guide
   states the figures: the CC2652P has **80 KB RAM / 352 KB flash**, and the guide
   calls the OpenThread route for TI CCxx52x sticks "doesn't exist" — the
   certified Thread Zigbee sticks are **EFR32MG24** with **256 KB RAM /
   1536 KB flash**. Thread's radio tables are what does not fit in 80 KB. The
   same 2026 field guide that walks this swap end to end — a retired ZBDongle-E
   flashed to a Thread radio from inside Home Assistant in under two minutes —
   marks its own stick table **Dongle-E (EFR32MG21) yes, Dongle Plus MG24 yes,
   Connect ZBT-2 (MG24) yes, SONOFF ZBDongle-P (CC2652P) *no*** and writes the
   reason as "sticks with a Texas Instruments chip like the ZBDongle-P won't
   work: this firmware route doesn't exist for them"
   (`https://raspberry.tips/en/smart-home/thread-border-router-home-assistant`).
   So upstream disagrees about whether the route *exists* — receipt 1 says the
   image is distributed and hashed, receipt 2 says someone put that image on this
   dongle and the RCP never answered — and agrees about what it *does*: it does
   not work on this silicon. That is what `flashed: false` and `do not` encode. The
   TI e2e forum reports the same signature on a LAUNCHXL-CC26X2R1 after roughly
   six sensors and a few minutes: the R7-class TI silicon holds enough for a *small*
   mesh and runs out of it, so the TI line is a size cliff, not a broken radio.
4. **This repo has no first-hand measurement of a flashed CC2652P** — `flashed:
   false` on every row of `radios/firmware-pins.json`, and the `verdict` column
   says which measurements are upstream's rather than this network's. If someone
   flashes one, that row is the one to update, and the doc to update it in is this
   one.

## What radio this network should get instead

Two families hold Thread-certified silicon (or a Thread-shaped RCP that the
current Home Assistant Thread integration supports), and both are in
`radios/firmware-pins.json`:

- **silicon-glass, the cheap answer**: EFR32MG21 (Sonoff ZBDongle-E / Dongle
  Lite MG21) and EFR32MG24 (Sonoff Dongle Plus MG24, Dongle Max, Dongle-M). The
  current Thread guide lists the five certified sticks — Dongle Plus (MG24),
  Dongle Max (MG24), Dongle Plus-2P (pass-through), Dongle Plus-2U (pass-through),
  Dongle-L — and says "not all have been tested", so the list is not a receipt
  about any one device. `just thread-fw-fetch silabs-mg21-zbdonglee-rcp` pulls
  darkxst's 2.5.3.0 RCP; the pinned version is deliberate: **2.7.0.0**, the
  highest number, in the release named `-Next`, was measured (by the guide this
  network followed, in 2025) to crash the radio every few seconds — `radio tx
  timeout`, `RCP failure detected`, `Software reset co-processor`, `RadioSpinelNo
  Response` — while 2.5.3.0 ran clean. Pinning the highest is how this whole
  class of "latest is worse" gets reintroduced by the next reader.
- **TI, only on the R7-class**: LP-CC2652R7, the CC2652R7 row in
  `radios/firmware-pins.json`. Thread-certified TI silicon, and untested here.

For this space the pick is **MG24-class**, and the sentence worth keeping is the
one that makes it a *choice and not an availability fact*: this network's Zigbee
coordinator is already an **SMLIGHT SLZB-06M**, an MG24-class module, and one
dongle cannot be both a Zigbee coordinator and a Thread radio — which is why the
plan needs a second dongle, and why the table's `thread-rcp.device` is empty
until someone plugs one in.

What the *spare* is good for, if the Thread radio has to be bought, is the other
mesh: a CC2652P is a working 802.15.4 radio, it just is not Thread-certified and
does not have the RAM for Thread's radio tables. Flashed with a **Zigbee Router**
firmware — a repeater, with no `pan_id` and no stack behind it — it joins the
network the live SLZB-06M runs and extends it, which is why the table has a
`zigbee-router` role and the guard makes its channel *equal* to the mesh's rather
than 8 away. It is an option, not medicine: the house whose repeaters were
flashed and plugged in around a 150-device mesh still had ~50 nodes at bad link
quality, and what fixed it was moving the coordinator to the middle of the house.
The decision belongs to the mesh's own numbers, and the flash belongs to a
row of `radios/firmware-pins.json` that does not exist yet — pin the artifact
before flashing it, not after.

## The wire is the same wire, and the channels have to be apart

Zigbee and Thread are both **IEEE 802.15.4 on 2.4 GHz**, channels **11-26**,
which is the same 2.4 GHz this building's Wi-Fi is full of. Measured on
services1 2026-11-26: zigbee2mqtt's runtime config says `advanced: channel: 11`
(`pan_id: 12345`) for the SLZB-06M, and `common/radio-dongles.nix` puts the
Thread row at **26**.

The reason they must differ: two radios on one channel drown each other, and
Thread and Zigbee *can* share one channel only under **MultiPAN** — one radio
chip serving both networks — which Home Assistant marks **deprecated** and which
this network does not use. "Set both to the same channel" is what the guides that
run one dongle for both recommend, and it is the advice that will quietly break
this box once the Thread radio is enabled: the guard in `common/radio-tables.nix`
refuses a Thread channel within 8 of any Zigbee channel, so the sentence has to
be read, not re-derived. The distance is the rule; **26 is a pick**, not a law — it is
matter.js's default and the end of the band, and a guide that starts Zigbee at 25 puts
Thread at 15. One more measured pair of numbers for the same rule, from a building whose
Wi-Fi access points sat on **2 and 9**: an openHAB mesh dropped nodes off Thread's default
**15** and moved it to **23**, with its Zigbee mesh on **25**. The channel has to clear
*every* 2.4 GHz speaker on the network, not just the other mesh in the house — which on this
network means the hub's Wi-Fi radios too, and `docs/router-and-dns.md` is where those live.

## This box's IPv6 is not Thread-ready, and this repo does not fix that

Measured on services1 2026-11-26:

```
net.ipv6.conf.all.forwarding                   = 0
net.ipv6.conf.vlan225.forwarding               = 0
net.ipv6.conf.vlan225.accept_ra                = 0
net.ipv6.conf.vlan225.accept_ra_rt_info_max_plen = 0
net.ipv6.conf.all.accept_ra                    = 1
/proc/net/if_inet6: vlan225 fd99:dead:beef:: (network's own), 2001:08b0:... (tunnel),
                vlan1    fd00:0000:0001:1::, vlan201 fd00:0000:00c9:1::, vlan226 ...,
                vlan230 ..., veth* (podman) ..., lo
```

Three things about those numbers, in the order that costs people time:

1. **Thread speaks IPv6 only**, and both `forwarding` (kernel routing) and
   `accept_ra` (kernel router-advertisement acceptance) read 0 on the interface
   this box's LAN IPv6 actually lives on, while `accept_ra_rt_info_max_plen` —
   the value systemd-tmpfiles names for exactly this purpose, and that the
   OpenThread guide sets to the prefix of the network — reads 0 too.
2. **The two upstream docs disagree about the first one**, so nothing here sets
   it: the Matter server's own OS requirements say *not* to enable
   `forwarding` with a networkd-managed setup and instead pin
   `PRIMARY_INTERFACE`, which is what `matter-server.nix` does; the OpenThread
   guide says to set `ipv6.conf.<lan>.forwarding` to 1 and give the border router
   that interface. Both are upstream, both are plausible, and this network's
   answer is **measured**: gw's NAT splits egress by VLAN
   (`docs/router-and-dns.md`, "Which uplink each VLAN leaves by") and the tunnel
   is what carries VLAN 225's IPv4 and *all* IPv6, so kernel forwarding on
   `vlan225` is a question about that box's rules as much as this one's.
3. **Matter's own caveat is the one that makes this the interesting part**:
   Matter uses link-local multicast, and link-local addresses are per-interface,
   so a device on a *different* VLAN tag is not guaranteed to see this box's
   pool. This box sits on VLAN 225 (`machines/services1/networking.nix`), which
   is also where the space's devices get their leases — so the border router's
   `BACKBONE_IF` is `vlan225` — but nothing in this repo has measured whether a
   Thread mesh reaches a VLAN-tagged NIC on the same hub. That open item is the
   real blocker, and it is a question about the switch and the router, not about
   a container.

One more read of that question, from a network shaped like ours: a VLAN-aware setup
(`br100`, `br100.10`, `br100.20` — VLAN tags as interfaces of their own, on Proxmox and on
VyOS) found Router Advertisement **disabled on both hosts** and the Thread routes did not
reach the clients until *every* interface along the chain got `accept_ra = 2`,
`accept_ra_rt_info_max_plen = 64` and `forwarding = 1`, including the VLAN-tagged pairs
(`vmbr0/10`, `br100/10`, …): `https://www.vanwerkhoven.org/blog/2026/upgrading-from-zigbee-to-thread-or-not/`
is that write-up. It closes the disagreement above rather than adding a third vote: the
**intermediate** host of a VLAN-aware network needs forwarding, the **client** end needs RA
acceptance, and 64 is the RIO prefix length every working recipe names. What ours is not:
the sysctl keys on both boxes answer for `vlan225`, `vlan226`, `router-lan`, `router-uplink`
… as **plain named interfaces**, with no `iface/tag` key on either, because our VLAN tags are
`slide` links with their own names rather than kernel VLAN pairs — so that recipe lands here
as one row per named interface, and the pair half of it does not apply.

The same write-up names a second thing this network has not considered: Thread's discovery
half is **IPv6 mDNS**, and "mDNS does not travel from zone to zone but instead to the router
and is then reflected outward again", so their VLAN-aware network needed an explicit mDNS
**reflector** (`service mdns repeater interface 'br100.20'`) plus firewall rules from every
zone *into* the LOCAL zone — two hours of their life, they say. `gw` runs dnsmasq, which
answers DNS, DHCP and the NAT sets; nothing on this network reflects mDNS. So a Thread mesh
that has to cross our VLAN tags is plausibly two questions, not one: the RA question above,
and an mDNS question one layer up. Neither is guessable from the container, and neither is
answered here.

This section deliberately stops at the measurement. **No sysctl, no
`IPv6AcceptRA` row and no firewall change has been made here**, and the
`just check-eval` gates pass with the Thread services off, so the half of the plan
that is *not* blocked ships while the half that is blocked stays open.

## Where the pieces live, and what Home Assistant's side is

| Piece | Where |
| --- | --- |
| the radios, their roles, channels, device nodes | `common/radio-dongles.nix` (DATA), read by `zigbee2mqtt.nix`, `thread-router.nix` and `matter-server.nix` |
| the rules over that table, in one implementation | `common/radio-tables.nix` (HELPER) |
| the guard, run by `just check-eval` | `common/check-radio-tables.nix` (CHECK) |
| the border router (OpenThread + the radio) | `machines/services1/services/thread-router.nix`, `infra.thread.enable = false` |
| the Matter controller | `machines/services1/services/matter-server.nix`, `infra.matter.enable = false` |
| the firmware artifacts and their digests | `radios/firmware-pins.json` |
| the verbs | `just/radios.just` |

Home Assistant's side, measured against this box's pinned core **2026.11.0**:

- **The Thread integration is core, so it is here** — but the border router that
  feeds it is an **app**, and apps are a **Supervisor** feature. This box runs
  Home Assistant **Container** under podman
  (`machines/services1/services/home-assistant.nix`), so the guide's "install the
  OpenThread Border Router app" has no meaning here, and the flake owns the
  process instead — the same reason `zigbee2mqtt.nix` owns zigbee2mqtt and
  `mqtt.nix` owns the broker. HA finds the border router the way it is documented
  to (mDNS/DNS-SD on the LAN), and the Thread integration's config flow appears
  by itself.
- **The Matter integration is core too**, and 2026.11's version is new enough for
  what its own docs call recent: Matter **network maps** (HA 2026.2+, 1.4+) and
  **Matter cameras** (HA 2026.6+, 1.5+), which is why the JS server's `0.7.x` pin
  is comfortably inside what the docs promise rather than next to it.
- **Commissioning needs Bluetooth, and this box has none.** `BLE_PROXY` is the
  shape (a `/ble` WebSocket endpoint HA drives from wherever the adapter lives),
  and it needs HA 2026.6+ (satisfied) and an ESPHome device hosting the bridge.
  `BLUETOOTH_ADAPTER` is not an option: this box's `/sys/class/bluetooth/` is
  empty and `lsusb` has no Bluetooth device, facts already recorded in
  `machines/services1/services/home-assistant.nix`'s header for the same reason.

### The REST port is not the guide's, and that is a measured collision

Every guide puts OpenThread's REST API on `:8081`. On this box that port is already
taken: `machines/services1/services/hackspace-api.nix` has `SPACE_API_PORT = 8081`
and `ports = ["8081:80"]`, so the Space API — the one endpoint on this network with
an audience off it, 7,939 external `/space.json` requests in 40 hours — answers on
`127.0.0.1:8081`, and `hackspace-mcp` dials exactly that as its `--space-url`.
Measured 2026-11-26 with `curl -i http://127.0.0.1:8081/`: a uvicorn `307` to `/docs`,
while `ss -ltnp` from the host's view shows nothing listening there — the socket is
there (`/proc/net/tcp` answers `0100007F:1FA1` type `0A`), it is just podman-published,
which is why one tool sees it and the other does not. So this flake takes `8196`
(REST) and `8098` (web UI), next to the band this network already uses — 8088 status-
dashboard, 8095-8097 the apps, 8191-8195 the bot and the voice tier — and all three
of `8196`, `8098` and the Matter server's `5580` answered connection-refused on
2026-11-26. `just thread-status` prints that check, so the collision is a number and
not a paragraph.

**Proven on the box vs. still a guess**, in this plan's own words:

- proven, 2026-11-26: this box is `services1` and runs HA Container 2026.11.0;
  one dongle is plugged in and it is the SMLIGHT SLZB-06M, the live Zigbee
  coordinator, on channel 11; the IPv6 sysctls and interface list above; the
  firmware digests; the registry tag lists — `smnws/ha-otbr-docker` has
  `latest`/`edge`/`edge-<sha>`/`sha-<hex>` and **no version tag**, while
  `home-assistant/app-openthread-border-router` has **no `latest`** but does
  have `stable`, `3.4.0`, `dev`, seven commit tags and seven `sha256-<hex>`
  build-image tags, so the app image is *pinnable* and the reason the flake
  carries the third-party one is that that one is built to run **without
  Home Assistant OS's Supervisor** — which is what this box runs — and pins
  the same two upstream commits the app does; and that the guards pass,
  and that they **fail** when the Thread row is given the Zigbee channel (the
  verified-to-fail test for this guard, in this repo's habit).
- still a guess: whether the space has any Matter device today (nothing in the
  flake or in HA's `.storage` names Thread or Matter, and nothing was commissioned
  — HA's integrations are runtime state in a browser); whether the space has an
  MG24-class spare dongle to plug in; whether a Thread mesh crosses VLAN tags on
  this hub; whether `127.0.0.1:8081` survives the front door's rules for HA's
  REST calls (nothing on this network has yet asked a Thread device to reach the
  MQTT broker, which is why `NAT64 = "0"` is written as a decision point and not
  a fact).

## 12. What this session added, and the two refusals that fired

The table, the guard, the pins, and the verbs are this session's, and the
interesting part is not that they pass — it is the two refusals that were made
to fire, because a guard that has never refused is a guard nobody has read.

**The table is one file, the rules over it are another, and five places read
them.** `common/radio-dongles.nix` (DATA), `common/radio-tables.nix`
(HELPER), `common/check-radio-tables.nix` (CHECK) — the three of `common/`
are each labelled at the top because the directory mixes all five kinds
(AGENTS.md's rule about `common/`). The guard is evaluated from
`just check-eval` (so it runs on `gw`, which cannot run the flake step —
AGENTS.md rule 22), from `flake.nix`'s `checks` (so `nix flake check` and
`nix eval` see it too), and as a `config.assertions` of `zigbee2mqtt.nix`,
`thread-router.nix`, and `matter-server.nix` — a service's own assertion is
the form that stays charged when someone changes a table, so the same rule is
enforced where the table is used, not only where it is checked. One
implementation, five entry points, the same shape AGENTS.md rule 24 asks of a
vhost move.

**Refusal 1 — a Thread radio on a Zigbee's channel.** The guard was written
with the upstream pick baked in (`thread-rcp.channel >= 25`) and then
deliberately broken (26 → 11) to see it answer; it refused and named both rows.
Fixing it was a decision, not a patch: 25 or 26 is **matter.js's default**, not
the law — the whole point of the two upstream guides that start Zigbee at 25 is
that they put Thread at 15 — so the rule is the **distance**, which is what the
collision is: `thread-rcp.channel` must be in 802.15.4's band and at least 8
channels from every channel some Zigbee is using. 26 stays the table's value,
with the reason written next to it (it is matter.js's pick, it is the end of
the band, and this flake is the thing that decides), and the refusal now says
"26 is not a law, the distance is".

**Refusal 2 — a Zigbee repeater modelled as a second coordinator.** The
`zigbee-router` role was added (a repeater is neither a coordinator nor a
spare: it joins the network the coordinator's stack runs, and no stack talks to
it) and a row was planted with `channel = 12` and a `mountAt` to prove the
guard sees both halves of the rule: a `zigbee-router` must answer on one of the
mesh's own channels and must have no `mountAt`, because nothing is behind it.
It refused, naming the row. The row was then deleted; the table has no repeater
today, because no Zigbee-Router artifact is pinned and the decision is open
(`follow-ups.md`).

**The channel table agrees with the runtime, and that is the receipt.** The
guard checks the table against itself; what it cannot check is the table
against Z2M's own config, so the pair was read: the table says
`channel = 11` for the coordinator, `/srv/zigbee2mqtt/configuration.yaml` says
`advanced: channel: 11`, and the table's `mountAt` (`/dev/ttyUSB0`) equals the
runtime's `serial.port` — `zigbee2mqtt.nix` was then changed to *generate* the
`--device` argument from the table instead of hardcoding the node, so the two
cannot drift again. `just radio-status` is the verb that prints all three
together and fails when they disagree.

**The measured facts that the table and the pins carry**, and their sources —
read, not assumed, this session:

| fact | value | where it was read |
| --- | --- | --- |
| CC2652P class RAM / flash | 80 KB / 352 KB | `raspberry.tips` and Koenkk's `docs/adapters.md` |
| EFR32MG24 / CC2652R7 class RAM / flash | 256 KB / 1536 KB | the same table's "Use more RAM" column |
| the CC2652P-class RCP zip, its hex, and what the hex carries | 92 423 bytes; 228 114 bytes; **81 068 data bytes** (79 KiB) over 5 074 records, highest linear address `0x58000`, two type-`02` offset records | fetched and measured; digest matched the pin |
| the CC2652R7-class RCP zip, its hex, and what the hex carries | 91 600 bytes; 226 457 bytes; **80 480 data bytes** over 5 037 records, highest linear address `0xb0000` | fetched and measured; digest matched. **Same content size, higher link address** — so the image-size argument in the field guides does not hold on this network's own numbers, and what separates the classes is RAM: upstream's table, 80 KB against 256 KB |
| the two Silabs `.gbl`s (MG21, MG24) | 110 540 bytes; 125 180 bytes | fetched and measured: digests matched |
| Zigbee-Router adapter list | does **not** include ZBDongle-P | `Koenkk/zigbee-adapter-docs` `docs/adapters.md` |
| OTBR image build args | `OTBR_OPEN_THREAD=2025.03.19`, `OTBR_OPEN_THREAD_URL` = `main`, `OTBR_VERSION=cb4bdae242…`, `OTBR_SUBMODULES_JSON` pins `admux_proto` at `2025.07.13` (truncated in the read) | `ghcr.io/smnws/ha-otbr-docker`'s `build.args` |
| the pty radio form | `spinel+hdlc+forkpty://${PATH_TO_EXECUTABLE}?…` | `ot-extcap`'s own help string |
| this box's three RA sysctls | 0 / 0 / 0 on `vlan225`; `all.accept_ra` = 1 | `/proc/sys/net/ipv6/conf/*` |
| this box's Bluetooth | none (`/sys/class/bluetooth/` empty, no USB BT device) | `just thread-status` |
| the REST port choice | `8081` is matter.js's default and this network's Space API; 49 listening ports on the box; `8196`/`8098` answer nothing | `/proc/net/tcp` + `curl` |
| the two pinned digests | all four artifacts matched their pins on fetch | `just thread-fw-verify` |

`radios/README.md` owns the rules a flash makes permanent (a flash is a one-way
door per dongle; a flash does not change a dongle's USB ids, so the table's
*role* is the fact that survives one), and it is a pointer to the firmware's
own repositories for the firmware facts — AGENTS.md rule 30's shape again:
OpenThread's tree and Koenkk's repositories own what the firmware is and does,
and this flake owns the pins, the verbs, and the network.

## What is open

In `docs/follow-ups.md`, under this doc's name: the radio that has to arrive,
the IPv6 answer, the BLE bridge for commissioning, `NAT64` for MQTT-over-Thread,
and the border-router image's moving `:latest` (it has no version tag to pin, while Home Assistant's own app image does).

## Re-verify with

```
just check-eval          # the radio-table guard runs here, on every box including the router
just radio-status        # table vs USB ports vs the channels each stack is using
just thread-pins         # the pinned artifacts, and whether the upstream tags still answer
nix eval --impure --raw -f common/check-radio-tables.nix   # must print ok
just thread-fw-verify radios/fw/<file>                     # the digest gate before flashing anything
```
