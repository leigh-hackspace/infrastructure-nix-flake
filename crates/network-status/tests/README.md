# network-status headless-browser test suite

`just network-status-test` runs it. It builds the SPA and the binary, starts the service on
a loopback bind with `--local`, and drives the page with headless Chromium.

```
tests/
  run.sh       build, write the fixture lease file, start, wait for, run; cleanup
  suite.mjs    the tests
  debug.mjs    not a test: loads the page, prints console errors and what rendered
  package.json playwright (the only dependency)
```

## What the suite is for

The port replaced imperative DOM with a reactive render, and the three things the
imperative version guaranteed are exactly the three a reactive rewrite loses first:

1. **the per-panel error** — one endpoint can refuse on its own, and the panel that failed
   must show its reason while the panels that can answer still answer;
2. **the canvas redraw** — a draw has to wait for the mount, re-run on every data change,
   and read the tab so a chart in a hidden panel is not left at the size it had when it was
   hidden;
3. **the tab and the URL being the same fact** — the fragment seeds the tab on load, a tab
   change writes the fragment, and `hashchange` lets the browser's own history drive it.

Plus the library primitives the page now depends on — `common_ui::tabs`, `checkbox`,
`toggle_group` and `progress` — asserted on the roles and state attributes the primitives put
on the DOM, because if the page stopped using them the styling would still look right while
the keyboard and the screen reader lost the widget.

And one check that is not about behaviour at all: **the sheet covers the DOM**. Both of these
are rules this suite learned the expensive way, and they are here rather than only in the
check names because neither one is guessable from the outside:

1. **Select a tab by its label, never by its position.** The suite was written against four
   tabs and passed. `Uplink` went into `PANELS` at position 2 (`5ebe2d5`, 2026-11-09 17:10) and
   four checks went red silently: `#connections` came back as `#uplink`, the canvas check looked
   for a canvas in a hidden tab, the "moving now" box was not on the panel it clicked. A click
   on the wrong tab still clicks something, so nothing failed loudly. It was found two hours
   later because the restyle needed the suite — and that is the whole point: nothing *runs*
   these suites (the open item is in `docs/follow-ups.md`), so a red result is only ever found
   by whoever happens to need it, and the last receipt in this file said "all pass".
   `suite.mjs` resolves every tab through `tabsByName(…)`, which times out with the label it
   could not find. Same reasoning as naming a mount point rather than a disk: the position is a
   fact about an array someone is entitled to reorder.
2. **An unstyled page passes every behavioural check, so the styles get a check too.** The
   port rewrote the stylesheet from scratch keyed on classes the Rust does not emit
   (`.pill.ok` where the component says `pill good`, `.issues li` where it says `.issue.bad`)
   and dropped the rules for the interface cards, the table controls, the legend, the status
   dots and the rate bars entirely. The page rendered and the suite passed while a user saw a
   wall of unstyled text with three white OS `<select>`s on a dark background — including a
   pill that could never turn green while reading "problem detected", which reads as a label
   rather than an alarm. The story and the library side of it:
   `docs/component-library.md`, "The stylesheet is app code too". Two checks keep it from
   happening again, and they are deliberately different halves: the **stylesheet guard** walks
   every tab and refuses any class the rendered DOM carries that no rule in the document
   mentions; the **painted check** then reads *computed* styles, because a rule can exist and
   still do nothing — a `width` on an inline element, a `span` used as a status dot with no
   size, a legend swatch with a colour and no box were each styled and still invisible. No
   screenshot and no baseline in either, so neither has anything to rot.

Every check is a **poll with a label**, not a fixed wait: a page that never renders the
thing should fail immediately and say which panel was empty.

## Why the lease table reads a fixture file

The two data sources this page can fail on are group-read files on the router that belong
to a service account (`machines/gw/conntrack-reader.nix`, `machines/gw/lease-reader.nix`).
A suite that read them would report a result that depends on which box it ran on, so:

