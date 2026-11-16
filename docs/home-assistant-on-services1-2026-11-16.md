# Home Assistant on services1 — moving a live box off `apps1`, and leaving a proxy on the old address to find out who notices

Written 2026-11-16 from a session on **services1** (10.3.1.20), with every number below
measured on the two boxes before anything was switched.

**STATUS: deployed 2026-11-16 15:41–15:46 GMT, confirmed, and verified.** Downtime was the
interval between stopping the old container (15:41:04) and `podman-home-assistant` answering on
services1 (first `401` at 15:41:51, `Starting Home Assistant 2026.11.0` at 15:42:18) — **under a
minute**, of which the final data copy was 1.9 s. All receipts are in
"Deployed, 2026-11-16, with the receipts" at the end; **read that before this file's plan**, which
is the design as it was written and contains two things that turned out to be wrong about the
tooling (§"Two things the plan got wrong").

## What moved, and what did not

Home Assistant ran on **`apps1`** (10.3.1.30, Proxmox VM 109 on `monster1`) in a hand-written
`home-assistant.service` (`docs/monster-proxmox.md` owns that box). After the move it is a
podman container on **services1** (`machines/services1/services/home-assistant.nix`), and
`apps1` keeps running everything else it was running:

| On `apps1` | Still there? | Why that matters here |
| --- | --- | --- |
| `hackspaceapi` :8081, and three more front-doored origins on `:8082`/`:8083`/`:8084` | `:8081` **moved too, the same afternoon**; the three others stayed | Written when the Space API was still here — it was the only reason this was "never a retire-the-VM move", and it turned out small enough to move the same day (see "Then the Space API moved too"). `jenkins` `:8082`, `access-api` `:8083` and `user-tweaker` `:8084` remain, plus `cupsd` and this box's Prometheus, so `apps1` is still a box this network needs. |
| `cupsd` on `:631` | **yes** | HA's `ipp` entry "Big Laser Printer" dials `10.3.1.30:631`. After the move that is a *cross-host* call from services1; CUPS' `<Location />` says `Allow @LOCAL` and services1 is in the same `/24`, so it keeps working — re-verify after the switch rather than trusting the read of a config file. |
| `prometheus` (active+enabled) | **yes** | It scrapes HA: job `hass` → `http://apps1.int.leighhack.org:8123/api/prometheus`, and on `apps1` that name resolves to **`127.0.1.1`** via its own `/etc/hosts`. So HA's Prometheus endpoint is scraped over loopback on the old box, and it is `up` today. |
| `homeassistant` container, `/srv/homeassistant` | **stops** | The thing being moved. Its unit and image stay installed as the rollback. |
| `mosquitto`, `zigbee2mqtt`, `uptime-kuma`, `vulcan-bot` | already off | `docs/monster-proxmox.md`. |

The address `10.3.1.30` **cannot move to services1**, which is the one thing that makes this
different from the authentik `.36` precedent in `docs/authentik-container-migration.md`: there,
the old box was stopped, so the address could be taken as a second address on `vlan225`. Here
`apps1` still needs it for four front-doored origins, CUPS and Prometheus, and
`common/dns-records.nix` pins it to the VM's MAC `92:99:29:be:af:95`. Two boxes answering one
address is the silent-failure mode that document already warns about. So the old address stays
where it is, and the *service* at it is replaced by a forwarder.

## The client map, read before anything moved

Everything that dials HA, and what the move does to it. This is the whole reason the move is
safe to do at all: **four clients, and three of them dial a name.**

