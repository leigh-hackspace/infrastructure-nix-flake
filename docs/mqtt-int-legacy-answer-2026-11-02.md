# Home Assistant's Frigate cameras read "Unavailable": `mqtt.int` answers with the front door, and the broker's port does not live at the front door

**Dated 2026-11-02 and closed 2026-11-03, `services1` and `gw`, with the three clients and
the broker read on the boxes.** **Status: closed.** The outage ended on 2026-11-02 with all
three dials retargeted to the broker's address by hand; the declarative fix — `mqtt` in
`common/int-dns.nix`'s new `answeredBy` group, answered with services1 — **deployed to `gw`
at 01:09 on 2026-11-03**, and all three dials then went back to the **name** (01:12–01:14),
so no client hardcodes an address any more and all eight Frigate camera entities are
`recording` again (HA's own state history, §"Deployed"). Everything that was pending when
this file was first written is done; what is left is the tail of `docs/follow-ups.md`
(`tailscale.int`'s equivalent check, and Home Assistant's unpinned `:stable` tag).
The symptom was in Home Assistant (apps1, 10.3.1.30): every Frigate **camera** entity —
`camera.rack_corridor`, `camera.main_space`, `camera.social_space`, `camera.woodwork`,
`camera.workshop`, `camera.pi_room`, `camera.cam8`, `camera.cnc` — rendered **Unavailable**,
while the Frigate **person-count / occupancy / fps sensors** on the same cameras showed live
numbers. The cause is not Frigate, not the cameras, and not Home Assistant's version: it is
the answer of one `*.int` name for one **non-HTTP** port. Read the pair with
`docs/homeassistant-frigate-via-device-2026-11-02.md`, which is the *other* end-of-October
fault on the same box — that one stopped the camera entities from being **created**, this one
has kept them **unavailable** since the front door moved.

## The shape that makes the symptom look like a dead camera

The Frigate HA integration delivers the camera entities over **MQTT**, not HTTP
(`/srv/homeassistant/custom_components/frigate/`, read on apps1):

```python
camera.py:159   class FrigateCamera(FrigateMQTTEntity, CoordinatorEntity[FrigateDataUpdateCoordinator], Camera)
__init__.py:741 FrigateMQTTEntity.__init__          self._available = False
__init__.py:719 available()                         self._available and super().available
__init__.py:746 self._available = any(topic == f"{topic_prefix}/available" …)   # the flip
camera.py:205   subscribes f"{topic_prefix}/{camera}/{enabled|motion|recordings}" / state
__init__.py:84  SCAN_INTERVAL = 5 s                 # the *sensors* are CoordinatorEntity over HTTP
```

A Frigate camera is therefore `available=False` from construction until the **broker** sends
it a message, and the sensors have no such dependency. "Sensors counting, cameras grey" is
what a dead MQTT link looks like by design — it is not a camera fault. (`camera.py:321` has a
second unavailability rule — `camera_fps == 0` in Frigate's `/api/stats` — ruled out here: all
eight cameras were at 3.2–7.2 fps.)

## The three dials, all off-flake, all naming the same name

```bash
# Home Assistant's MQTT config entry — apps1, runtime state, not in this flake
sudo python3 -c "
import json; d=json.load(open('/srv/homeassistant/.storage/core.config_entries'))
[print(e['entry_id'], e['domain'], e['data'].get('broker'), e['data'].get('port'), e['data'].get('protocol'))
 for e in d['data']['entries'] if (e.get('data') or {}).get('broker')]"
# d0c4ee0e892a913b3a0dd4a968f132e5 mqtt mqtt.int.leighhack.org 1883 5

# Frigate's publisher — services1, runtime config the flake only bind-mounts
sudo grep -n -A4 '^mqtt:' /srv/frigate/config/config.yaml
# 1:mqtt:
# 2:  enabled: true
# 3:  host: mqtt.int.leighhack.org

# zigbee2mqtt — services1, runtime config the flake only bind-mounts
#   (machines/services1/services/zigbee2mqtt.nix mounts /srv/zigbee2mqtt as /app/data;
#    which broker to dial is in the runtime file, not in the module)
sudo grep -n -A3 '^mqtt:' /srv/zigbee2mqtt/configuration.yaml
#  7:mqtt:
#  8:  base_topic: zigbee2mqttpoe
#  9:  server: mqtt://mqtt.int.leighhack.org
```

Three clients, one name. Nobody had to see the breakage in the same place: zigbee2mqtt's
smart-device entities simply go dark (its own log stops publishing), and the flake has no
word for any of the three — searching the tree for `mosquitto` finds the module, one line of
`docs/monster-proxmox.md`, and the `legacy` row.

`mqtt` is in `common/int-dns.nix`'s **`legacy`** set — "the front door answers, no vhost serves
it" — and `frontDoor` moved from `services1` to `gw` on 2026-10-24
(`docs/router-front-door-migration.md`). So the name's answer moved **with the door**. What it
answers now, read off the router:

```bash
sudo grep mqtt /etc/dnsmasq-int-vhosts.hosts        # gw; the file is the store path the last deploy wrote
# 10.3.1.1                   mqtt.int.leighhack.org
# 2001:8b0:1d14:225::1       mqtt.int.leighhack.org
# fd99:dead:beef:225d::1     mqtt.int.leighhack.org
ls -la /etc/dnsmasq-int-vhosts.hosts                # -> /etc/static/…, written 2026-10-31 20:05
```

and the broker is the podman `eclipse-mosquitto` container on **services1**
(`machines/services1/services/mqtt.nix`, `listener 1883`, `allow_anonymous true`, published
`0.0.0.0:1883`). Nothing on `gw` listens on 1883, and the door has no vhost there either —
`mqtt.nix`'s `ports` are the only thing that ever put a socket on that port.

## Measured, from apps1

```
10.3.1.20:1883   accepted, bytes reach mosquitto (its journal logged `New connection from 10.3.1.30…`)
10.3.1.1:1883    Connection timeout after 6–8 s        # what the name answers with now
```

Home Assistant says the same in its own words, once per start — three starts in journald:

```
2026-10-31T11:54:52 ERROR [homeassistant.components.mqtt.client] Failed to connect to MQTT server due to exception: timed out
2026-10-31T13:24:52 … same
2026-11-02T20:09:48 … same
```

## The broker's own log is dated, and it is the whole story

```bash
sudo journalctl -u podman-mqtt.service --since "2026-10-24" | grep -E "New client connected|disconnected"
# 2026-10-24T02:00:20  New client connected from 10.88.0.1  … as mqttjs_355b188d            (p4, c1, k60)
# 2026-10-24T02:00:24  New client connected from 10.3.1.30  … as auto-E866FDB8-…            (p4, c1, k60)
# 2026-10-24T02:00:29  New client connected from 10.3.1.30  … as 06PvjqdAD33kgxuwn5PsZk     (p5, c1, k60)
# 2026-10-31T11:39:38  Client auto-E866FDB8-… [10.3.1.30] disconnected: exceeded timeout.
# 2026-10-31T11:39:45  Client 06PvjqdAD33kgxuwn5PsZk [10.3.1.30] disconnected: exceeded timeout.
# 2026-10-31T18:31:11  Client frigate [10.88.0.1] disconnected.
#      … and no successful CONNECT from anything until 2026-11-02 22:21
```

`10.3.1.30` is apps1 (Home Assistant), `10.88.0.1` is a podman container network on services1
(Frigate, and the JS client). So MQTT was **alive on 2026-10-24** — the day the front-door move
deployed, and the day the mqtt container was (re)created, both clients reconnecting — and
**dead from 2026-10-31**: Home Assistant's two clients died when it restarted at 11:48, Frigate's
`frigate` client died at 18:31 when the container restarted at 20:03, and neither reconnected
after that, because both now dial the name. Frigate's own log says the same:

```
2026-10-24T02:00:19 [frigate.comms.mqtt] ERROR: MQTT disconnected     # then it reconnected
2026-10-31T18:31:11 [frigate.comms.mqtt] ERROR: MQTT disconnected     # and never did again
```

`docs/router-and-dns.md`'s excuse for a `legacy` name 404ing — *"dnsmasq is answering, nginx
isn't"* — is only true of HTTP. A client dialling a **socket** on that name is not 404ed, it is
dialed at an address where nothing listens, and the failure is a hang. That is the class of
breakage the 2026-10-24 front-door migration could not see: the three dials are runtime files
that live on other boxes — `apps1`'s `.storage/core.config_entries`, services1's bind-mounted
`/srv/frigate/config/config.yaml` and `/srv/zigbee2mqtt/configuration.yaml` — no vhost and no
guard reads them, and no `git log` line
touches them. The `legacy` row's own comment hedged exactly this: *"`mqtt = "mosquitto speaks
MQTT on its own ports, not HTTP; this name has never been fronted"`*".

## The repair, 2026-11-02 22:2x

*(This half is the emergency repair — an address hardcoded three times. It was reversed on
2026-11-03 once the name answered correctly; see "Deployed" below. Nothing below this
heading should be read as the end state.)*

All three clients now dial the broker's address instead of the name — a line each in the three
off-flake runtime files, then one restart each, restart and confirmation in the same
invocation:

```bash
# services1: Frigate's publisher
ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20 \
  'sudo python3 - <<PY   # one host line inside the mqtt: block, host verified to be the broker's
   …  host: mqtt.int.leighhack.org -> host: 10.3.1.20  # + a comment saying why
   PY
   sudo systemctl restart podman-frigate.service && systemctl is-active podman-frigate.service'

# apps1 (Proxmox VM 109, root keyed): Home Assistant's MQTT config entry
ssh -i ~/.ssh/agent-hop-key root@10.3.1.30 \
  'python3 - <<PY   # json-edit .storage/core.config_entries: broker -> 10.3.1.20, one entry only
   PY
   systemctl restart home-assistant.service && systemctl is-active home-assistant.service'
```

Frigate's config change did **not** hot-reload (its watcher did not fire for a host-side
rewrite that replaces the inode), so the restart was explicit. The broker then logged both
clients again, first time since 2026-10-24:

