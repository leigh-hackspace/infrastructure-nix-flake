# Home Assistant's Frigate cameras: core 2026.10.4 makes the deprecated `via_device` a fatal error, and `apps1` pulls `:stable` unpinned

Written 2026-11-02 from a session on services1, reading Home Assistant on **`apps1`**
(10.3.1.30, Proxmox VM 109 — `docs/monster-proxmox.md` owns that box, this file owns this
finding). The question that started it: *"why did the Frigate integration stop working in
Home Assistant at the end of October?"* Nothing in this repo caused it, and nothing in this
repo can fix it — `apps1` is off-flake and none of it is declarative.

## What was broken

The **Frigate camera entities** were never created, since 2026-10-31. Everything else the
integration owns kept working — sensors, counters, occupancy, recordings switches,
notifications — so the symptom reads as "the cameras went quiet" rather than "the
integration died", which is why it was not obvious at the time.

The log line is the whole story:

```
2026-10-31 11:54:45.189 ERROR (MainThread) [homeassistant.components.camera] Error adding entity None for domain camera with platform frigate
```

`Error adding entity None` — the entity has no id yet, so it died *during* creation.

## The receipts, read off the box

Two startup failures, one per startup, six entities each — and **nothing else in
journald**, which reaches back to 2026-10-15:

```bash
journalctl -u home-assistant.service --no-pager -o short-iso \
  | grep -c "Error adding entity None for domain camera with platform frigate"   # 12
journalctl -u home-assistant.service --no-pager -o short-iso \
  | grep "Error adding entity None for domain camera with platform frigate" \
  | awk '{print $1}' | sort -u          # 2026-10-31T11:54:45  2026-10-31T13:24:43
```

The full traceback of one failure, from `home-assistant.log.1` (the Oct-31 morning run):

```
File ".../homeassistant/helpers/entity_platform.py", line 725,  in _async_add_entities
File ".../homeassistant/helpers/entity_platform.py", line 872,  in _async_add_entity
File ".../homeassistant/helpers/entity_platform.py", line 1040, in _async_add_entity_impl
    device = dev_reg.async_get_or_create(config_entry_id=self.config_entry.entry_id, ...)
File ".../homeassistant/helpers/device_registry.py", line 2395, in async_get_or_create
    report_usage("calls `device_registry.async_get_or_create` with a deprecated "
                 "`via_device` parameter; use `via_device_id` instead", ...,
                 breaks_in_ha_version=version)
File ".../homeassistant/helpers/frame.py", line 190, in report_usage
    integration_frame = get_integration_frame(exclude_integrations=exclude_integrations)
File ".../homeassistant/helpers/frame.py", line 128, in get_integration_frame
    raise MissingIntegrationFrame
File ".../homeassistant/helpers/frame.py", line 386, in _report_usage_no_integration
    raise RuntimeError(msg) from err
RuntimeError: Detected code that calls `device_registry.async_get_or_create` with a deprecated `via_device` parameter; use `via_device_id` instead.
```

Read against the installed core (`docker exec home-assistant …` on the running
container, whose HA lives at `/usr/src/homeassistant/homeassistant`, **not**
`/usr/src/homeassistant/homeassistant/camera/…` as the traceback's path decoration
claims — that is the other half of the surprise in reading these tracebacks):

- `helpers/device_registry.py:276` — the deprecation is now table-driven:
  `"via_device": ("2027.8.0", "via_device_id")`.
- `helpers/device_registry.py:2395` — the report for it is
  `core_behavior=ReportBehavior.ERROR, core_integration_behavior=ReportBehavior.ERROR`.
- `helpers/entity_platform.py:1040` — core **splats the entity's `DeviceInfo`** into
  `async_get_or_create(**cast("dr.DeviceInfo", device_info))`. So the offending
  `via_device=` is not typed by the integration at the call site; it is a *key in the
  DeviceInfo dict* the integration hands HA, and core re-passes it.
- `helpers/frame.py:386` — because the call is core code, `get_integration_frame()`
  walks the stack and finds **no integration frame**: the entity add was run by
  `helpers/event_worker/__init__.py`'s task, and the integration's own coroutine frames
  had already returned by then. Unattributable code gets `core_behavior`, which is
  `ERROR`, so it is a `RuntimeError`, not the warning the same integration gets elsewhere
  in the same run (`… calls `async_add_entities` … This will stop working in Home
  Assistant 2027.8.0 …` — attributed, `LOG`, harmless).