| Client | How it dials HA | After the move | Where the fix lives |
| --- | --- | --- | --- |
| services1's origin vhost (`ha.int.leighhack.org`) | `proxyPass http://10.3.1.30:8123`, twice (`/` and the `/auth/authorize` authentik rewrite) | repointed to `http://127.0.0.1:8123` | `machines/services1/http.nix` |
| `hackspace-mcp.service` (the Slack bot's MCP server) | `--hass-url http://10.3.1.30:8123` | repointed to `http://127.0.0.1:8123` | `mcpFlags` in `machines/services1/services/slack-agent.nix` |
| door entry system | `wss://ha.int.leighhack.org/api/websocket` | **nothing to change** — it names the door | already correct |
| gatus "Home Assistant" endpoint | `https://ha.int.leighhack.org`, `[STATUS] == 200` | **nothing to change** | `common/monitoring-endpoints.nix` |
| `apps1`'s Prometheus, job `hass` | `apps1.int…:8123` → `127.0.1.1` (its own `/etc/hosts`) | keeps scraping **through the legacy proxy**; should be repointed to `10.3.1.20:8123` when convenient | off-flake: `/srv/prometheus/prometheus.yml` on `apps1` |
| `crates/hackspace-mcp`'s compiled default `http://10.3.1.30:8123` | only if no flag is passed | the unit always passes `--hass-url`, so it is dead code as a default | leave, or bump the default later |

Two things that are *not* clients and got checked anyway because they look like ones: `grafana`
on `apps1` mentions no `8123`, and nothing else under `/srv` on that box does either except
`/srv/homeassistant.bak/` (an old copy) and `/srv/prometheus/`.

## The landmine: ZHA would have stolen zigbee2mqtt's radio

`core.config_entries` carries a **`zha`** entry named "Sonoff Zigbee 3.0 USB Dongle Plus" on
`/dev/ttyUSB0`. `apps1` **has no `/dev/ttyUSB0`** — measured, `ls /dev/ttyUSB* /dev/ttyACM*
/dev/serial/by-id/` all fail there — so that entry has never had a radio and ZHA is inert
config. The container ran `--privileged` anyway, which on `apps1` granted host `/dev` access to
nothing.

`services1`'s `/dev/ttyUSB0` is a different story:

```
/dev/serial/by-id/usb-SMLIGHT_SMLIGHT_SLZB-06M_be6f53…-if00-port0 -> ../../ttyUSB0
/srv/zigbee2mqtt/configuration.yaml:  port: /dev/ttyUSB0, adapter: ember
```

i.e. the coordinator **zigbee2mqtt already owns**. Copy `--privileged` across and the moved HA
finds a working radio, ZHA opens it, and zigbee2mqtt loses its port — and since every Zigbee
entity in this HA arrives *through zigbee2mqtt over MQTT* (there are also two `smlight` entries,
one at `10.3.2.2`), the visible outcome of a successful "move HA" would be the entire Zigbee
mesh going dark while HA itself looked healthy. So the new container passes **no devices and no
`--privileged`**: it reproduces the old box's *measured* behaviour, not its declared behaviour,
and it drops a root-level grant nobody was using.

This is the general shape worth keeping: when copying a container unit between boxes, the flag
list is not the truth about it — **what is plugged into the old box is**.

## The four container decisions, and why

`machines/services1/services/home-assistant.nix` carries the long version; the short one:

- **the host network** — the only host-networked container on this box, deliberately.
  `matter`/`thread`/`apple_tv`/`zeroconf` need multicast on the LAN, which a podman bridge does
  not carry, and direct-IP clients must arrive with their real source address. A
  `ports = ["8123:8123"]` instead would have kept browsers working, broken discovery, and made
  every direct client appear as `10.88.0.1`, which is in nobody's `trusted_proxies`.
  **How it is written is not how it was first written**: `networkMode = "host"` does not exist in
  this pinned nixpkgs' `oci-containers`, so it is `extraOptions = [ "--network=host" ]`, and
  `--cap-add=NET_ADMIN --cap-add=NET_RAW` sit beside it for the reason in the module's header
  (without them HA logs two ERRORs on every start asking for exactly those two caps; neither box
  has a Bluetooth adapter, and `--privileged` is not what they need, because `--privileged` is
  also what would have opened `/dev`).
  Consequence stated plainly: HA answers `:8123` on every address services1 has, on a box with
  `networking.firewall.enable = false`. Same surface `apps1` had, new audience.
- **image pinned to `2026.11.0`** — that is the core the box was already running (`.HA_VERSION`
  read `2026.10.4` on 2026-11-02 and `2026.11.0` on 2026-11-16, both under the unpinned
  `:stable`), so the move carries no version change. The tag's existence was checked against
  the registry manifest (HTTP 200) before the line was written, because the open follow-up
  already says what a bad pin costs. It also means the nightly `update-containers.timer`
  (`machines/services1/containers.nix`, which re-pulls each running image and restarts the unit
  when the digest moved) now leaves HA alone: **upgrading HA is an edit to `HA_IMAGE` plus a
  switch**, and `docs/follow-ups.md`'s unpinned-tag item closes with it.
- **no `--privileged`, no `--device`** — the section above. The first start after the switch
  proved the decision correct rather than merely argued: `Loaded zha` appears in the log, ZHA
  never reached a device, `podman-zigbee2mqtt` stayed `active` with zero port-error lines, and
  nothing but zigbee2mqtt holds `/dev/ttyUSB0`.
- **`TZ=Europe/London`, `/srv/homeassistant:/config`** — parity with the old unit. The
  activation script only `mkdir -p`s; it does not `chown -R` the tree on every activation the
  way `zigbee2mqtt.nix` does, because this tree is 556 MB.

HA's own `configuration.yaml` needed **no edit**: `use_x_forwarded_for: true` with
`trusted_proxies: [127.0.0.1, 10.3.1.0/24, …]` already trusts both the loopback proxy it is
about to gain and `apps1`'s address, and `cors_allowed_origins` are names, not IPs. That
 mattered: `use_x_forwarded_for` with a proxy address *not* in `trusted_proxies` is how HA
 starts logging every client as the proxy — or refusing the header outright.

## The copy, and the size that is not what it looks like

`/srv/homeassistant` is **837 MB**, of which:

```
403M  home-assistant_v2.db
278M  home-assistant_v2.db.corrupt.2024-06-06T01:27:25…   ← excluded
 71M  home-assistant.log                                   ← included, it is only history
 43M  backups
 26M  home-assistant.log.1
5.5M  .storage          ← the registries and every config entry: the actual state
4.2M  home-assistant_v2.db-wal
```

So the copy excludes `*.corrupt.*` and lands **556 MB**, verified with a dry-run pass reporting
zero remaining differences. It was taken while HA was still **running**, which is exactly why it
is a *warm* copy and not the copy: SQLite plus a live WAL is not a consistent snapshot, and HA
rewrites `.storage/*.json` on shutdown. The real copy is the delta taken after the old HA stops,
which is why the window is seconds and not minutes.

Run from services1 as root, because `/srv` is root's:

```bash
sudo rsync -aHAX --partial --exclude='*.corrupt.*' \
  -e 'ssh -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=yes \
      -o UserKnownHostsFile=/home/leigh-admin/.ssh/known_hosts \
      -i /home/leigh-admin/.ssh/agent-hop-key' \
  root@10.3.1.30:/srv/homeassistant/ /srv/homeassistant/
```

Two ssh facts that cost a failed attempt and are worth knowing about `apps1`:

1. **`IdentitiesOnly=yes` is required for root@apps1.** Without it:
   `Too many authentication failures` — rule 32's system `programs.ssh.hosts` block is generated
   from `common/hosts.nix`, `apps1` is *deliberately* not in that table (`slack-agent.nix`'s
   comment says why), so `ssh` offers every key it can find and the server gives up first.
2. **root on services1 has no identity and no host key for `apps1`.** The hop key is at
   `~/.ssh/agent-hop-key` and `~` for root is `/root` (rule 32 again), and
   `/etc/ssh/ssh_known_hosts` contains **zero** lines for `10.3.1.30` — its host key exists only
   in `/home/leigh-admin/.ssh/known_hosts`. Hence the explicit `-i`/`-o UserKnownHostsFile`.
   This is the same open item as the hand-written `/root/.ssh/config` stanzas in
   `docs/follow-ups.md`, seen from a third side.

## The legacy path: `10.3.1.30:8123` keeps answering, on purpose

The requirement is that whatever is still dialling the old address keeps working *and* announces
itself. `apps1` keeps `10.3.1.30` (above), so the forwarder lives there, and the measurement
happens there too.

**Why not the other three shapes, each rejected for a measured reason:**

- *DNAT on `gw`* — clients on VLAN 225 ARP for `10.3.1.30` and never traverse the router, so the
  door would only ever see cross-VLAN traffic and would report "nobody" while a HomePod on the
  LAN hammered it. Ruled out by topology.
- *DNAT + SNAT on `apps1`* — works, but `apps1` runs docker, whose `FORWARD` chain policy is
  `DROP` with `DOCKER-USER`/`DOCKER-FORWARD` above it, so a forwarded-to-another-box rule means
  hand-editing docker-managed netfilter on a box that rebuilds it at every daemon start.
- *nginx `stream` on `apps1`* — `nginx -V` there reports **no `--with-stream`**, and nginx is
  otherwise busy serving that box's own vhosts.
- **`socat`, chosen:** already installed at `/usr/bin/socat`, one unit, no firewall edits, and it
  logs the peer address of every accepted connection to the journal. Proven before being
  recommended, on a scratch port against the live HA (`-x` omitted once it was clear what the
  lines look like):

  ```bash
  systemd-run --unit=socat-proof /usr/bin/socat -d -d TCP-LISTEN:18124,fork,reuseaddr TCP:127.0.0.1:8123
  curl -s -o /dev/null -w '%{http_code}\n' http://10.3.1.30:18124/api/     # 401 = reached HA
  curl -s http://10.3.1.30:18124/ | head -c 60                             # the real SPA index
  journalctl -u socat-test -o short-iso | grep accepting
  # … N accepting connection from AF=2 10.3.1.20:35254 on AF=2 10.3.1.30:18124
  ```

  That `accepting connection from` line is the accounting: one per TCP connection, with the
  source address, in the journal, with the timestamp.

**The unit, parked here in the off-flake form of rule 23** — `/etc/systemd/system/ha-legacy-proxy.service`
on `apps1`. `ExecStart` must **not** pull or build anything; this box's job now is to be boring:

```ini
# LEGACY PROXY — 2026-11-16 — Home Assistant moved to services1 (10.3.1.20:8123); this unit
# exists only to keep 10.3.1.30:8123 answering and to log who still dials it.
# Remove with: systemctl disable --now ha-legacy-proxy.service && rm /etc/systemd/system/ha-legacy-proxy.service
# Retirement criterion: `just ha-legacy-watch 30` shows no source other than 127.0.0.1 and
# 10.3.1.20 for 30 consecutive days (docs/home-assistant-on-services1-2026-11-16.md).
[Unit]
Description=legacy 10.3.1.30:8123 -> 10.3.1.20:8123 forwarder (Home Assistant moved to services1)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/bin/socat -d -d TCP-LISTEN:8123,fork,reuseaddr TCP:10.3.1.20:8123
Restart=always
RestartSec=2
StartLimitIntervalSec=0

[Install]
WantedBy=default.target
```

Ordering is load-bearing: **the old HA must be stopped before this starts** (they want the same
port), and the forwarder must be up before anyone can notice the app is gone.

### What the log will look like, and what each class means

Four classes, and getting them straight is the whole point of the exercise. This is what the
first hour after the switch actually reported — see "What the forwarder found", which is the
half of this exercise that paid for itself:

- `10.3.1.20` — **this box**: services1's nginx origin vhost and/or `hackspace-mcp`. Since the
  repoint landed, *nothing on services1 should dial `.30` any more*, so these lines mean the
  repoint did not take, or an off-flake something on services1 is going the long way round. In
  practice the first two lines here were **my own `curl` probes** at the old address during
  verification, which is the one way this signal produces a false positive: run the verb before
  you probe by hand, or read the timestamps.
- `127.0.0.1` / `127.0.1.1` — **`apps1` itself**, loopback. This *was* its `hass` Prometheus job
  (which resolved `apps1.int.leighhack.org` to `127.0.1.1` via that box's own `/etc/hosts` and so
  scraped HA over loopback); that target was repointed to `10.3.1.20:8123` on 2026-11-16, so a
  loopback line now means something else local to `apps1` has started dialling HA.
- `172.17.0.x` — **a container on `apps1`'s own docker bridge**, and the class nobody predicted:
  these are not LAN devices, so they will never have a lease and never appear in
  `common/dns-records.nix`. `docker network inspect bridge` names them (the addresses are
  assigned by docker and are not stable across container restarts, so map them each time).
- **anything else** — a real client that hardcoded the old address. That is the population the
  exercise exists to find: companion apps configured with an IP, dashboards, scripts, and above
  all anything speaking to HA's `/api/websocket` or a webhook.

`just ha-legacy-watch [days]` does the bucketing and names the unknowns by asking the router
what it calls that address.

### The measurement that preceded all of this

Before anything moved, a SYN-only capture was started on `apps1` so the *baseline* — who dials
the old address while the app still lives there — is recorded rather than guessed:

```bash
setsid bash -c 'timeout 7d tcpdump -i any -nn -l \
  "tcp port 8123 and (tcp[tcpflags] & (tcp-syn|tcp-ack) == tcp-syn)" \
  >> /root/ha8123-syn.log 2>&1 </dev/null &'
```

First two minutes: **one** source, `10.3.1.20` — services1 itself. That is consistent with the
client map and is not yet an answer: daily and weekly clients (a phone's companion app on Wi-Fi
roaming, a scheduled script, a nightly backup) need days to appear, which is what the 7-day
window is for. Read it with `sort | uniq -c` on the third field, and read it *before* judging
`just ha-legacy-watch` output as clean. (What 29 minutes of it turned out to contain, including
the client the in-tree client map could not have predicted, is the table in "What the forwarder
found".)

## The window

Downtime is steps 2–4. Everything before it is preparatory and everything after it is
reversible without touching services1.

1. **Pre-flight** (no outage): `just check` green; `sudo podman pull
   ghcr.io/home-assistant/home-assistant:2026.11.0` on services1 so the switch does not wait on
   a registry; warm copy already done and re-diffed.
2. **Stop the old HA** — `apps1`: `systemctl stop home-assistant.service`. This is also the moment
   HA flushes `.storage/*.json`, which is what the delta copy below will pick up.
3. **Final delta** — the same `rsync` as above, no excludes beyond `*.corrupt.*`. Seconds.
4. **Switch services1 and confirm in the same invocation** (GOLDEN RULE):
   `cd ~/Projects/infrastructure-nix-flake && sudo nixos-rebuild switch --flake .#services1 > /tmp/ha-switch.log 2>&1; rc=$?; [ $rc -eq 0 ] && sudo nixos-confirm`
   — that activation starts `podman-home-assistant`, repoints the vhost, and restarts
   `hackspace-mcp`/`slack-agent` with the loopback `--hass-url`. Do **not** pipe the rebuild into
   `tail` — the pipeline lies to the `&&`.
5. **Confirm the legacy proxy**: on `apps1`, install the unit above,
   `systemctl daemon-reload && systemctl enable --now ha-legacy-proxy.service`.
6. **Read back, don't assume** — see the checklist.

### Checklist after the switch

- `readlink -f /run/current-system /nix/var/nix/profiles/system-good` — equal, and equal to the
  new generation.
- `systemctl is-active podman-home-assistant`, and `podman logs --tail 40 home-assistant` for
  `Starting Home Assistant 2026.11.0` and **zero** `Failed to connect to MQTT server`.
- `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8123/api/` → `401`; same through
  `https://ha.int.leighhack.org` → `200` (the door's own path).
- **The eight Frigate camera entities are `recording`, not `unavailable`** — the symptom
  `docs/mqtt-int-legacy-answer-2026-11-02.md` exists to catch, and now the only way to catch it.
  HA's own history is the read: `sqlite3 -readonly /srv/homeassistant/home-assistant_v2.db` with
  the `states`⋈`states_meta` join in that doc §"If it recurs" (no `entities` table;
  `coalesce(last_reported_ts, …)` because most rows have NULL there).
- `sudo journalctl -u podman-mqtt.service --since -15m | grep -E "New client connected|disconnected"`
  — a `New client connected` from services1's own address for the Home Assistant client, and
  `frigate` and `mqttjs_*` (zigbee2mqtt) still present, all three of them.
- zigbee2mqtt **unaffected**: `systemctl is-active podman-zigbee2mqtt` still active, and the
  journal shows **no** `device busy`/`Resource temporarily unavailable` on `/dev/ttyUSB0` — that
  is the ZHA-collision failure mode, and this is the check that proves it did not happen.
- `matter`/`thread`/`apple_tv`/`esphome` reconnected: HA logs each domain's setup; a `matter`
  server that cannot reach its accessories shows up there.
- gatus's "Home Assistant" endpoint green; `hackspace-mcp` answering
  (`just slack-agent-call home_assistant_states '{"limit":1}'`).
- The legacy proxy: `curl -s -o /dev/null -w '%{http_code}' http://10.3.1.30:8123/api/` → `401`,
  and `journalctl -u ha-legacy-proxy -o short-iso | grep accepting` naming services1 — i.e. the
  old address demonstrably reaches the new app.

## Rollback

The old box keeps everything: `home-assistant.service` is **stopped and disabled, not removed**,
the image stays in docker's store, and `/srv/homeassistant` on `apps1` is untouched (it is the
source of the copy, never the destination). So rollback is:

```bash
ssh -i ~/.ssh/agent-hop-key root@10.3.1.30 \
  'systemctl stop ha-legacy-proxy.service && systemctl start home-assistant.service'
```

plus reverting the three flake edits (`http.nix`, `slack-agent.nix`, the new module) and
re-switching services1. The two sides must not be up at once: the forwarder and the old
container both want `:8123` on `apps1`, so the proxy stopping is part of the rollback rather
than a separate decision — and if both were somehow up, two HAs would share one MQTT broker and
one set of `frigate/#` topics.

## Two things the plan got wrong

Recorded because both were invisible to `just check` and both would have been "the config is
fine, the service is just down" for whoever came next.

1. **`networkMode` is not an option in this nixpkgs' `oci-containers`.** Writing the module with
   `networkMode = "host"` produced a *valid-looking file* and no container: the failure surfaced
   only when something actually evaluated services1, as
   `error: The option 'virtualisation.oci-containers.containers.home-assistant.networkMode' does
   not exist. Did you mean '…networks', '…workdir' or '…autoStart'?`. The flag form
   (`extraOptions = [ "--network=host" ]`) is what ships.
2. **A file dropped into `machines/services1/services/` is not a service.** That directory is
   imported by `machines/services1/services/default.nix`, whose `imports` is an **explicit list**
   — so a new `home-assistant.nix` sitting beside thirty-odd imported files is *never evaluated*,
   prints nothing, breaks nothing, and leaves `just check` completely green. The only thing that
   caught it was the rule-10 discipline of reading the rendered unit out of a real build:
   `nix build .#nixosConfigurations.services1.config.system.build.toplevel` and then looking for
   `etc/systemd/system/podman-home-assistant.service`, which was **not there**. This is rule 23
   seen from the other side — a parked module is invisible to `grep -rl '^# PARKED'` because it
   is never evaluated, and an *unimported new* module is invisible to everything, because nothing
   in the tree is obliged to mention a file that does not yet exist. The lesson generalises
   past this box: `ls`-ing a `services/` directory is not evidence of what a machine runs.

Both are written up where they will be hit again: the import-list trap in `docs/services.md`
("Adding a service module…"), and this file for the rest.

## What the forwarder found

Within one hour of the switch, the accounting had already found traffic this repo's client map
could not have predicted — which is the entire argument for leaving the forwarder on:

| Source | Before the move (SYN capture, ~29 min) | What it is |
| --- | --- | --- |
| `172.17.0.6` | **194**, all of them in 15:41–15:45 | `hackspace-api` (`ghcr.io/leigh-hackspace/hackspace-api:1.3.2`) on `apps1`, dialling `HOMEASSISTANT_INSTANCE=http://apps1.int.leighhack.org:8123` from `/etc/docker/environment_files/hackspaceapi`. **Read that number correctly, because it is not a poll rate:** in the 29 minutes *before* the move it dialled **zero** times, and all 194 land in the four minutes after `systemctl stop home-assistant.service` — it is a reconnect storm against a refused port (~48/min, which is `48 15:41 / 48 15:42 / 2 15:43 / 48 15:44 / 48 15:45`). It normally holds one long-lived connection, the forwarder started at 15:45:31, and its first successful connect after the outage was 15:45:31. Left on this path at the time, and **moved off it the same afternoon** — see "Then the Space API moved too". |
| `10.3.1.20` | 43 | services1's own nginx origin vhost, pre-repoint. Correct — that was the thing being moved. |
| `172.17.0.7` | 12 | `docker.io/prom/prometheus:v2.48.0`, the container behind `prometheus.service`, whose `hass` job scraped `apps1.int.leighhack.org:8123`. **Repointed to `10.3.1.20:8123` the same afternoon** (`/srv/prometheus/prometheus.yml`, timestamped `.bak` beside it): zero user-visible surface, and it removes the largest steady source of legacy traffic. `up{job="hass"} == 1` on the new target, so HA's `/api/prometheus` and the long-lived token inside the copied database both crossed the move intact. |

**Zero unknown clients.** No LAN device, phone, dashboard or script had hardcoded `10.3.1.30:8123`
in the observation window — every source was `apps1`'s own containers or this box. That is the
answer to the question the whole exercise was built to ask, for as long as the capture has run
so far; the 30-day criterion is what turns it into a decision.

### Counting things through a proxy is not the same as counting SYNs

The one genuinely surprising measurement error in this pass, and worth internalising before
reading any `just ha-legacy-watch` output:

- **The SYN capture measures connection attempts, not use.** `hackspace-api` looked like the
  loudest client on the old address (194) and was in fact the *quietest*: it opens one long-lived
  connection and keeps it. Every one of those 194 was a retry after its connection was killed.
  A busy client and a client stuck in a reconnect loop are the same shape in a SYN log.
- **Through a forwarder, "how many things" is answered by distinct sources plus live sockets**,
  never by a connection count either: one `accepting connection` line can be everything a client
  ever asks (`curl -s http://10.3.1.30:8081/space.json` and HA agree because 280 KB crossed that
  one socket once, which is roughly one `/api/states` dump of 431 entities). The read that
  distinguishes "live" from "warm and idle" is the socket's own accounting:

  ```bash
  ss -itn state established '( sport = :8123 )'   # bytes_sent/received, lastrcv — sample it twice
  ````

  Sample it twice ~60 s apart: if `lastrcv` ages and the byte counters do not move, the
  connection is pooled and idle, not streaming.
- **What proves a client is genuinely served is its own output, end to end.** All seven
  temperatures `hackspace-api` publishes are Home Assistant's current values to the decimal —
  `Rack 1 15.6 / Pi Room 15.56 / Fabrication 14.2 / Workshop 13.0 / Main Space 15.1 / Dark Room
  15.0 / Craft Space 13.7` against `sensor.rack_sensor_temperature = 15.6` and its siblings —
  read at the same minute. (Which also catches the trap in the previous paragraph: HA has *two*
  entities titled "Rack 1 Temperature", and the bluetooth-proxy one is `unavailable` because
  `10.3.2.14` is down. The Space API reads the live one.)

So the answer to "what is still on the proxy" was **one client**, `hackspace-api` — and then, the
same afternoon, that client moved too, which is the next section. Nothing has dialled
`10.3.1.30:8123` since.

## Then the Space API moved too (same afternoon, 16:15–16:30)

The forwarder existed for exactly one remaining client, so the cleanest way to end the whole
exercise was to move that client rather than wait 30 days for it to be reported idle. It is now
`machines/services1/services/hackspace-api.nix`, and `apps1` runs neither Home Assistant nor the
Space API.

| Time (UTC) | |
| --- | --- |
| 16:15 | `nix build` of services1's toplevel, then **the rendered unit read back**: `-p 8081:80`, `-v /srv/hackspaceapi:/data`, `--env-file=/var/lib/hackspace-api/env`, pinned image, `--pull missing`, and the root `ExecStartPre=+ … test -s …` gate. Gate over the closure: **0 files contain the token name** |
| 16:1x | state copied host-to-host — `sensors.yaml` (1,694 bytes) and the env file redirected straight into `/var/lib/hackspace-api/env` at `0600`, never through a terminal; key-set hash identical to the source (7 keys), `HOMEASSISTANT_TOKEN` line md5 identical, two values rewritten by `sed` |
| 16:2x | `just switch` + confirm: `podman-hackspace-api` active. **`space.json` byte-identical to `apps1`'s instance** (3,439 bytes, one md5) while both ran — which is the proof that the *Prometheus* half crossed too, since `total_member_count` and `ext_3d_printers` come from `PROMETHEUS_INSTANCE` |
| 16:2x | `git push` then `just deploy gw` (the recipe fetches the commit on the box, so the row had to be committed first): switch + confirm on the router, `front-door-cert-check.service` → `Result=success`, nginx/dnsmasq/wan-manager/network-status all active, `/api/leases` `total=30 expired=0 stale=null` |
| 16:2x | `https://api.leighhack.org/space.json`, `https://api.int.leighhack.org/space.json` and `http://10.3.1.20:8081/space.json` all answer **the same document**; `/health` `{"health":"ok","version":"1.3.2"}` |
| 16:2x | `apps1`: `systemctl stop` + `disable hackspace-api`, zero listeners on `:8081`, unit text parked as `hackspace-api.service.2026-11-16` in `/var/lib/systemd/disabled-units/` with the restore line in its header, unit file and `/srv/hackspaceapi` left in place as the rollback |
| 16:26 | `apps1`'s Prometheus repointed and restarted: `hackspaceapi → http://10.3.1.20:8081/metrics` **health=up**, alongside `hass → 10.3.1.20:8123/api/prometheus` **health=up** |
| 16:3x | **`just ha-legacy-watch`: no source, no established connections.** The legacy address has been silent since the last probe of it at 15:57 |

What the bot does with it, checked through the real path after the repoint
(`--space-url http://127.0.0.1:8081`, so this exercises the new container and nothing on
`apps1`): `space_status` returns open/closed, all seven temperatures and humidities, both 3D
printers and `39 Active Members`; `space_events` returns the calendar. So the deterministic
answer and the Monday digest's renderer are both on-box now.

### Three findings from this half, none of them about the move

- **The scrape config for this job was not in the file its siblings are in.** `apps1`'s
  `prometheus.yml` has `scrape_configs: - /prometheus/scrape_configs/*.yml`, and
  `hackspaceapi.yml` lives in that directory. Editing the main file for this job would have been
  a silent no-op — the same class as the unimported module earlier in this document: **find the
  file the loader actually reads, not the one that looks like the config.**
- **`/srv/prometheus/scrape_configs/uptimekuma.yml` holds a plaintext `admin` password** for
  Uptime Kuma — the application that `docs/monster-proxmox.md` records as stopped, disabled and
  **masked** since 2026-11-01, whose DNS name no longer resolves. So the credential is useless
  against its own service and readable by whoever can read that directory, and it is exactly the
  kind of password that outlives the app by being reused. Not touched here; opened in
  `docs/follow-ups.md`.
- `space_status` renders the printer line as `3D-1 offline (41m ago ago)`, and
  `state.lastchange` still carries **15:45 UTC — Home Assistant's restart during this document's
  maintenance**, not a real open/close. Both are cosmetic; the second is the one to mention in
  any future HA window, because the public API shows it.

### …and that one client is the hackspace's public machine-readable presence

Measuring what `hackspace-api` is turned the "leave it on the forwarder, it is a quiet internal
consumer" framing inside out. Read the door's own log (`/var/log/nginx/access.log` on `gw`,
40 h: 2026-11-15 00:00 → 11-16 16:09, 319,066 requests, 565 sources, 228,508 of them off-LAN):

| Endpoint | Hits | Who |
| --- | --- | --- |
| `/space.json` | **8,365** — **7,939 from the public internet** | self-identifying aggregators: `spacer-fetch` (4,817, ≈ every 30 s), `Go-http-client` (2,246), `MapsOfMaking-heartbeat/1.0`, `http://mapall.space`, `SpacePush/0.3`, `HackerspacesGlobe/1.0`, `HackerspaceNewsFeed/0.1`, `MapsOfMaking` — i.e. the global hackspace/makerspace maps and status aggregators all poll us |
| `/events.ics` | **448**, again mostly external | members' calendar clients subscribing to the space calendar |
| `/events?calendar=…` | 3 | the internal callers (the bot) usually go straight to `10.3.1.30:8081` and do not cross the door |

What it is, precisely: a small Python (uvicorn) service implementing the community **SpaceAPI**
spec (`api: "0.13"`), configured by `SENSOR_CONFIG_FILE=/srv/hackspaceapi/sensors.yaml` — 62 lines
mapping Home Assistant entity ids to named locations — plus `HOMEASSISTANT_INSTANCE`/`HOMEASSISTANT_TOKEN`,
`PROMETHEUS_INSTANCE`, and `HACKSPACE_OPEN_ENTITY=input_boolean.hackspace_open`. It also serves
`/events` (the calendar proxy `hackspace-mcp` uses) and `/events.ics`. So it is a thin
aggregation layer: HA for sensors and open/closed, Prometheus for the rest, static YAML for the
identity fields — but with a **standard public schema and a stable URL**, which is exactly what
the pollers above require.

Two operational facts that follow, neither of which was known before the move:

- **A Home Assistant restart writes into the public API.** `state.open` comes from
  `input_boolean.hackspace_open` in HA, and `state.lastchange` after the move read
  `1794843913` = **15:45:13Z** — the minute HA came back. So an HA maintenance window moves the
  public "last changed" timestamp even when the space never opened or closed. Harmless, but say
  it out loud in any future window rather than explaining it to whoever reads the API afterwards.
- **It is load-bearing internally too, not just externally.** `hackspace-mcp` builds
  `{space_url}/events?calendar=…` for `space_events` — the deterministic "what's on" answer *and*
  the renderer the Monday 09:00 `slack-agent-digest.timer` posts — and `space_status`, temps,
  humidity, members and printers all come from `space.json`. Deleting it is a Rust change in
  `tools_space.rs`, not a config edit. (`docs/slack-agent-2026-11-16.md` already called this one
  endpoint "most of Vulcan"; the access log agrees.)

Therefore the forwarder's retirement is gated on **repointing one env line in
`/etc/docker/environment_files/hackspaceapi`**, never on deleting anything — and since restarting
that container drops a service the outside world asks about ~200 times an hour (seven or eight
named pollers, plus calendars), do it at a quiet hour and
watch `/space.json` come back to 200, rather than treating it as a config-only change. **Done
2026-11-16 at a quiet hour — but not by restart:** the service was rebuilt on services1 and run
*alongside* `apps1`'s until the door row and every consumer pointed at it, so the public endpoint
never dropped a request. Next section.

One self-inflicted footgun worth knowing before believing a report: the `10.3.1.20` rows in the
*after* numbers were **my own `curl` probes of the old address**, typed to prove the forwarder
worked. Probe first, read second, or the report accuses this box of the thing you were testing
for.

## Deployed, 2026-11-16, with the receipts

Built first, then switched, so the window was activation-only (`nix build` of the toplevel: 17 s,
all cached — and that build is the pass that found the unimported-module fault above).

| Time (UTC) | |
| --- | --- |
| 15:41:04 | `systemctl stop home-assistant.service` on `apps1` → `inactive`, zero listeners on `:8123` there |
| 15:41:04–18 | final `rsync` delta: **1.9 s**; `home-assistant_v2.db` mtime `15:41:10` = HA's own shutdown checkpoint, so the copy is consistent rather than caught mid-WAL |
| 15:41:4x | `nixos-rebuild switch --flake .#services1` → `rc=0`, `sudo nixos-confirm` chained in the same invocation |
| 15:41:51 | first `401` from `http://127.0.0.1:8123/api/` on services1 |
| 15:42:18 | `Starting Home Assistant 2026.11.0` — the pinned core, running |
| 15:44:53–15:45:15 | second switch (the two caps), then HA's MQTT client reconnected from `10.3.1.20` |
| 15:46 | `ha-legacy-proxy.service` active+enabled on `apps1`, `home-assistant.service` inactive+disabled, `home-assistant.service.2026-11-16` parked in `/var/lib/systemd/disabled-units/` with the restore line in its header, `socat` holding `:8123` |

Gates read afterwards, each one against the failure it exists to catch:

- **The objective outage measure: one failed check.** Gatus polls
  `https://ha.int.leighhack.org` every minute and retains 50 results; in the whole retained
  window there is **exactly one non-200**, a `502` at `15:41:40Z` — the minute the door was
  already proxying to services1 while HA was still booting. Every check before and after is 200.
  So the user-visible downtime was **one check interval**, not the wall-clock span between
  stopping and starting.

- `readlink -f /run/current-system /nix/var/nix/profiles/system-good` → both
  `…-nixos-system-services1-26.11.20261112.2074cc6`, equal, and equal to the generation whose
  rendered unit had been read back before the switch.
- **`0` ERROR lines** in the running container's log (after the caps; the two capability ERRORs
  are gone, which is what `--cap-add=NET_ADMIN --cap-add=NET_RAW` was bought for).
