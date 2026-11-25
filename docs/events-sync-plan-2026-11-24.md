# Eventbrite → Home Assistant → everywhere: the events plan (2026-11-24)

**Status: a plan to execute, phase by phase, one commit per checkbox.** It is not a
post-mortem and it is not reference; when a phase is done its `Commit:` line is filled
in and the section above it becomes history. `docs/space-calendar.md` still owns **how to
write one calendar row** — read it for the recipe and the hazards; this file owns the
system, and §2 supersedes one of its "Still open" bullets.

## 0. What this buys, and the shape of it

`www.leighhack.org/events` should be an engaging listing of the hackspace's **public**
events, and Eventbrite (`eventbrite.co.uk/o/36377471193`, *Leigh Hackspace CIC*) is where
those events actually get made. Today the two are joined only by a human remembering to
copy a row into Home Assistant, so the public calendar is whatever someone last
bothered to type, `/events` is a JavaScript widget over it, and `/calendar` does not
exist.

The fix is **not** a new merged feed and **not** a change to the Space API. It is one
mirror into Home Assistant, so HA becomes the merge point and every existing reader
inherits Eventbrite without being touched:

```
Eventbrite                                    HA local_calendar
(ticketed public one-offs)                    (recurring nights, members-only)
        │  crates/events-sync — Rust, timer on services1
        │  ONE source: GET /v3/organizations/36377471193/events/ — the token is the gate (§0.1)
        │  there is no unauthenticated way to list them (§2 row 7), and a webhook is a pointer (§6)
        │  uid = eventbrite:<event id>; create/update/delete over HA's WebSocket API
        ▼
calendar.public_events  ◄── the merge point, and it owns ONLY uids it wrote
        ├──► hackspace-api  /events · /events.ics · space.json feeds      [no change]
        │        ├── 448 ICS subscriptions + 6 external pollers           [no change]
        │        └── space_events · voice · the Monday #general digest     [no change: §2 row 6]
        └──► (day-before notice: a second run of the same digest job, §3.1 — not a new formatter)
                    │
www.leighhack.org (Hugo on GitHub Pages)
        /events   = server-rendered cards: what is coming, plus a RECENTLY block (§7 decision 5)
        /calendar = the month/list widget (what /events is today), fed by BOTH calendars
```

New code: one crate, one automation, one Hugo layout, one content file. `hackspace-api`,
`hackspace-mcp`, the bot, the 448 subscribers and the front door stay untouched until
the optional phase 5.

## 1. How to tick this off

- **One commit per numbered item.** Message: `events: <item id> — <what changed>`; append
  the short sha to that item's `Commit:` line in the same commit, so the file is the
  checklist and the log is the receipt.
- **Do the `Verify:` line before the commit**, and paste what it actually printed into the
  commit message. An item whose verification was not read off the box is not done.
- **§0.1 is the only thing that blocks this plan:** with no token the mirror has no input,
  so 1.1–1.9 can be written and tested against captured fixtures, and 1.10 onward waits.
- Phases 0–1 change nothing a member can see. **Phase 2 deletes rows a human wrote**, and is
  ordered so the proof happens before the deletion (§2.2 before §2.3). Phase 3 posts into
  Slack, so it is one `--dry-run` and a read before it is armed. Phase 4 is a PR into
  `leigh-hackspace/website-hugo` (not this repo) and previews on `web-test.leighhack.org`.
- Everything machine-shaped here is on **services1** (10.3.1.20): HA, the Space API, the
  bot. Read `hostname` first — AGENTS.md rule 27, and `just deploy services1` from
  services1 is the box deploying itself.
- New files in this repo: `git add` before `just fmt`, then `just check` (AGENTS.md rule 5
  and the alejandra note under *Deploying* — `just fmt` silently skips untracked files).

## 2. Measured 2026-11-24 — the receipts that make this cheap

Eight things that were not known (or were known wrongly) before this read. Nobody should
re-derive them. Rows 1–5 are the 2026-11-24 morning read; rows 6–8 are the follow-up that
the decisions in §7 made necessary, and row 7 is the one that killed "just scrape it".

| # | Finding | Evidence |
| --- | --- | --- |
| 1 | **HA can update and delete calendar rows.** `docs/space-calendar.md` and `docs/follow-ups.md:4209` both record the opposite — that only `create_event`/`get_events` exist. True of *services*; the calendar component also registers **WebSocket commands** `calendar/event/create` (`__init__.py:308`), `/delete` (`:346`), `/update` (`:393`), `/subscribe` (`:442`), and `local_calendar` implements all three entity methods (`../local_calendar/calendar.py:143`, `:153`, `:179`) and accepts `rrule`. | Live probe against the running `2026.11.0` container: `calendar/event/delete` with a bogus uid → `{"success":false,"error":{"code":"failed", …}}` (registered, uid not found); `update`/`create` with `{}` → `{"code":"invalid_format", "required key not provided at 'event.dtstart'" …}`. Payload keys are `dtstart` `dtend` `summary` `description` `location` `rrule`; needs `POLICY_CONTROL` on the entity. `md5sum /srv/homeassistant/.storage/local_calendar.fabrication_bookings.ics` was `b01920dfb60d8ad0fab859e13a11b485` before and after — an errored write does **not** re-serialise the file. |
| 2 | **Nothing notifies Slack about any event today.** The user-visible "events in Slack" is `slack-agent-digest.service` (Mon 09:00, Rust) — not HA. | `automations.yaml` holds 20 automations (open/close → `general`, 3D-print-finished → `fabrication`, Frigate → `cameras`, floodlights, big button, WLED, heater); `grep -i calendar automations.yaml configuration.yaml packages/` → no hits. |
| 3 | **HA already ships the trigger this plan needs.** `components/calendar/trigger.py` offers `trigger: calendar` with `event: start\|end`, `offset`, and a multi-calendar `target:` — and `notify.leighhack` is a configured Slack webhook notify service whose `target:` is a channel name already in use. | `trigger.py:46`: `UPDATE_INTERVAL = dt.timedelta(minutes=15)`; `.storage/core.config_entries` carries a `notify`/`slack` entry (`api_key`, `default_channel`, `name`). |
| 4 | **Eventbrite offers no feed.** The organiser page is a client-side React calendar: 30 `calendar-day-skeleton` placeholders, exactly one server-rendered event card, and no ICS/RSS link anywhere. Every third-party "Eventbrite ICS" project exists because there isn't one; all of them use the Data API. So the sync is **API + token**, org id `36377471193`, and two costs: the full description is a **separate per-event endpoint** (`GET /v3/events/{id}/description/`, since `description.html` was deprecated), and their docs state ~1,000 calls/hour per token. | `curl https://www.eventbrite.co.uk/o/36377471193` → 200, 181,960 bytes; `data-testid` histogram; json-ld `ProfilePage` → `o/leigh-hackspace-cic-36377471193`. |
| 5 | **The site's `/events` is 100 % client-side, and the API is not the slow part.** `layouts/page/calendar.html` renders an empty `<div id=calendar>`; `assets/js/calendar.js` (15 lines) `$.getJSON('https://api.leighhack.org/events')` → `@event-calendar` `listMonth`. No image, no ticket link, no JSON-LD, no-JS fallback = one paragraph + an ICS link. `www.leighhack.org/calendar` → **404** (`calendar.html` is a *layout*, used only by `content/events.md`). | `curl` of the API: **6.4 KB in 70 ms**, `access-control-allow-origin: *`. So website issue **#18 "Slow loading on the Events page"** is the third-party scripts (jQuery from ajax.googleapis.com, `@event-calendar@2.4.1` from jsDelivr, the FontAwesome kit), not the data. The page also still emits `<!--# include virtual='/cgi/events.cgi' -->`, which can never execute on GitHub Pages (`server: GitHub.com`). |

