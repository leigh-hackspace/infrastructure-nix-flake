# The space calendar — one event from a ticket page to the public feed

The runbook for **adding (or trying to change) an event on the space calendar**, and
the four things that make the obvious version of it wrong. Read it before you touch
either of the two files it names: the change is small, its blast radius is the whole
network's public face, and the failure modes show up far from where you typed.

This is **not a flake change**. Home Assistant's calendar state is runtime state the
flake only mounts (`machines/services1/services/home-assistant.nix` mounts
`/srv/homeassistant`; the flake never edits inside it), so there is nothing to switch,
nothing to `nixos-confirm`, and no generation that means "the calendar changed". The
evidence that counts is the rendered *feed*, one read per reader.

```
you ──► Home Assistant (services1, calendar.public_events)
              │        ▲
              ▼        └── HA parses /srv/homeassistant/.storage/local_calendar.<key>.ics ONCE at setup
   Space API :8081  (/events?calendar=public|members, /events.ics)
              │
              ├── api.leighhack.org / api.int.leighhack.org through the front door on gw
              ├── 448 calendar-client subscriptions to /events.ics (measured 2026-11-16)
              ├── hackspace-mcp's `space_events` — the deterministic "what's on", and
              │   the renderer the Monday 09:00 slack-agent-digest posts
              └── the HA dashboard (`lovelace.lovelace` shows public + members)
```

## What the three calendars are, and which one an event belongs to

| HA entity | `storage_key` → the file HA reads | Who reads it | An event belongs here when |
| --- | --- | --- | --- |
| `calendar.public_events` | `leigh_hackspace_events` | the Space API's `calendar=public`, `/events.ics` subscribers, `space_events`, the digest, the HA dashboard | it has an audience **off** the network: tickets, tours, public club nights |
| `calendar.member_events` | `member_events` | the Space API's `calendar=members`, `space_events`, the digest, the HA dashboard | members-only: general openings, maintenance days, the alarm test |
| `calendar.fabrication_bookings` | `fabrication_bookings` | **nothing in this repo** — not one of the two entity ids `hackspace-api` knows, not the dashboard (measured 2026-11-23: only `public_events` and `member_events` appear in `lovelace.lovelace`) | room bookings, i.e. today only HA itself sees it |

Which entity is which is `core.config_entries`' fact (the `storage_key` per entry), not
a naming convention — `calendar.public_events` does **not** back
`local_calendar.public_events.ics`. Read the config entry before naming a file.

`hackspace-api` asks HA for `/api/calendars/{entity_id}` **by entity id**
(`hackspaceapi/models/config.py`: `hackspace_public_calendar = calendar.public_events`,
`hackspace_member_calendar = calendar.member_events`), and `hackspace-mcp` accepts three
spellings of each (`tools_space.rs`: `calendar.public_events` | `public` | `public_events`).
The Space API adds the venue `LOCATION` and `GEO` itself, so a calendar row needs no
address of its own; nothing in this chain carries an address but the one fact.

## The recipe, in order

1. **Read `hostname`** (rule 27) — HA and the Space API both live on `services1`
   (10.3.1.20) since 2026-11-16, and a hop from HA to HA exits 0 and keeps none of the
   cwd (`docs/nix-dev-workflow.md`).
