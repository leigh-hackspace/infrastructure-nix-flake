# switch-gui headless-browser test suite

`just switch-gui-test` runs it. It builds the SPA and the binary, starts a
**fake ProCurve console**, points the service at it, and drives the page with
headless Chromium — asserting on the device's own state as well as the page's.

```
tests/
  run.sh            build, start, wait for, run; scratch schema; cleanup
  fake-switch.mjs   the fake device: a real Telnet server + an in-memory model
  suite.mjs         the tests
  package.json      playwright (the only dependency)
```

## Why a fake device

The tool's whole job is that a tick in a table becomes a line on a console
somewhere. Between the two are: the page's overlay, `POST /api/intent`, the
queue in Postgres, the sync's claim, the console session, the CLI, the re-read,
the shadow copy, and the page again. **Every layer between the checkbox and the
device has already been broken once in a way that looked like the layer next to
it**, so a test of one layer passes while the feature is broken.

The suite therefore asserts on three things per change: what the queue says,
what the **device** says, and what the page says after the sync has re-read.
The device is the only end of the pipeline that cannot agree with a bug.

The fake is a real Telnet server rather than a stubbed client because the tool
has no API to stub: it speaks a console protocol (LF-only lines, a keypress gate
that eats a character, `-- MORE --` paging, a config command whose echo and
prompt share a row, `enable` as a *second login*). Faking below that boundary
would test the fake. The protocol details it implements are the ones
`src/switch.rs` and `src/parse.rs` depend on, taken from the real captures in
`src/parse.rs`'s fixtures.

Only ports **23 and 24** of the fixture are ever written to: they are
administratively shut and link-down, which is the same choice an operator makes
when they want to test a change.

## Running it

```bash
just switch-gui-test                     # the lot
just switch-gui-test                     # (devshell: cargo, rustc, node)
cd switch-gui/tests && SG_TEST_SKIP_BUILD=1 ./run.sh   # reuse bundle + binary
SG_TEST_KEEP=1 SG_TEST_TMPDIR=/tmp/sgk ./run.sh        # leave it up to poke at
```

Needs: the local Postgres cluster (the service's own DSN over the unix socket,
and the `switch_gui` role, which owns the database and may create schemas), and
`nix` for the wrapped Chromium — Playwright's own download is unwrapped and
cannot find `libglib` on NixOS. `SG_TEST_BROWSER` overrides it.

Each run gets its own Postgres **schema** (`sgtest_<pid>`), so runs do not
interfere and nothing can touch the real shadow copy. The fake device is
reachable on `SG_TEST_SWITCH_PORT` (Telnet) and `SG_TEST_STATE_PORT` (HTTP:
`/state`, `/log`, `/meta`, `/reset`, `/fail`, `/offline`, `/online`), which is
how a test reads what the "device" believes and how it makes it refuse or
vanish.

## The hooks the page carries for this

Like filestore's `data-fs-*`, these exist only for the tests and are marked as
such where they are defined (`frontend/src/page.rs`):

| hook | where | identifies |
|---|---|---|
| `tr[data-port="23"]` | a matrix row | one port, by the switch's own id (`23`, `Trk1`) |
| `input[data-vlan="228"]` | a matrix checkbox | one VLAN column, by id |
| `button[data-apply="23"]` | a row's Apply | only present when the row is editable |

Selectors deliberately do not count `td`s: the VLAN columns come from the
device, so a column index is a fact about a switch, not about the page.

## What it has already caught

* **A tick that unsets itself and nothing happening.** `sync::run` returned
  early (poisoned console lock, or a failed connect) without naming the changes
  it had been handed; `record` only writes the rows named in `applied`/`failed`,
  so a claimed change stayed `applying` with its attempt spent, no error, no
  journal line, and invisible to every error path until `recover_stale` released
  it fifteen minutes later. `run` now fails the batch on every path that never
  reached the console, `claim_batch` fails what it claimed if a later claim
  errors, and `pass` fails the batch if the console task dies.
  `assertNothingStuck()` is a post-condition of every test, not just one.
* **Tagged memberships that were stored and never shown.** The shadow copy
  wrote `port.tagged` as JSON strings and every reader parsed `Vec<i64>`, so the
  matrix rendered a page of unticked boxes for a switch with 19 tagged
  memberships — and a membership diff built from it could not see a membership
  to remove.
* **A claim that never returned what it claimed.** `UPDATE … ` with no
  `RETURNING` answers with a command tag and no row, so `fetch_optional` yielded
  `None`, `claim` reported "nothing queued", and the queue silently did nothing.
* **A refusal that looked like a dead button.** Ticking a port's own native VLAN
  asks for a state the switch has no room for — a port is not tagged in the VLAN it
  is untagged in — and `commands::port_members` refused it, correctly, into a banner
  at the top of a page with fifty port rows above the matrix. `Apply` also dropped
  the row's overlay *before* the reply, so the row snapped back to the switch
  whatever the answer was, and a refusal wrote nothing to the journal either: the
  report was "the checkboxes reset and nothing happened" with no trace anywhere.
  The page now keeps that rule as it edits (the native VLAN's column is not
  tickable, and moving the native VLAN off a ticked VLAN unticks it), a refused
  intent puts the overlay back so the edit survives its own refusal, the banner is
  pinned, and `POST /api/intent` logs its refusals.
* **A fixture that lied.** `padEnd(8)` on the eight-character string `Untagged`
  glued it to `Learn`; `members()` reads a row that is not four fields as the
  *end of the table*, so every untagged membership vanished and the page showed
  an empty native VLAN for a port the device reported as untagged in 225. The
  column templates keep a literal space after every padded value for that
  reason.

## Adding a test

Put the device in a state, do the thing in the browser, then assert on the
device, then on the page. `open()` reloads (which is also how a test gets the
shadow copy's latest read into the DOM); `waitShadow()` waits until the shadow
copy agrees with the device port for port, which is what a page assertion
actually depends on.

Two traps the suite has already eaten:

* **`latestChange()` includes applied rows** — `clearQueue()` only forgets what is
  still open. A test that waits for "the newest change is applied" passes on the
  previous test's row, so read the id first and wait for one newer (and check
  `ports` names the port the test is about).
* **An error note is on the page before anything is clicked** — the fake switch
  answers every read with the RADIUS warning, which renders as `.note.err`. The
  answer to a click is in the banner (`.note.banner`, `noteText()`).
* When the page can no longer be made to ask for the impossible, fake the refusal
  at the HTTP boundary: `page.route('**/api/intent', r => r.fulfill({ body:
  JSON.stringify({ ok: false, error: '…' }) }))`. That is what the
  "the row keeps the edit that was refused" test does.
* **A matrix cell is a `button[role=checkbox]`, not an `input`** (the component
  library's `Checkbox`, `crates/common-rs/ui/`). `isChecked()`/`check()`/`isEnabled()`
  only speak to form controls, so the suite asserts the announced state: `aria-checked`,
  `aria-label`, `aria-describedby`, and the DOM `disabled` plus `data-disabled` — the two
  channels the widget actually emits (it does not emit `aria-disabled`). The selector is
  `td.ck[data-vlan=…] [role=checkbox]`, with `data-vlan` on the cell rather than the
  widget, because the cell is this app's markup and the control inside it is the library's.
  The a11y test is the one that proves the widget is what made the page readable: name,
  announced state, Space-ticks/Enter-does-not, and the reason box shown on focus.