| 6 | **The Monday digest already publishes both calendars, in time order, tagged `[public]`/`[members]`.** It does not contain the Eventbrite events for one reason only: they are not in HA. So "make the digest contain the public events" is closed by the mirror, with **no digest change**. | `slack-agent call space_events '{"days":7}'` on services1, 2026-11-24, printed: `Upcoming events — next 7 days on the both calendar, soonest first (8 found)` — `Today (Tue 24 Nov) 10:30–11:30 Fire Alarm Test [members]`, `18:30–20:30 Textile Tuesday [public]`; `Tomorrow (Wed 25 Nov) 18:30–21:30 Code Club - Public [public]`; `Sat 28 Nov` tours + general opening + electronics club; `Sun 29 Nov Workshop Maintenance Day [members]`; `Mon 30 Nov Cyber Mondays [members]`. The `calendar` argument is never set — `fastpath::calendar_args` (`crates/slack-agent/src/fastpath.rs:21`) passes only `days`, and `space_events` defaults it to `both` (`crates/hackspace-mcp/src/tools_space.rs:309`). |
| 7 | **The organiser page cannot be scraped for the list.** It server-renders exactly **one** event card, whatever the client; the rest is a client-side fetch. So "scrape the HTML" is not an option on its own — it needs a browser, or an inbox. | The same URL fetched with a Chrome UA, a Googlebot UA, bare `curl/8.13.0` and Mobile Safari: **all four 200 at 181,978 bytes, one `event-card-tracking-layer`, one `/e/…-tickets-…` link** (GeekUp). `?page=2` and `?_rsc=1` return byte-identical HTML. `__NEXT_DATA__` exists but holds i18n strings + header/footer (`"events"` appears 0 times in it); of the 30 chunks the initial HTML loads, the surface-API base is `/api/organizer-profile` but every guessed path under it 404s. |
| 8 | **An event page, by contrast, is fully scrapeable** — it server-renders a complete schema.org `Event`. Useless for *discovery* (you need its URL first), and therefore recorded as the fixture shape and the answer to "what fields exist", not as a source. | `curl /e/geekup-leigh-tickets-2003217423238` (198,558 bytes) → `ld+json`: `name`, `startDate 2026-12-03T19:00:00Z`, `endDate …T22:00:00Z`, `eventStatus: EventScheduled` (so *cancelled* is visible), `url`, `organizer`, `location.Place` with a full `PostalAddress` (Park Lane, Leigh, WN7 2LB), and `offers` `AggregateOffer` with `lowPrice 0.0` (⇒ free), `availabilityEnds`, `priceCurrency`. **`?format=json` does not exist** — it answers 200 with the HTML page. Note the `Z` on `startDate`: in December you cannot tell an honest UTC instant from a wall clock with a `Z` bolted on, which is §0.4. |

**Baseline, measured 2026-11-24 09:3x on services1** (re-measure before phase 2, and
re-measure after: the diff *is* the proof):

| file | VEVENT | bytes | md5 (8) |
| --- | --- | --- | --- |
| `.storage/local_calendar.leigh_hackspace_events.ics` (public) | 38 | 12,469 | `be934b1c` |
| `.storage/local_calendar.member_events.ics` | 11 | 3,434 | `3f99e1e5` |
| `.storage/local_calendar.fabrication_bookings.ics` | — | — | `b01920df` |

Rows naming an Eventbrite URL: **1** — `GeekUp Leigh`, uid `21cad31a-e728-11f1-bc9e-c8d3ffa5b225`,
`DTSTART:20261203T190000`. One *past* sibling in the same series, `Geekup Leigh`, uid
`0fb860f4-6382-11f1-a591-929929beaf95`, 2026-06-18, no description: the sync will not own
it and it stays.

Series with **no** Eventbrite listing at all, which the sync must never disturb:
*Textile Tuesday* (weekly Tu), *Code Club – Public* (weekly We), *General Tours* (weekly Sa),
*Hackspace CTF* (4-weekly Mo).

---

## Phase 0 — gates

**The gate is the token, and it is not a formality: there is no unauthenticated way to learn
which events exist.** §2 row 7 measures that the organiser page serves one card and nothing
else, and §6's webhook row measures that a webhook is a *pointer* to the same API. So
`crates/events-sync` has exactly one viable input, and the crate is written and tested
against fixtures until it is available.