- **the lease file is a fixture** in the run's scratch directory — the same format, the same
  parser (`crates/network-status/src/leases.rs`), the same endpoint, so the render path is
  the real one. One of its rows carries a hostname that is markup, which is only worth
  checking through a path that parses it: the test is that the vnode layer escapes text
  nodes, not that `esc()` was called.
- **that fixture has two leases and the run passes `--vlan vlan226=10.3.2.0/24`**, so one is
  inside a block the router was told about and one is not. `known` is what decides whether a
  lease's length — and so its countdown bar — is knowable at all, and the bar's two states are
  two different claims the page makes. One row could only ever show one of them; two rows show
  a determinate `aria-valuenow` and an indeterminate bar that is refused a number, in the same
  table, through the same parser.
- **the connection table is left on but comes back empty** on a test box (`/proc/net/nf_conntrack`
  is `0440` here). That is the honest shape to drive a control against — an empty table with
  working controls — rather than a stubbed response.
- **the error path is driven by the suite** (one endpoint answered with a 503), and in the
  order that matters: the panel is loaded with real data *first*, then the endpoint starts
  refusing. Showing the reason only when there is no data would leave a stale table reading
  as a current one — which is the failure mode the rule exists to prevent, and the one the
  SolidJS page had.
- **the fixture's mtime is back-dated by the suite** (`fs.utimesSync` on `NS_TEST_LEASES`).
  That is the only way to drive the `stale` verdict without a dead dnsmasq, and it changes
  nothing about the rows — the numbers stay the same, only the channel changes. So the check
  measures exactly the thing the incident turned on: whether the page can tell a quiet
  network from a file nobody is writing to, and whether it says so before an operator
  concludes a device is absent.

## What the suite asserts against the library rather than the app

The arrow keys move the *roving focus*; they do not activate. That is what upstream's
`focus_next` does, and the suite asserts it as such — focus is not selection — then asserts
activation separately (the click, and `app.rs`'s `choose` writing the fragment). The same
two-halves assertion is made of the `toggle_group`'s segments, because both widgets are built
on `collection` and a change upstream would otherwise show up as a keyboard user silently
selecting things. A suite that asserted "ArrowRight changes the tab" would be testing the
app's old behaviour and would pass against a primitive that had changed.

## Playwright on NixOS

Playwright's own browser download is unwrapped and cannot find `libglib`, so `run.sh` uses
nixpkgs' wrapped browsers (`nix build nixpkgs#playwright.browsers-chromium`) and passes the
executable path explicitly. Override with `NS_TEST_BROWSER`.

Environment: `NS_TEST_PORT` (default 18091), `NS_TEST_KEEP=1` (keep the scratch directory),
`NS_TEST_SKIP_BUILD=1` (reuse the existing bundle and binary — but a stale bundle means the
suite tested a page nobody can deploy), `NS_TEST_RELEASE=1`.

## Traps hit

- **Port 18091 can already be taken** by a `network-status` left running by an earlier run
  (`NS_TEST_KEEP=1`, or an aborted tool call that did not stop what it started). The suite
  then fails at the *first* fetch, which reads like a broken page rather than like a
  leftover process: `pgrep -a -f network-status` before believing the suite.
- **A hidden tab has no canvas in the document.** `TabContent` renders only the selected
  panel, so a selector like `[role="tabpanel"] canvas` must select the tab the canvas is in.
- `just network-status-test` is a build-shaped recipe, so it refuses on `gw` (`_build-host-guard`).

Ran 2026-10-31: ten checks, all pass — the receipt that was still standing when the suite
below went red.

Ran 2026-11-09 (services1, full build): fourteen checks, all pass. The last two are new with
the restyle and both were **falsified** rather than trusted: deleting the `.iface`/`.iface-grid`
rules makes the stylesheet guard fail and name `iface-grid` while the other thirteen stay green,
and setting `.dot` to `0×0` makes the painted check fail with "the status dot is 0×0px" while
the guard stays green — which is the pair's whole division of labour, a missing rule and a rule
with no effect. Both were run against the real page and the real binary, and the sheet was
restored afterwards (`/tmp/negtest.sh`, `/tmp/neg2.sh` on services1, the log at
`/var/tmp/ns-neg*.log`).