```
2026-11-02T22:21:20  New client connected from 10.3.1.30:35927  … as 1W9tVOb6KGNP3zHyZf1PCY  (p5, c1, k60)
2026-11-02T22:24:15  New client connected from 10.88.0.1:60055  … as frigate                 (p4, c1, k60)
2026-11-02T22:33:13  New client connected from 10.88.0.1:54164  … as mqttjs_5680e197          (p4, c1, k60)
```

(the third is zigbee2mqtt, whose first act after reconnecting was to publish the whole
`homeassistant/sensor/…` entity-config set again — its half of the same MQTT link, which had
been dark since 2026-10-24 like Frigate's.)

and Home Assistant's run logged **0** `Failed to connect to MQTT` and immediately started
*using* the topics — 22:25:15, an event snapshot fetched over the front door:

```
10.3.1.30 - - [02/Nov/2026:22:25:15 +0000] "GET /api/events/1793658314.415069-os5drk/snapshot.jpg HTTP/1.1" 200 837837 "-" "HomeAssistant/2026.10.4 …"
```

That is the end-to-end proof: Frigate publishes, Home Assistant receives, the availability
flip (`__init__.py:746`) happens, and the camera entities stop being Unavailable.

## The decision, taken 2026-11-02, deployed 2026-11-03

**(a) chosen**: answer `mqtt.int` with the box that runs the socket. **(b) rejected**
before it was attempted — moving the broker to `gw` would put a podman container and its
`/srv/mosquitto` state on the box that is the network's router, front door, TLS
terminator, dnsmasq and UPS host, for three clients that live on services1 and apps1, and
it would buy the name nothing that (a) buys. Both halves of (a) are a move of a *port*, so
the rule-24 discipline applies: table, renderer and guards in one change, deployed in one
activation.

| Half | Where | State |
| --- | --- | --- |
| the answer | `common/int-dns.nix`'s new `answeredBy` group, `mqtt.via = "services1"` | landed 2026-11-02 |
| the rendering | `machines/gw/lib/network-tables.nix`'s `intHosts`: a third branch, one IPv4 line per name, no bare-name alias | landed 2026-11-02 |
| the guards | `machines/gw/lib/check-network-tables.nix`: `via` must be a machine in `common/hosts.nix`, the label must be in no other group and no record, the name must render as exactly one line carrying that address, and the file's total line count counts this group too | landed 2026-11-02, `just check-eval` green |
| the deploy | `just deploy gw` — one activation; `int-dns` is a `restartTriggers` value, so the new file and the dnsmasq that reads it at startup land together | **done 2026-11-03 01:09** (it took a detour: the router's checkout had diverged, see "Deployed") |
| the clients | the three dials back on the *name*: `mqtt://mqtt.int.leighhack.org` in HA's config entry, Frigate's `mqtt.host`, zigbee2mqtt's `server:` — the hardcoded `10.3.1.20`s removed | **done 2026-11-03 01:12–01:14**, after the answer was proved, in that order |
| the cert check | `sudo systemctl start front-door-cert-check.service` after any switch on gw, reading the issuer, and a look at the leases page afterwards | **done 2026-11-03 01:11**: `issuer=C=US, O=Let's Encrypt, CN=YE2`, `Result=success`, 44 leases in the file, `lease-reader-watch.timer` next fire 1 min 17 s out, dnsmasq advertising (7 `RTR-ADVERT(vlan225) 2001…` in 90 s) |

The order is not cosmetic: the deploy first, then the dials. Flipping the clients to the
name before the answer moves re-breaks all three at once; after the deploy the clients
that still dial the address keep working, so the flip is outage-free. And prove the
answer before pointing three clients at it — `dig +short @10.3.1.1 mqtt.int.leighhack.org`
answering `10.3.1.20`, then one socket probe from apps1 and the broker logging the
CONNECT, before any of the three restarts.

What the two existing guards do *not* do, and why that is the point: `mqtt` is in neither
`names` nor `exempt`, so `machines/gw/int-dns.nix` is untouched and stays green. That is
not an oversight — putting `mqtt` in `exempt` would claim a vhost serves MQTT, and that
assertion is precisely what refuses the claim, so no existing guard can be told about a
socket. The guard that owns this group is `check-network-tables.nix`, and it now asserts
the three things that can go wrong in a group no drift check reads: a typo in `via`
(`"servics1"` would answer a box that does not exist), a label that also lives in another
group (a name with two answers, which is the fault `check-network-tables.nix` names further
up), and a rendering that is not exactly one line — the last being what makes the
IPv4-only decision checkable rather than a comment.

## Deployed, 2026-11-03 01:09–01:19, with the receipts

The deploy went through `just deploy gw` **in a detached unit**, because the build of the
router's toplevel outlives a tool call; `docs/nix-dev-workflow.md` ("Building in a detached
unit") is the recipe and now carries the two traps this run added to it (the unit must run
as the *operator*, since the recipe's ssh needs the hop key and git needs `safe.directory`;
and a unit that exits in under a second has not started, so read its journal before its
log). The GOLDEN RULE survived the detour: `deploy` chains `nixos-rebuild switch &&
sudo nixos-confirm` **on gw** in one remote shell, and after it finished

```
readlink /run/current-system /nix/var/nix/profiles/system-good
  → …-nixos-system-gw-26.11.20261014.319e4ce   (both, identical)
git -C ~/Projects/infrastructure-nix-flake log --oneline -1 → e8f0a7e
systemctl is-system-running → running
```

It did not start clean: `git merge --ff-only` refused, because this box's checkout carried
three duplicated cam8 commits of its own — the story and the repair are
`machines/gw/README.md`. Nothing was switched by the failed attempt, which is the whole
value of the recipe refusing to merge.

**The answer, as dnsmasq serves it** (`/etc/dnsmasq-int-vhosts.hosts` on gw, generated):

```
118:10.3.1.20	mqtt.int.leighhack.org
100:10.3.1.1   zigbee2mqtt.int.leighhack.org      # the browser-facing ZigBee name, untouched
101:2001:8b0:1d14:225::1 zigbee2mqtt.int.leighhack.org
102:fd99:dead:beef:225d::1 zigbee2mqtt.int.leighhack.org

dig +short @127.0.0.1 mqtt.int.leighhack.org      → 10.3.1.20
dig +short @127.0.0.1 mqtt.int.leighhack.org AAAA → (nothing, on purpose)
getent ahosts mqtt.int.leighhack.org              → 10.3.1.20      on services1 *and* apps1
```

and the probe that had to precede the three restarts — from apps1, the box whose client had
been dead longest: `</dev/tcp/mqtt.int.leighhack.org/1883` → **OPEN**.

**The three dials, back on the name** (runtime files, each with a timestamped `.bak`
brought alongside, `20261103-011149` on services1):

| Client | File | Broker log after the restart |
| --- | --- | --- |
| zigbee2mqtt | `/srv/zigbee2mqtt/configuration.yaml` → `server: mqtt://mqtt.int.leighhack.org:1883` | `01:12:03 New client connected from 10.88.0.1:43990 as mqttjs_8e0e7a4c (p4, c1, k60)` |
| Frigate | `/srv/frigate/config/config.yaml` → `host: mqtt.int.leighhack.org` | `01:12:51 New client connected from 10.88.0.1:48603 as frigate (p4, c1, k60)` |
| Home Assistant | apps1 `.storage/core.config_entries`, `data → entries → [mqtt].data.broker = mqtt.int.leighhack.org` (edited with HA **stopped**: it rewrites that file from memory on shutdown) | `01:14:01 New client connected from 10.3.1.30:48859 as 4ZDDcKseb0DEwMkpVGc1A8 (p5, c1, k60)` |

**The end of the story is in HA's own state history**, which is the only place the original
symptom was ever recorded — `states` joined to `states_meta` (in this schema `states.entity_id`
is NULL; the id lives on the metadata row):

```
camera.cam8 / workshop / woodwork / social_space / main_space / rack_corridor   unavailable 01:12:51
                                                              → recording                  01:14:42
camera.cnc     recording 01:17:37        camera.pi_room  recording (by 01:19:15)
# 01:19:15, every one of the eight: recording. Not one `Detected code that calls
# device_registry…`, one `Error adding entity`, or one `Failed to connect to MQTT server`.
```

`frigate/available` is retained `online` on the broker (note: **one topic for the whole
server**, not `frigate/<camera>/available` — the `+` in that subscription finds nothing, and
`-t frigate/+/available` is a recipe that will mislead whoever reads this next). Two cameras
flickered `unavailable` for a few minutes during their own restart window because their
feeds were refusing connections at that moment (`dial tcp 10.3.2.10:554: connect: refused`
is cam5, i.e. `pi_room`, the known marginal camera) — a camera fault, and now cleanly
distinguishable from the MQTT one because the eight recovered and the MQTT error is gone.

## If it recurs

1. Read the **name's answer** first, on the box that answers for the network:
   `sudo grep mqtt /etc/dnsmasq-int-vhosts.hosts` on gw, and `dig +short mqtt.int.leighhack.org @10.3.1.1 A AAAA`.
2. Read the **broker's log** for client names and the addresses they arrive from:
   `sudo journalctl -u podman-mqtt.service | grep -E "New client connected|disconnected"` —
   that log alone usually tells you which client died and when, and it survives the client's
   own amnesia.
3. Read the **client's** complaint (`Failed to connect to MQTT server` on the HA side,
   `frigate.comms.mqtt ERROR: MQTT disconnected` on Frigate's) and the availability code,
   which is where "Unavailable" comes from: `_available = False` until a broker message arrives.
4. Only then suspect the camera, Frigate's fps, or the feed — this time all three were healthy
   throughout, and `podman ps` said so.