- [ ] **0.1 The read-only Eventbrite credential.** Whoever owns the space's Eventbrite
      account mints a **personal token** (Account settings → App & tokens), least scope
      only — `user:read`, `events:read`, `organizations:read` — and **nothing that can read
      attendees or orders**: attendee PII must never be one HTTP call away from a feed with
      448 subscribers. Prefer a shared organiser identity over a member's personal account,
      because a personal token dies with that person (and on a password change).
      - Where: `secrets/eventbrite.yaml` — **a new sops file, not `secrets.yaml`**, because
        that file's cost is per-*file*: every key in it is decrypted by its own `runCommand`
        keyed on the whole file, so adding one key dirties every unit that embeds any secret
        (measured 2026-11-16). `.gitignore`'s `**/secrets/*` needs a negation line, not
        `git add -f`. `sops.secrets.eventbrite_token` → `/run/secrets/eventbrite_token`,
        `0440`, one group, the sync's own account; **no `infra.buildTimeSecrets` entry** —
        nothing needs it at build time. Handed to the unit by `LoadCredential=`, never
        `argv`, never printed (rule 13).
      - `Verify:` `curl -fsS -H "Authorization: Bearer $(cat /run/secrets/eventbrite_token)" https://www.eventbriteapi.com/v3/organizations/36377471193/` → `200`, org named.
      - `Commit:`

