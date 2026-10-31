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

Plus the two library primitives the page now depends on (`common_ui::tabs`,
`common_ui::checkbox`) — asserted on the roles and state attributes the primitives put on
the DOM, because if the page stopped using them the styling would still look right while
the keyboard and the screen reader lost the widget.

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

## The one thing the suite asserts against the library, not the app

The arrow keys move the *roving focus*; they do not activate. That is what upstream's
`focus_next` does, and the suite asserts it as such — focus is not selection — then asserts
activation separately (the click, and `app.rs`'s `choose` writing the fragment). A suite
that asserted "ArrowRight changes the tab" would be testing the app's old behaviour and
would pass against a primitive that had changed.

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

Ran 2026-10-31: ten checks, all pass.