So: **a Frigate entity whose `DeviceInfo` carries the old `via_device` key cannot be
created at all on this core.** `_async_add_entities` catches the exception per entity,
logs that one line, and moves on — which is why the failure was a *quiet* missing
camera rather than a crashed integration.

## The trigger: an unpinned container tag

`/etc/systemd/system/home-assistant.service` on `apps1`:

```ini
ExecStartPre=/usr/bin/docker pull ghcr.io/home-assistant/home-assistant:stable
ExecStart=/usr/bin/docker run --rm -v /srv/homeassistant:/config --privileged -e TZ=Europe/London --network=host --name=home-assistant ghcr.io/home-assistant/home-assistant:stable
Restart=always
```

`:stable` is **not a version**. Any start of that unit takes whatever ghcr currently
holds, so a core minor bump can land on the box without anyone touching the box. The
unit was started twice on 2026-10-31 — `systemd[1]: Starting home-assistant…` at
**11:48:53** and **13:22:20** — and both runs logged:

```bash
journalctl -u home-assistant.service --no-pager -o short-iso | grep -o "Starting Home Assistant [0-9.]*"
# 2026-10-31T11:54:45 … Starting Home Assistant 2026.10.4
# 2026-10-31T13:24:43 … Starting Home Assistant 2026.10.4
```

Everything that was *running* through 2026-10-30 was **2026.10.0**, visible in the
User-Agent of every outbound request in mid-October
(`HomeAssistant/2026.10.0 aiohttp/3.14.4 Python/3.14`), and `/srv/homeassistant/.HA_VERSION`
carries the core version (`2026.10.4`, rewritten on each start). The component was
**5.15.4** in both runs (`Integration Version: 5.15.4`, logged at setup).

**Honest about the boundary:** the last core start *before* 2026-10-31 predates the journald
window (journal begins 2026-10-15 mid-run), and HA logs entity-creation failures only at
startup, so "2026.10.0 tolerated this and 2026.10.4 does not" is an **inference from the
version change coinciding exactly with the first occurrence**, not a measured A/B of two
startup logs. The failure side is measured directly; the "was fine" side is bounded by
what the box was running.

## The state that made it look stale rather than gone

`/srv/homeassistant/.storage/` holds HA's registries as JSON (this is what HA 2026.x uses
instead of the `entities` table in `home-assistant_v2.db`, which holds only the events
DB — the `no such table: entities` dead end that costs a detour if you go looking there):

- `core.entity_registry` — 223 `platform: frigate` entities, **7** of them `camera.*`
  (`rack_corridor`, `main_space`, `social_space`, `woodwork`, `workshop`, `pi_room`,
  `cnc`), **no `camera.cam8`**.
- `core.device_registry` — 8 frigate devices (one main, seven children linked by
  `via_device_id`), `modified_at` 2026-10-31.
- `/srv/frigate/config/config.yaml` (on services1, `machines/services1/services/frigate.nix`
  mounts it) had **8 enabled cameras** at that moment — the `3d_1`/`3d_2`/`3d_3`/`voron`
  blocks are commented out — so `cam8`'s missing entity is the new camera, and the seven
  others were the ones that had existed before. Six of the eight adds logged a failure;
  which two did not is not recoverable from the log, because the message prints no entity
  name.

**A file mtime is not a state readout here.** `core.entity_registry` was last written
2026-09-30 and `core.device_registry` 2026-10-31 13:27, and neither changed after the
repair at 2026-11-02 20:09 — HA flushes those files on change/exit, not on every
registration. The current state came from the journal instead
(`Registered new camera.frigate entity: camera.cam8`). Reading the registry files answers
"what was registered last time something was written"; reading the journal answers
"what this run did".

## The repair, and what the box said

Someone had already half-fixed it on 2026-11-02 at **18:37**: HACS reinstalled
`blakeblackshear/frigate-hass-integration` into `/srv/homeassistant/custom_components/frigate`
at **5.15.6** (`manifest.json` → `{"version": "5.15.6"}`). That version carries the shim:

```python
def get_frigate_via_device(hass, entry) -> DeviceInfo:
    identifier = get_frigate_device_identifier(entry)
    if "via_device_id" in DeviceInfo.__annotations__:
        ... = device_registry.async_get_device_by_identifier(identifier, entry.entry_id)
        device_info["via_device_id"] = device.id
        return device_info
    return {"via_device": identifier}
```

and 2026.10.4 has both halves (`DeviceInfo.via_device_id` at
`helpers/device_registry.py:151`, `async_get_device_by_identifier` at 2091), so it takes
the new branch. **It changed nothing at 18:37**, because HA had not restarted since
2026-10-31 13:22 — the running Python still holds 5.15.4's module objects, and HACS
swapping files on disk cannot swap an imported module. Replacing a HACS component is a
no-op until the process that imported it restarts; that is the whole reason the box
"looks fixed but is still broken".

The restart, in one ssh (2026-11-02 20:09, and `:stable` was still cached, so no further
drift landed):

```bash
ssh -i ~/.ssh/agent-hop-key root@10.3.1.30 \
  'systemctl restart home-assistant.service; systemctl show -p Result -p ActiveState --value home-assistant.service'
# restart rc=0  Result=success  ActiveState=active
```

Measured after it (`journalctl -u home-assistant.service --since "2026-11-02 20:09"`):

- core banner unchanged: `Starting Home Assistant 2026.10.4` — the pull took seconds
  (layers already local), so `:stable` had not moved since Oct 31 and this restart
  carried **no** version risk;
- `Setting up frigate` then `Integration Version: 5.15.6`, and all eight platforms
  (`frigate.sensor`, `frigate.camera`, `frigate.image`, `frigate.number`, …) set up;
- `Registered new camera.frigate entity: camera.cam8` — the previously missing camera,
  the other seven restored from the registry;
- **0** `Error adding entity`, and **0** `Detected …` frame reports of any kind for the
  whole run — 5.15.6 is clean against this core, so the deprecation traffic is gone, not
  just silenced.

## Not this

Two things that happened in the same window and are **not** the cause, so nobody chases
them again:

- **2026-10-31 18:31 → 19:58**, HA's `http://10.3.1.20:5000/api/stats` fetches failing
  (`fetch failed`, then `500 Internal Server Error` at 19:58:20–19:58:35) and
  `Fetching frigate data recovered` at 19:58:41. That is Frigate itself: `/srv/frigate/config/config.yaml`
  was rewritten at 19:49 (the thingino feed fix, `rtsp://…cam5…:554/stream=0` → `/stream0`,
  `docs/camera-thingino-migration-2026-10-31.md`) and the container restarted, so the
  sensors were blind for eight minutes. It healed itself.
- The `frigate`/`frigate-monitor` `*.int` rows added to `common/int-dns.nix` and
  `common/front-door.nix`, and `gw` becoming the front door on 2026-10-24
  (`docs/router-front-door-migration.md`). HA's frigate **config entry** still talks to
  `http://10.3.1.20:5000` directly (`.storage/core.config_entries`, `url = http://10.3.1.20:5000`,
  `modified_at` 2025-11-14), so no vhost or TLS move is on the path it takes.

## If it recurs

1. Read both versions **before anything else** — core from the journal banner /
   `.HA_VERSION`, component from `Integration Version` at setup (commands above). A
   minor-bumped `:stable` is the highest-probability cause on this box and the one with
   no commit to `git log` against it (which is `AGENTS.md` rule 20 seen from the far side:
   the box ran a newer generation than anything in this repo).
2. Read the traceback's last frames, not the first: HA's frame helper distinguishes
   *attributed* (`LOG`) from *unattributable* (`core_behavior`, usually `ERROR`), and an
   entity created in HA's event worker is unattributable. "The integration is old" and
   "the entity creation is core's" are the same bug from the two ends of that split.
3. A HACS re-install does not fix anything until HA restarts. Restart by the unit, and
   know that the unit re-pulls `:stable`.

Still open, and deliberately not done in this pass: **pinning the tag** in
`home-assistant.service`. Pinning is what stops this class of break, but a wrong pin makes
the unit fail and takes HA down for the day, so it belongs with a decision about *which*
tag (see `docs/follow-ups.md`, "Machines / access").