2. **Get the event's facts from the primary source, and the year from a second one.**
   Ticket pages often print "Thursday, December 3" with no year. Cross-check
   (a listing, the organiser's feed) and then check the arithmetic — 2026-12-03 really
   is a Thursday, and that is what makes the year in the file right rather than plausible.
3. **Send the wall-clock time, not the UTC one.** HA's TZ is `Europe/London`
   (set in the module), and `local_calendar` stores **floating** local times
   (`DTSTART:20261203T190000`, no `Z`), converting any datetime you hand it with
   `dt_util.as_local(...).replace(tzinfo=None)`. The Space API is what later prints
   `+00:00`/`Z` on those floats, so a December event sent as 19:00 London prints as
   19:00Z and a June one would print 18:00Z. Send the time the flyer says.
4. **Snapshot and count before you write** — the numbers you will want afterwards:

   ```bash
   sudo cp -p /srv/homeassistant/.storage/local_calendar.leigh_hackspace_events.ics /root/backups/<date>/
   sudo md5sum /srv/homeassistant/.storage/local_calendar.leigh_hackspace_events.ics
   sudo grep -c 'BEGIN:VEVENT' …; sudo stat -c '%s %y' …
   ```

5. **Look for the instance you might already have.** `grep -i <series name>` the file,
   and read HA's own view of the range you care about — both before writing, because
   "add the December one" and "the June one is still in there" are one read apart, and
   the June one being in the **past** is what makes it not a duplicate:

   ```bash
   HA_TOKEN=$(sudo awk -F= '/^HOMEASSISTANT_TOKEN=/{print $2}' /var/lib/hackspace-api/env)   # never printed
   curl -fsS -m 10 -H "Authorization: Bearer $HA_TOKEN" \
     'http://127.0.0.1:8123/api/calendars/calendar.public_events?start=2026-12-01T00:00:00&end=2026-12-31T00:00:00'
   ```

   **The credential is `0600 root` and rule 13 applies twice.** Take it into a variable
   and never print the file: `grep`/`cat`/`sed` on it *to a terminal* is the leak. The
   idiom that looked masked and was not is
   `sed 's/^HOMEASSISTANT_TOKEN=/*/p'` — the mask covers only the name, and the whole
   token body follows it. Ask a question of that file instead of printing it
   (`grep -c '^HOMEASSISTANT_TOKEN='`, or `${#HA_TOKEN}`), and if a long-lived token
   ever lands in a transcript, treat it as burned: rotation is in `docs/follow-ups.md`.
6. **Write it through HA**, not into the file. `calendar.create_event` is an
   **entity service**, so it is callable over the WebSocket API at
   `/api/websocket` with `{"type":"call_service","domain":"calendar","service":"create_event",
   "target":{"entity_id":"calendar.public_events"},"service_data":{…}}`. The schema
   (`components/calendar/services.py`'s `CREATE_EVENT_SCHEMA`) takes `summary` plus
   either `start_date`/`end_date` (a whole day), or `start_date_time`/`end_date_time`,
   or `in:{days|weeks}` — exactly one of the three families, start and end in pairs
   (`Inclusive`, `AtLeastOne`, `AtMostOne`), same timezone on both, and long enough to
   clear `MIN_NEW_EVENT_DURATION`; `description` and `location` are optional. The REST
   shortcuts people reach for first — `/api/assist/debug/create_event` — do **not**
   exist in this build: they answer `404: Not Found` on the pinned `2026.11.0` core,
   which is why the WebSocket is the path (and `POST /api/…` with any token will not
   find them). Ask what is callable rather than remembering it:
   `get_services_for_target` on the entity (measured 2026-11-23, `calendar.public_events`
   offers exactly `calendar.get_events` and `calendar.create_event`, and nothing that
   writes is offered for changing an existing row — see the open item below).
7. **Verify through every reader, in the order the proofs mean** — HA, then the app,
   then the door, then the subscribers, then the bot:

   ```bash
   curl -fsS -m 10 -H "Authorization: Bearer $HA_TOKEN" \
     'http://127.0.0.1:8123/api/calendars/calendar.public_events?start=…&end=…'      # HA served it
   curl -fsS -m 10 'http://127.0.0.1:8081/events?calendar=public&start=…&end=…'      # the app saw HA
   curl -fsS -m 12 'https://api.leighhack.org/events?calendar=public&start=…&end=…'  # the door serves it (public)
   curl -fsS -m 12 'https://api.int.leighhack.org/events?calendar=public&start=…&end=…'
   curl -fsS -m 12 'https://api.leighhack.org/events.ics'                            # what subscribers get
   timeout 90 curl -sS -m 80 -X POST -H 'content-type: application/json' -d '{"text":"what is on this week?"}' \
     http://127.0.0.1:8194/v1/ask                                                   # the bot's fast path (slack-agent listen)
   ```

   The bot answer comes back `"engine":"none (fast path)"`, `"rounds":0`,
   `~65 ms` — no model involved, and this is the same `space_events` the digest renders.
   **It looks seven days ahead** (`triage::events_window` → `space_events(7d)`), so an
   event ten days out is *correctly* absent today. The number that decides whether it
   is announced at all is the digest's: `systemctl list-timers | grep digest` (the
   timer is `Mon 09:00`, `Persistent=true`), and an event has to be inside seven days
   of that run. Say the arithmetic; do not call a missing event a fault.
8. **Read the file back and diff it against the snapshot** — expect your event and
   nothing else, and report what else you see rather than swallowing it (§"What the
   file does to itself" is what the expected "else" is).

## What the file does to itself, which is the reason the file is not the place you write

Any HA-side edit rewrites **the whole file**: `_async_store` serialises the entire
in-memory calendar and `_store` `write_text`s it. Measured on the one December event
added 2026-11-23: 37 → 38 `BEGIN:VEVENT`, 11950 → 12469 bytes, `SEQUENCE:0` added to
the new row, and **five pre-existing date-only rows re-emitted as
`DTSTART;VALUE=DATE:`** — same meaning, different bytes.

Both directions of the resulting lie:

- **the file is not what HA is serving.** `local_calendar` parses it once in
  `async_setup_entry` (`IcsCalendarStream.calendar_from_ics`); the periodic
  `async_update()` only re-materialises a timeline from that parsed object, and
  nothing re-reads the file while HA runs. A hand edit is therefore invisible to HA,
  to the Space API, to the bot and to the 448 subscribers, indefinitely — and the
  symptom appears far from DNS, the same shape as `docs/mqtt-int-legacy-answer-2026-11-02.md`.
- **HA owns those bytes**, so the next HA-side edit replaces whatever you hand-wrote,
  silently. The file is a channel in `AGENTS.md` rule 29's shape, read the other way.

And the reverse of the first bullet is the trap that makes people reach for a restart:
**restarting Home Assistant to pick up the file is not an escape hatch, it is the heavy
thing this repo has already been bitten by twice** — `docs/home-assistant-on-services1-2026-11-16.md`
(an HA start writes `state.lastchange` into the **public** API, because `state.open` is
HA's `input_boolean.hackspace_open`) and `docs/homeassistant-frigate-via-device-2026-11-02.md`
(twelve failed camera-entity registrations). A restore of a snapshot only becomes what
HA serves on the next HA start, so a restore is a *pair* of actions, not one
(`cp` + a decision about restarting HA that someone in the room has to make). The
snapshot for the 2026-11-23 edit is `/root/backups/2026-11-23-ha-calendar/` with a
README and the caveat written into it.

## Receipts, 2026-11-23 — the worked example (GeekUp Leigh, Thu 3 Dec 2026)

| | |
| --- | --- |
| Source | `eventbrite.co.uk/e/geekup-leigh-tickets-2003217423238` — "Thursday, December 3, 7 PM–10 PM", Leigh Hackspace; year cross-checked against the organiser's own listing (2026-12-03, a Thursday) |
| Chosen calendar | `calendar.public_events` — free public tickets, and the series' earlier instance already lives in that one |
| Before | 37 `VEVENT`, 11950 bytes, md5 `930c220d…` |
| Written | `calendar.create_event` over `/api/websocket` → `success:true`, uid `21cad31a-e728-11f1-bc9e-c8d3ffa5b225`, `DTSTART:20261203T190000` / `DTEND:20261203T220000` (floating), summary `GeekUp Leigh`, description + the ticket link |
| After | 38 `VEVENT`, 12469 bytes, five pre-existing date-only rows re-emitted as `DTSTART;VALUE=DATE:`, nothing lost |
| Proved at | HA `/api/calendars/calendar.public_events`, Space API `:8081/events?calendar=public`, `api.leighhack.org` and `api.int.leighhack.org` (both, one hit each), `/events.ics` (with the door-side `LOCATION`/`GEO`) |
| The bot | `POST /v1/ask` on `127.0.0.1:8194`: `space_events(7d)`, `engine: none (fast path)`, 65 ms — correctly absent on 23 Nov (event is 10 days out), and inside the window of the next digest, whose timer read `Mon 2026-11-30 09:00 GMT` |
| Untouched | `member_events.ics` (md5 unchanged, 0 matches for the series) |
| Docs | this file, and `docs/services.md` §"The space calendar…" keeps the identity and points here |

## Still open

- **Editing or removing an existing row on this build.** **Superseded 2026-11-24: the path
  exists, and the paragraph below was confidently wrong.** Create, update and delete are
  registered as **WebSocket commands** — `calendar/event/create`, `/delete`, `/update`,
  `/subscribe`, core `components/calendar/__init__.py:308/346/393/442` — and
  `get_services_for_target` cannot see a WebSocket command at all, which is exactly why the
  2026-11-23 measurement read a fully implemented feature as absent. Payloads are
  `{entity_id, event:{dtstart, dtend, summary, description?, location?, rrule?}}`, and
  `update`/`delete` additionally take `uid` (plus optional `recurrence_id`,
  `recurrence_range`), gated on `POLICY_CONTROL` on the entity. Receipts — probed against
  the live `2026.11.0` container, with the `.ics` md5 unchanged because an *errored* write
  does not re-serialise the file — are in `docs/events-sync-plan-2026-11-24.md` §2 row 1,
  which also states the general rule: before believing "HA cannot do X", say **which of the
  three doors you asked** (a service, a WebSocket command, or the entity's declared
  `supported_features`). Everything below is kept as the record of the wrong probe.

  `create_event` and
  `get_events` are the only *services* registered for the calendar component
  (`components/calendar/services.py`), even though the entity declares
  `CREATE_EVENT | DELETE_EVENT | UPDATE_EVENT`; the entity methods are reached by
  Assist/dashboard actions, and those (`/api/assist/debug/*`) 404 here. So "move this
  event to a new date" has no found path that is not a second event. **Probe
  `get_services_for_target` before believing this** — it is a 2026.11.0 statement,
  and HA core 2026.11 is a pinned core (`HA_IMAGE` in `machines/services1/services/home-assistant.nix`)
  that only moves when someone bumps the pin.
- **The HA token that went into a transcript on 2026-11-23** while reading
  `/var/lib/hackspace-api/env` — it is a long-lived HA credential, so it is burned
  (rule 13). The recipe above is the shape that does not print it.

Re-verify with:

```bash
sudo grep -c 'BEGIN:VEVENT' /srv/homeassistant/.storage/local_calendar.leigh_hackspace_events.ics
diff /root/backups/<date>/local_calendar.leigh_hackspace_events.ics \
     /srv/homeassistant/.storage/local_calendar.leigh_hackspace_events.ics   # only your event
systemctl list-timers --no-pager | grep digest        # the announcement window
# and get_services_for_target on the entity, for what can be called at all
```