- **All eight Frigate camera entities `recording`** — `cam8`, `cnc`, `main_space`, `pi_room`,
  `rack_corridor`, `social_space`, `woodwork`, `workshop`, read out of HA's own `states`⋈
  `states_meta` history. Not one `unavailable` among them. That is the
  `docs/mqtt-int-legacy-answer-2026-11-02.md` symptom, and it is clear.
- MQTT broker journal: `Client … [10.3.1.30] disconnected: connection closed by client` at
  15:41:12 (the old box leaving cleanly), then `New client connected from 10.3.1.20` — Home
  Assistant now arrives from this box, and `frigate`/`mqttjs_*` never dropped.
- **Entity count identical across the move: 431 before, 431 after** (`just slack-agent-call
  home_assistant_states '{"limit":1}'`, whose tool dials HA through `hackspace-mcp` — so it
  checks the repointed `--hass-url` at the same time as it checks that nothing was lost).
- `https://ha.int.leighhack.org` → `200` in 33 ms through the door; `http://10.3.1.30:8123/`
  → the real SPA index **through the forwarder**.
- Zigbee: `podman-zigbee2mqtt` `active`, **0** `busy`/`Resource temporarily unavailable` lines, and
  the `zha` log shows `Loaded zha` with no device ever opened — the SLZB-06M never left
  zigbee2mqtt's hands.