- [ ] **0.2 Does that token see *every* public event?** The one question the API can answer
      badly. `/v3/organizations/{id}/events/` returns only events the token's user can see,
      so a member's token may quietly under-list the organiser page. Page through it, count,
      and compare with the count a human reads off the public page.
      - Agree → the sync is authoritative (it may delete, per §1.10's retention rule).
      - Disagree → the sync runs in **reconcile-only mode: create and update, never delete**,
        and the gap is a journal line and a `degraded` field, not a silent short list. This
        is a *mode of one source*, not a second fetcher (§1.9) — scraping back in would buy
        coverage with fragility, and it still cannot reach past events.
      - Record in fixtures, dated: whether `?status=live` includes past/draft; pagination
        fields and the maximum `page_size`; what a **recurring** Eventbrite event arrives as
        (one row with recurrence data, or separate events — this decides whether the mirror
        writes series or instances, and instances keep today's HA shape, where each
        occurrence is its own row with a `recurrence_id`).
      - `Verify:` `jq '.pagination.object_count' fixtures/org-events.json` equals the human's
        count for the same window. `Commit:`

- [ ] **0.3 HA write round trip, reversible.** On `calendar.fabrication_bookings` — the
      entity **no reader in this repo consumes** (`docs/space-calendar.md`'s table), which is
      what makes it the safe place to learn the write path. Create → read back through
      `/api/calendars/calendar.fabrication_bookings` → `calendar/event/update` →
      `calendar/event/delete` → `md5sum` the `.ics` back to `b01920df…`.
      - **Mint a separate long-lived HA token for this job.** Do not reuse the one in
        `/var/lib/hackspace-api/env`: it is `0600 root`, already listed as burned and
        pending rotation in `docs/follow-ups.md`, and a shared credential means the Space API
        and a write-capable mirror fail (or leak) together. Same shape as §0.1: its own sops
        file, `/run/secrets/`, never a store path, never `argv`.
      - `Verify:` the probe's create is visible in HA's `/api/calendars/…` output, the delete
        is not, and `md5sum` returns `b01920dfb60d8ad0fab859e13a11b485`. `Commit:`

- [ ] **0.4 Which API field is the wall clock — proved with a summer fixture.** HA wants
      floating local times (`docs/space-calendar.md` step 3), and the API offers
      `start.local` + `timezone` *and* `start.utc`. Read the wrong one and every BST event
      lands an hour out on a public feed — the `docs/space-calendar.md` trap arriving from
      the other side, and cheap to catch now, expensive to be told about in July. (The event
      page's JSON-LD is a live example of the ambiguity: `startDate: 2026-12-03T19:00:00Z`,
      and in December you cannot tell an honest `Z` from a wall clock with a `Z` bolted on —
      §2 row 8.)
      - Capture **one BST-dated event** and write the expected `DTSTART` down *before*
        writing the code that produces it.
      - `Verify:` the test is seen to fail first, and the fixture is a July date, not a
        December one. `Commit:`

- [ ] **0.5 The webhook: measured, and parked.** Recorded here so nobody builds it after
      reading only the word "webhook" (§6 row). Short version: it is a **latency
      optimisation on top of the pull, not a replacement for the credential**, and the
      latency it buys is minutes we do not need for events announced a week out.
      - `Commit:` (no repo change; the row in §6 is the artefact)

---
---

## Phase 1 — `crates/events-sync`, the mirror

Native member of the `crates/` workspace (`cargo test -p events-sync`), a systemd timer on
services1, state in `/var/lib/events-sync/`. Uses `common_http`'s `get`/`post_json`. If it
is built by crane, **name the shared crates in `CRANE.crate`'s `sharedCrates` list** — it
is transitive and hand-maintained, and the only symptom of forgetting is the Nix build
saying `cannot find function get in crate common_http` (AGENTS.md rule about it).

Window: `[now − 1 week, now + 8 weeks]`. Mapping: `uid = eventbrite:<event id>`,
`summary = name.text` **unescaped** (Eventbrite returns `&#38;`), `dtstart`/`dtend` from
`start.local`/`end.local` **as floating wall clock**, `description` = full description
converted to **plain text** (HA/ICS show text; HTML into an ICS is how a row shows
`<p>` in someone's Outlook) + a trailer carrying the ticket URL and a "mirrored from
Eventbrite, do not hand-edit this row" line.

**Until the token lands, the left-hand side of that mapping is an event page's JSON-LD, not
the API** (§2 row 8) — so read the paragraph above as *the crate's internal event shape*.
Both sources fill the same struct, and everything from §1.2 down is identical whichever of
them filled it.

- [ ] **1.1 The crate, dry-run first.** `events-sync --dry-run` reads both sides and prints
      the intended diff, writes nothing. `--state`, `--token-file`, `--window-days` flags;
      exit non-zero on any refusal, with the reason in one line.
      - `Verify:` `cargo run -p events-sync -- --dry-run …` prints the live diff and
        `md5sum` of every `.ics` is unchanged afterwards.
      - `Commit:`

- [ ] **1.2 Provenance is the only delete-eligibility test.** A row may be updated or
      deleted **iff** its uid is `eventbrite:<id>`. Everything else in the calendar is
      somebody else's, including the four recurring series and the past GeekUp row. This
      is `crates/pi-agent-sync`'s rule ("touch only what you can prove you wrote") in a new
      place, and it is what makes the mirror safe to run forever.
      - `Verify:` a test with a fixture calendar containing `Textile Tuesday` and a
        non-Eventbrite uid, mirrored against an empty Eventbrite answer, asserts 0 deletes
      — and was confirmed to fail before the guard was added. `Commit:`

- [ ] **1.3 A 200 with no events is not a usable answer.** `object_count: 0`, a 4xx, a page
      whose rows are fewer than its own `pagination.object_count`, or a JSON body that
      fails to parse ⇒ **write nothing**, keep the last state, mark the run
      `degraded: true`, exit non-zero. A quiet Eventbrite failure must never empty the
      network's public calendar. This is `pi-agent-sync`'s "`/v1/models` returned an empty
      list" trap verbatim, and rule 31's "a stale answer beats a deleted one".
      - `Verify:` the crate pointed at a fixture that is a valid-but-empty response leaves
        all three `.ics` md5s untouched and exits 1. `Commit:`

- [ ] **1.4 Diff, do not rewrite.** Read HA's window first; issue create/update **only** for
      rows that actually differ. Reason, measured 2026-11-23: **every** HA-side write
      re-serialises the whole file (`local_calendar._async_store` → `write_text`), and that
      re-write re-emits date-only rows as `DTSTART;VALUE=DATE:`. Rewriting 40 rows every
      30 minutes churns the file 448 subscribers read, for no change.
      - `Verify:` two consecutive runs; the second's diff is empty and the public `.ics`
        md5 is identical between them. `Commit:`

- [ ] **1.5 Timezone discipline.** Refuse (count + journal, do not guess) any event whose
      `timezone` is not `Europe/London`. `docs/space-calendar.md` step 3 is the rule — send
      the wall clock the flyer says — and HA's `as_local_timezone` cannot rescue a wall
      clock that was wrong when it was sent.
      - `Verify:` a fixture event in `America/New_York` is reported as refused and absent
        from the diff. `Commit:`

- [ ] **1.6 One run at a time.** A lock file (or `flock`) so a second invocation refuses; the
      voice path's `turn_gate.rs` is the same lesson: a competing request is not a head
      start, it is a failed turn.
      - `Verify:` start two, the second exits immediately saying why. `Commit:`

- [ ] **1.7 The endpoint allow-list.** The crate knows exactly three URLs: the org, the event
      list, the description. No `attendees`, no `orders`, no `ticket_classes` beyond the
      cost field, no writes to Eventbrite. Encode the list as a literal and test that no
      other path appears in the code; put the *why* (attendee PII vs a public feed) in
      `crates/events-sync/README.md`.
      - `Verify:` `grep -rn "eventbriteapi.com" crates/events-sync/src` prints only the
        allow-listed three, and the test passes. `Commit:`

- [ ] **1.8 The unit, the timer, and the verbs.** `machines/services1/services/events-sync.nix`
      — a new service module, and **it is not a service until `machines/services1/services/default.nix`
      names it** (AGENTS.md: nothing will tell you otherwise). Timer `*:/*/30` with
      `Persistent=true` and a `RandomizedDelaySec`; `Restart=on-failure`; `LoadCredential`
      for both tokens; state dir `0700` on the service account; `journalctl` messages that
      answer "what did it see, what did it change, is it degraded" without a debug flag.
      New `just/events.just` (rule 26 — recipes live in `just/`, and a recipe's docstring
      travels with it: `just --list` prints the **last line** of the comment block) with
      `events-sync`, `events-sync-dry`, `events-status`.
      - `Verify:` `just switch` on services1 with the GOLDEN RULE confirm chained in the
        same invocation; then `systemctl list-timers events-sync.timer`, `systemctl
        status events-sync.service`, and the rendered unit read back from
        `/etc/systemd/system/events-sync.service` (rule 10 — never infer an `ExecStart`
        from the source).
      - `Commit:`

- [ ] **1.9 One source, and "the token sees less than the public page" is a *mode*, not a
      second fetcher.** `src/source.rs` is the only module that names `eventbriteapi.com`,
      and everything from §1.2 down — provenance, the usable-answer gate, the diff, the HA
      write, the tests — reads the struct it returns. If §0.2 measures that the token
      under-lists, the response is `--reconcile-only` (create and update, never delete, gap
      logged), **not** a second source bolted on beside it. Two fetchers means two failure
      modes and a merge, and the scrape cannot reach past events anyway (§2 row 7), so the
      one thing §4.6 needs is exactly the thing it cannot supply.
      - Keep the boundary honest the way `crates/voice-assistant` keeps its device boundary
        (`common/check-platform-boundary.nix`): assert the core names no host, *and* assert
        the source module still does — a "no X here" guard that can be satisfied by deleting
        X guards nothing.
      - `Verify:` a grep showing `eventbriteapi.com` appears in exactly one `src/` file, and
        `--reconcile-only` on a fixture with a missing uid deletes nothing and says so.
      - `Commit:`

- [ ] **1.10 Keep the history: past rows are retained, not tidied up.** Delete a row only
      when it is older than `--retention-days` (propose 120) — **never** merely because
      `end < now`. Reason: §7 decision 5 is that `/events` shows recent past events to prove
      the space is alive, and HA is where those rows live; a mirror that cleaned up after
      itself would thin the history the site has just started to publish. HA already keeps
      them: **39 public and 48 member instances in the 12 weeks before 2026-11-24** are in
      the feed right now (`/events?start=2026-09-01T00:00:00Z&end=2026-11-24T00:00:00Z`,
      both `calendar` values).
      - `Verify:` a fixture with a 30-day-old event leaves the row; a 130-day-old row with a
        claimed uid is deleted; a 130-day-old row with a foreign uid is not. `Commit:`

---

## Phase 2 — the cutover (the one hour that is not a `switch` away)

**§7 decision 3, taken 2026-11-24: the hand-made rows may go — *as long as the sync is
proven to recreate them*.** That wording is the whole phase, and it is why the order below
is "mirror first, prove, then delete" rather than the swap you would write if you were not
worried about the public feed. Between 2.2 and 2.3 the GeekUp event is listed **twice**
(two uids, same night); that is the accepted, visible, few-minutes state of the proof, and
it is what makes the deletion a consequence rather than a gamble.

- [ ] **2.1 Snapshot and count.** `sudo mkdir -p /root/backups/2026-MM-DD-events/`, `cp -p`
      all three `.ics` files, `md5sum` them into a README in that directory, `grep -c
      'BEGIN:VEVENT'` each, and write the restore caveat into that README: **a restore is a
      pair of actions** — `cp` plus a decision about restarting HA, which
      `docs/home-assistant-on-services1-2026-11-16.md` says is not free (an HA start writes
      `state.lastchange` into the public API) — and the file HA is *serving* is its parsed
      in-memory calendar, not the bytes, until it restarts.
      - `Verify:` the four numbers match this doc's baseline table or the difference is
        explained row by row before anything else happens. `Commit:`

- [ ] **2.2 Mirror first, and prove the recreate before anything is removed.** Run
      `events-sync --dry-run`, read the whole diff, run it. The claim to be tested is
      narrow and checkable: for the one hand-made row, the sync's own row
      (`uid eventbrite:2003217423238`) carries **the same night, the same wall-clock start
      and end, the same title, and a description that still names the ticket URL** — because
      if any of those four is wrong, deleting the hand row makes the feed worse, and the
      proof has to fail *before* that is possible rather than after.
      - `Verify:` diff the sync's GeekUp row against the hand row on all four fields, and
        show it in `https://api.leighhack.org/events.ics` (the public artefact, not the
        local one). Paste the two rows side by side into the commit message. `Commit:`

- [ ] **2.3 Only now retire the hand mirrors.** For each HA row whose `DESCRIPTION` names an
      `eventbrite.co.uk/e/` URL and whose uid is **not** in the `eventbrite:` namespace —
      today exactly one, uid `21cad31a-…` — call `calendar/event/delete` **by uid**, one at a
      time, re-reading `/api/calendars/…` after each (the file re-serialises per write, so a
      bulk assumption is wrong).
      - `Verify:` the *hand* uid is gone from HA, from `:8081/events?calendar=public` and
        from `https://api.leighhack.org/events.ics`, **and the GeekUp night is still in all
        three** — one row for 3 Dec, not zero and not two. `Commit:`

- [ ] **2.4 The proof that the mirror behaved.** Byte-compare `/events.ics` before and after
      and show the diff **contains only Eventbrite rows**: *Textile Tuesday*, *Code Club –
      Public*, *General Tours* and *Hackspace CTF* identical in both, and
      `local_calendar.member_events.ics`'s md5 unchanged at `3f99e1e5…`. This is the same
      shape of proof as the Space API move's byte-for-byte `space.json` diff, and it is the
      one item in this plan that says the thing did not happen to something else.
      - Then the readers, in `docs/space-calendar.md` §7's order: HA
        `/api/calendars/calendar.public_events` → `http://127.0.0.1:8081/events?calendar=public`
        → `https://api.leighhack.org/events` → `https://api.int.leighhack.org/events` →
        `https://api.leighhack.org/events.ics` → `POST http://127.0.0.1:8194/v1/ask` with
        "what's on this week" (the bot's fast path, `engine: none (fast path)`, no LLM) →
        and `slack-agent call space_events '{"days":7}'`, which is the Monday digest's body:
        the Eventbrite row must be in it, **with no change to any code in that path**.
      - And the idempotence claim, on live state: two more runs, md5 unmoved. `Commit:`

---

## Phase 3 — the day-before notice on Slack

**§7 decision 4 as amended 2026-11-24:** the Monday 09:00 `#general` digest stays exactly as
it is; what was missing from it was the Eventbrite events, and §2 row 6 measures that it
already renders both calendars sorted and tagged — so the mirror closes that with **no change
to `slack-agent` or `hackspace-mcp`**. Item 2.4 proves it by calling the digest's own tool.
Then, newly asked for: **a per-event notice the day before**.

Do it as a **second run of the same job**, not as a new formatter:

- [ ] **3.1 A daily digest instance with a short horizon.** `slack-agent digest` already
      takes `--to`, `--days` and `--title`, and the module already carries
      `infra.slackAgent.digest.{enable,channel,days,dryRun,onCalendar}`
      (`machines/services1/services/slack-agent.nix`). So this is a second `oneshot` +
      `timer` pair — `slack-agent-digest-tomorrow` — with its own four options, reusing the
      service body and its safety posture verbatim: `Restart = on-failure` and **not**
      `mkNeverGiveUp` (`mkNeverGiveUp` would re-post forever, the one thing a scheduled job
      must not do), a finite `startLimitBurst`, and `Persistent = true` so a box that was
      down overnight still posts.
      - **The horizon is the only sharp edge.** `space_events` counts *forward from now*, so
        `--days 1` run at 17:30 covers tonight, not tomorrow. `--days 2` from ~17:30 covers
        all of tomorrow (and today's evening, which a title can own: "This evening and
        tomorrow at the hackspace"). If the room wants a pure tomorrow, the honest fix is one
        new argument on `space_events` (`--from-hours`), not a template in Slack that
        re-sorts the list itself.
      - `Verify:` `slack-agent digest --to <channel> --days 2 --dry-run` prints what would be
        posted — and the Eventbrite row is in it, once phase 1 is live. `Commit:`

- [ ] **3.2 Channel and time, once, in writing.** Where the day-before line goes is a
      judgement about noise in a volunteers' workspace, not a technical one, and the two
      facts to state beside the answer: `#general` already gets Mondays, and this job fires
      **every day**, so a busy week is a daily line.
      - `Commit:`

- [ ] **3.3 (not chosen, kept) HA's `trigger: calendar`.** The alternative reading of
      "day before" is event-relative rather than wall-clock, and HA ships it:
      `trigger: calendar`, `event: start`, `offset: -P1D`, `target:` the calendar entities,
      actioning `notify.leighhack`. It is 20 lines of runtime YAML in
      `/srv/homeassistant/automations.yaml` and the primitives are measured and idle today
      (§2 row 3). Three reasons it is *not* the default here: `offset: -P1D` fires at the
      event's own clock time the night before (19:00 Wed for a 19:00 Thu), which is a poor
      hour to tell a room; HA's calendar trigger re-plans on a **15-minute** cycle
      (`UPDATE_INTERVAL`, core `components/calendar/trigger.py:46`), so the timing is
      approximate by construction; and the message would be Jinja — **a second renderer for
      one calendar**, which is exactly how `common-rs/` grew three crates. If it is ever
      wanted, `target: events` is off-flake state (the Slack app's channel grant is in
      neither a store path nor a flake field), so verify by posting once and deleting it.
      - `Commit:`

---
---

## Phase 4 — the website: `/events` becomes the listing, `/calendar` the calendar

All of this is `leigh-hackspace/website-hugo` (Hugo 0.124.1 → GitHub Pages; `server:
GitHub.com`), **not this repo**. Branch → preview on `web-test.leighhack.org` → PR, which
is what `tools/branch-deploy.sh` and `deploy-test.yml` are for. Closes site issues **#18**
(slow Events page) and contributes to **#27** (JS/CSS cleanup) and **#38** (no-JS
degradation).

- [ ] **4.1 `/calendar` is the old `/events`, and it shows *both* calendars.** Add
      `content/calendar.md` with `layout: calendar`; point the navbar's Calendar entry at it.
      **§7 decision 2, taken 2026-11-24: `/calendar` shows all events, public and member.**
      `assets/js/calendar.js` currently fetches `/events` once with no parameter, which is
      the public calendar only, so this is the one place in the whole plan that needs a
      second request: fetch `?calendar=members` alongside and merge the two arrays before
      handing them to the widget. **Every row must carry its label** — `[public]` /
      `[members]` — for exactly the reason `space_events` tags its lines: an undated,
      unlabelled list of members-only rows on a public page is how "the alarm test" turns
      into an invitation. Two feeds, one merge, no new server code.
      - While in `layouts/page/calendar.html`, delete the dead
        `<!--# include virtual='/cgi/events.cgi' -->` (Apache SSI; there is no Apache).
      - `Verify:` `web-test` `/calendar` shows an Eventbrite row, a recurring public night
        **and** a members-only row, each labelled; `curl -s '…/events?calendar=members' |
        jq length` equals the count of `[members]` rows rendered.
      - `Commit:`

- [ ] **4.2 `/events` is server-rendered cards.** New `layouts/page/events.html` +
      `content/events.md` switching to it. Data arrives at **build time**: a step (or a
      `schedule:`-triggered workflow, daily) that
      `curl`s `https://api.leighhack.org/events?start=…&end=…` into a temp data file Hugo
      renders from. Prefer fetch-at-build over committing `data/events.json`: then the
      artefact is what can be stale, not the source, and nothing but CI pushes to `main`.
      - Content, per card: day/date block, title, time range, description excerpt, and a
        **CTA** — "Book a ticket" whenever the description carries an
        `eventbrite.co.uk/e/` URL, "Add to calendar" per event. Above the list: the next
        event as a hero with a countdown. Below it: the ICS subscription line the no-JS
        paragraph already gives.
      - **Times come from the published offset, never converted to UTC** — the reason
        `crates/hackspace-mcp/src/tools_space.rs` states it is the reason here too: "the
        week the clocks change".
      - `Verify:` with **JavaScript disabled** the page still lists the Eventbrite event
        (curl the built HTML and grep for its summary — that is the no-JS proof), and
        `curl -s https://…/events/ | grep -c '<article'` ≥ the feed's event count.
      - `Commit:`

- [ ] **4.3 JSON-LD `Event` per event.** The lever with the biggest payoff, because the
      current page contributes nothing to search or Maps: emit `application/ld+json`
      `Event` objects (name, `startDate`, `endDate`, `location` = the Spinners Mill
      `PostalAddress`, `offers.url` = the ticket link, `organizer` = Leigh Hackspace CIC)
      from the same data file. Keep the existing `<link rel=alternate type=text/calendar>`
      and `config.toml`'s `calendarUrl`.
      - `Verify:` Google's Rich Results Test passes on the live URL, and `grep -c
        '"@type": *"Event"'` on the served HTML equals the number of listed events.
      - `Commit:`

- [ ] **4.4 Staleness is shown, never hidden.** The render prints "list updated
      &lt;time&gt;" whenever the fetch is older than 24 h, and a failed fetch keeps the
      previous render. Rule 31 again: keep serving the last good answer and *say which one
      you are serving*; never blank the public page because our own API had a bad night.
      - `Verify:` point the build step at a dead host and confirm the build still succeeds
        and the banner appears. `Commit:`

- [ ] **4.5 Drop the third-party weight from `/events`** — jQuery, `@event-calendar`, the
      FontAwesome kit are only needed by the widget, which now lives on `/calendar`.
      - `Verify:` the `/events` request graph names no third-party script, and #18 is
        closed with the before/after figure. `Commit:`

- [ ] **4.6 `/events` shows what has recently happened — §7 decision 5.** The ask is that
      the page prove the space is *alive*, which a list starting tomorrow cannot do. So
      `/events` renders a **RECENTLY** block under the upcoming list: the last ~4 weeks, and
      no more than about 6 cards. The data already exists and needs nothing new —
      `?start=` works today: **39 public instances between 2026-09-01 and 2026-11-24** come
      back from the same endpoint.
      - **Curate it, do not dump it.** A raw 4-week past window is 13 rows of *Textile
        Tuesday* and *Code Club*, which reads as a loop, not as a busy space: collapse
        repeats of the same series title to one card, put the one-off and ticketed events
        first, and stop at 6. "Twelve Textile Tuesdays" is the opposite of alive.
      - Style it plainly — dated, greyed, no countdown, no "Book a ticket" CTA on a past
        event (the mirror keeps `availabilityEnds`, so "tickets closed" is honest; the
        simple honest thing is to link the event page and let Eventbrite say). And per §1.10,
        **this is why the mirror must not delete past rows**: the history this block renders
        is HA's, and a tidy sync would silently empty it.
      - `Verify:` the block lists at least one dated past event with no future CTA on it,
        and its titles contain no repeats; `curl -s '…/events?start=…' | jq length` covers
        every title it shows. `Commit:`

---

## Phase 5 — optional: the rich feed (Eventbrite's images, cost, capacity)

The cards work without it (they use the hackspace artwork and the ticket link). It is only
worth its cost if the room wants real event images. `events-sync` already holds the fields.
Two shapes, and the choice is a trade of one public surface for one bot commit:

- **5a — a new public name.** The sync writes `/var/lib/events-sync/events.json`; one
  `origin` row in `common/front-door.nix` + a row in `common/int-dns.nix` + the two guards
  come along, per AGENTS.md rules 14 and 24 (`machines/gw/int-dns.nix`,
  `machines/services1/int-dns.nix`), `CONFIG.ROUTER_ONLY`, no cert on the origin.
  Blast radius: a new public endpoint, and a new name to keep honest forever.
- **5b — services1 pushes the data file into the site repo (recommended).** No new public
  surface, and services1 is the one box with a `read_only=false` repo deploy key
  (AGENTS.md rule 33) and `gh` configured. The push *is* the refresh trigger, so 4.2's
  build-time fetch disappears, and the audit trail is the commit. Blast radius: a bot
  commit on `main`, which is why it goes through the same §1.3 "usable answer" gate — never
  push an empty or degraded document.

- [ ] **5.1 Decide, then build whichever was chosen, and record the decision here.**
      `Commit:`

---

## 6. Deliberately not doing, with the reason

| Temptation | Why not |
| --- | --- |
| **Take the Eventbrite webhook instead of a token.** | Measured 2026-11-24, four reasons, and they compound. (1) **The payload is a pointer, not the event** — Eventbrite's own example is `{"api_url": "https://www.eventbriteapi.com/v3/events/26081133372/", "config": {"action": "event.published", …}}`, so you must call the Data API with a token to get a title, a date or a description: **the webhook presupposes the credential rather than replacing it.** (2) **The callback must be publicly reachable over HTTPS**, which in this network means a new door row + `int-dns` row + both guards (AGENTS.md rules 14 and 24) and a services1 origin that takes POST from the internet — the one piece of public surface the whole plan is shaped to avoid. (3) **Whether you can authenticate the sender is genuinely unclear**: Eventbrite's docs once specified an `x-eventbrite-signature` HMAC, and at least one operator's integration records the opposite — "Eventbrite does not sign webhook deliveries, and its webhook API takes no signing secret… the adapter still required an `X-Eventbrite-signature` HMAC, so every real delivery got a 401" (CenterCoopMedia/eventrunner PR #494). Assume unsigned until the app we create proves otherwise, and "unauthenticated POSTs into a calendar the world subscribes to" is not a shape to adopt for a latency win. (4) **It cannot backfill, delivery is minutes late by their own docs, and a `410` unsubscribes you** — so a services1 deploy that answers badly can silently delete the subscription, and nothing in this flake would ever ask. What to do instead: the token + the timer (§0.1). If it is ever wanted, the only right shape is a **nudge** — the webhook sets "pull now", and the pull stays the sole writer. |
| Paste each ticket URL into an inbox file so the mirror has an input before the token exists. | This was in the plan for one revision and it is gone. It existed only because *discovery* is the thing scraping cannot do, and a file of URLs is a human-shaped workaround for a missing credential — one more thing to keep, one more place a mistake becomes public. Fix the credential (§0.1); the crate's only local state then is `/var/lib/events-sync/last-run.json`, which is a report, not an input. |
| --- | --- |
| Merge in `hackspace-api` (a Python FastAPI service we run but do not build) | It is the **read fan-out**, not a place to put logic: 448 `/events.ics` subscriptions and six self-identifying external pollers (`spacer-fetch`, Maps of Making, `mapall.space`, `SpacePush`, `HackerspacesGlobe`, the news feed) plus `space_events` and the digest. Merging upstream of HA means all of them inherit Eventbrite with zero code change. AGENTS.md rule 1 also says no new Python. |
| Query Eventbrite from the browser | A token in JS is a public token, and the call volume lands on us. And the description is a per-event second call, so the "obvious" client version is slow too. |
| Two-way sync / create Eventbrite events from HA | Eventbrite takes money and holds attendee PII. We read. |
| Hand-editing `.storage/local_calendar.*.ics` — including "to fix a duplicate" | HA parses it once in `async_setup_entry` and nothing re-reads it while it runs, so a hand edit is invisible to the feed; and the next HA-side write replaces the bytes. `docs/space-calendar.md` owns this, and §2 row 1 now gives the real write path. |
| Restart HA to pick something up | Not an escape hatch: two documented bites, and it writes `state.lastchange` into the public API. |
| Move `www.leighhack.org` off GitHub Pages to get a CGI | The SSI line in the layout is residue, not a requirement. Static + a build-time fetch does the job. |
| Scrape the organiser page with a headless browser | It is the only way to make scraping *discover* events (§2 row 7), and the price is a ~500 MB Chromium on the box that already runs HA, Authentik, Frigate and Postgres in 15 GiB, breakage on every markup change, the closest thing in this plan to a ToS problem — and it still cannot fetch **past** events, which §4.6 now wants. The inbox costs one paste per event and keeps every other property the mirror needs. |
| Publish Eventbrite's ticket URLs by scraping *their* search page | Same fragility, more volume, and it is the part of their site that most aggressively notices. |
| A `calendar.eventbrite` entity of its own, to keep the mirror separate | Rejected for this phase: a fourth entity is invisible to `hackspace-api` (`models/config.py` knows exactly two calendar entity ids), to `space_events`, and to the dashboard, so it would be a feed nobody reads. Revisit only if the room wants Eventbrite separable at the API layer. |

## 7. Decisions

**Taken by the owner, 2026-11-24.** Each is a decision, not a discovery — write the reason
next to it, because the next reader will otherwise "improve" it.

1. **No Eventbrite token yet → "so scrape the HTML for now."** The scrape was measured and
   it cannot work as the source: the organiser page serves **one** event card to every client
   tried (§2 row 7), and an event page is scrapeable but you need its URL first (§2 row 8) —
   **discovery is the missing thing, and only the API supplies it.** The webhook answer comes
   back the same way: its payload is a pointer to the API, so it *presupposes* the token
   (§6 row 1, §0.5). So the instruction is honoured as far as it goes — the crate is written
   against captured fixtures, and the deploy is gated on one credential. What was dropped is
   the human-shaped workaround that would have filled the gap in between (a paste-inbox);
   it is in §6 with the reason, and git holds it if anyone wants to disagree.
2. **`/calendar` shows all events — public *and* member.** Implemented as two fetches and a
   merge, with every row labelled `[public]` / `[members]` (§4.1). The label is the part of
   the decision that keeps it safe: 48 member instances in twelve weeks are on a page the
   world can read, so an unlabelled list is the one way this becomes a disclosure. `/events`
   stays public-only until someone says otherwise.
3. **Delete the hand-made rows — once the sync is proven to recreate.** This is now an
   ordering rule, not a permission (§2.2 → §2.3): mirror, diff the recreated row against the
   hand row on night/time/title/ticket-link, show it in the public `.ics`, and only then
   delete the human's uid. The brief double-listing between the two steps is the accepted
   cost of a proof that can fail safely.
4. **The Monday 09:00 `#general` digest is right as it is; it just needs the public events —
   and *additionally*, a per-event notice the day before.** First half is measured rather
   than assumed: the digest already renders both calendars, sorted and tagged (§2 row 6), so
   the missing Eventbrite rows arrive with the mirror and **nothing in `slack-agent` or
   `hackspace-mcp` changes**. Second half is §3.1: a **second run of the same deterministic
   job** with a short horizon (`--days 2` at ~17:30, its own timer), chosen over HA's
   `trigger: calendar` with `offset: -P1D` because that fires at the event's own clock time
   the night before, is 15-minute-approximate by construction, and would put a **second
   renderer** for one calendar into Jinja. HA stays the single source either way.
5. **`/events` shows recent past events, to show the world we are alive.** §4.6: a RECENTLY
   block, ~4 weeks, ≤6 cards, repeats of one series collapsed, no ticket CTAs on past rows.
   And the constraint it puts on the mirror — §1.10, retain past rows rather than tidy them,
   because the history the new block publishes is the history HA keeps.

**Still needed — one blocks the deploy, two are five-minute answers:**

6. **The Eventbrite account's owner, and the token** (§0.1). This is the only thing that
   blocks the mirror: with no token there is no input at all, so the crate can be built and
   tested but not run. Ask for a **shared organiser identity** rather than a member's
   personal token, because a personal token dies with that person and on a password change,
   and the failure mode of a forgotten renewal is a quietly stale public calendar (§1.3),
   not an error.
7. **Channel and time for the day-before notice** (§3.2). `#general` already carries Mondays
   and this job is **daily**, so a busy week means a line every day; the two candidate
   answers are a quieter `#events`, or `#general` with the overlap accepted. A one-line
   answer either way; it is a judgement about noise in a volunteers' workspace, not a
   technical one.
8. **Whether `/events.ics?calendar=members` is advertised anywhere.** It already works; the
   decision is only whether the site *points at it*, which is decision 2 again in ICS form.

## 8. Two findings this read turned up, unrelated to the plan

- **`/srv/homeassistant/configuration.yaml` holds the authentik OIDC `client_secret` for
  Home Assistant in plaintext** (`auth_oidc.client_secret`, ~0644 root, a runtime file the
  flake only mounts), and reading that file to check *whether* it had a secret prints the
  secret: the filter that looked safe covered `api_key|token|password` and `client_secret`
  is none of those. Treat it as burned — it is a credential that can mint HA logins through
  authentik — rotate the HA application's secret in authentik, and move it out of
  `configuration.yaml` (a sops file read with `!env_file`, or the UI config entry). Add it
  to `docs/follow-ups.md` next to the HA long-lived-token rotation, with this sentence as
  the how-it-was-found receipt. Rule 13 applies to a file you only meant to *ask a question
  of*; the idiom has to be `grep -c`, never `grep`.
- **The plan's premise was recorded as impossible.** Two docs stated, with receipts, that
  HA could not change or delete a calendar row, and a whole workaround class ("add a second
  event and leave the old one") followed from it. What had actually been probed was
  `get_services_for_target` — which **cannot see a WebSocket command**, so a fully
  implemented feature read as absent. The general rule, worth its own line in AGENTS.md or
  `docs/space-calendar.md`: before believing "HA cannot do X", probe the three doors —
  services, WebSocket commands, and the entity's declared `supported_features` — and say
  which one was asked.

## 9. Commit log

| Phase | Items | Commits | The one number/date read off the box |
| --- | --- | --- | --- |
| 0 gates | 0.1 token · 0.2 does it see everything · 0.3 HA write probe · 0.4 which field is wall clock · 0.5 webhook parked | | fabrication `.ics` = `b01920df…`, and the July fixture's `DTSTART` |
| 1 the mirror | 1.1 dry-run · 1.2 provenance · 1.3 usable answer · 1.4 diff-only · 1.5 tz · 1.6 lock · 1.7 allow-list · 1.8 unit+verbs · 1.9 one source + reconcile mode · 1.10 retention | | two runs, same md5; 30-day-old row kept, 130-day claimed row gone |
| 2 cutover | 2.1 snapshot · 2.2 recreate · 2.3 delete hand row · 2.4 proof | | public `38 → ?` and back to `?`; four series byte-identical; `members` md5 still `3f99e1e5…` |
| 3 the day-before notice | 3.1 second digest timer · 3.2 channel+time · 3.3 HA trigger (not chosen) | | `digest --days 2 --dry-run` printed, with the Eventbrite row in it |
| 4 the site | 4.1 `/calendar` both · 4.2 `/events` cards · 4.3 JSON-LD · 4.4 staleness · 4.5 drop the JS · 4.6 RECENTLY | | no-JS curl shows the event; rich-results pass; `[members]` rows counted |
| 5 optional | 5.1 decide 5a/5b | | — |