- `curl http://10.3.1.30:631/printers/` from services1 → `200`, so HA's `ipp` "Big Laser Printer"
  entry keeps working cross-host, and `Allow @LOCAL` was the right read rather than the hope.
- `hackspace-mcp`, `slack-agent`, `podman-frigate`, `podman-zigbee2mqtt`, `podman-mqtt` all
  `active`; `door-entry-management-system-backend`/`-frontend` both `active` (it dials
  `wss://ha.int…`, so it needed no change).

### Three things that are broken and were not broken by this

Each one is dated to before the move, so nobody spends an evening re-litigating it:

- `camera.3d_1_camera`, `camera.3d_3_camera_2`, `camera.octoprint_camera_3` and the
  `sensor.octoprint_*` family are `unavailable`, and have been since **2026-11-07 13:18:48** —
  nine days before the move, and they only ever change state when HA restarts. Cause: their
  OctoPrint config entries name `3d-2.int.leighhack.org` and `3d-3.int.leighhack.org`, and
  **neither resolves from services1** (`getent hosts` empty; `3d-1` is configured as the literal
  `10.3.14.11`, which answers). The printers this network actually has are `3d-blue`
  `10.3.14.62` and `3d-lime` `10.3.14.61` (`docs/printer-servers.md`), so those entries are
  stale config from an older printer generation, live in HA's UI-managed state, and fixing them
  is an HA-UI job rather than a flake edit.
- The Brother `MFC-L3740CDW` at `10.3.1.193` (`brother` + one `ipp` entry): the device answers
  **no ping and no HTTP** from services1 right now — it is off or gone — and the old box's log
  carries 209 of the same connection failures before the move.
- The `esphome` **`bt-proxy` at `10.3.2.14:6053`** is refused (port closed), and `sensor.leighhack_do_not_disturb_until`
  (the `slack` integration) logged a DNS timeout reaching `slack.com` — 7,966 identical lines in
  `apps1`'s pre-move log, last on 2026-11-06. Worth knowing that from services1 the *same* query
  succeeds (`resolve slack.com` → answers, `https://slack.com/api/api.test` → 200, container
  included), so the sensor should recover on its own; the two devices are genuinely down.

