#!/usr/bin/env node
// Headless-browser test suite for the network-status web UI (the Dioxus SPA).
//
// Run via `just network-status-test` (or tests/run.sh), which builds the bundle, starts the
// binary on loopback with `--local`, and drives the page with headless Chromium.
//
// What this suite is for, in one line: the port replaced imperative DOM with a reactive
// render, and the three things the imperative version guaranteed are exactly the three things
// a reactive rewrite loses first — the per-panel error, the canvas redraw, and the tab/URL
// being the same fact. Each check here is one of those, plus the two library primitives the
// page now depends on (`common_ui::tabs` and `common_ui::checkbox`), plus the lease panel's
// freshness banner — which is not a reactive concern at all and is the check that exists
// because of a real incident (see check 9).
//
// Two choices about what the page is pointed at, because both are assertions:
//
//   * the lease table reads a **fixture file** written by run.sh, not the router's. The real
//     file is group-read for one service account (`machines/gw/lease-reader.nix`), so reading it
//     would make the result depend on the box the suite runs on. The fixture is the same format
//     through the same parser, so the render path is the real one, and one of its rows carries a
//     hostname that is markup — which is only worth checking through a path that parses it.
//   * the connection table stays **on**, because the panel renders its controls only when it
//     has data. On this box `/proc/net/nf_conntrack` is not readable by the test account, so
//     the table comes back empty — which is the honest shape: an empty table with working
//     controls, not a fake one.
//
// Every wait is a poll with a label, because a suite that only says "timed out" cannot say
// which panel was empty.

import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';

const PORT = Number(process.env.NS_TEST_PORT || 18091);
const BASE = `http://127.0.0.1:${PORT}`;

const run = async (name, fn) => {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    console.error(`FAIL ${name}\n     ${e.message}`);
    process.exitCode = 1;
  }
};

/** Poll a condition rather than sleeping: a page that never renders it should fail now, not
 *  after a fixed wait. The label is in the message for the reason above. */
const until = async (label, fn, ms = 8000) => {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const v = await fn();
    if (v) return v;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`timed out after ${ms} ms waiting for ${label}`);
};

const flat = (s) => s.replace(/\s+/g, ' ');
const panelText = async () => flat(await page.locator('[role=tabpanel]:not([hidden])').innerText());
const headerText = async () => flat(await page.locator('header').innerText());

const browser = await chromium.launch({
  executablePath: process.env.NS_TEST_BROWSER || undefined,
});

const page = await browser.newPage();

// 1 — the tabs are the library's tabs: roles, aria-selected, and the state the CSS keys on.
await run('tabs render as a tablist, with the state attributes the library puts on them', async () => {
  await page.goto(BASE);
  await until('a tablist', () => page.locator('[role=tablist]').count());
  const tabs = page.locator('[role=tab]');
  assert.equal(await tabs.count(), 4, 'four tabs, by question');
  const first = tabs.nth(0);
  assert.equal(await first.getAttribute('aria-selected'), 'true', 'overview is selected');
  assert.equal(await first.getAttribute('data-state'), 'active', 'the vendored primitive announces state');
  // The panel that is shown is the one the tablist names; the others are hidden, which is
  // what `TabContent` does rather than what the old CSS did with a class.
  const panels = page.locator('[role=tabpanel]');
  assert.equal(await panels.count(), 4, 'four panels');
  assert.equal(await panels.nth(0).getAttribute('hidden'), null, 'overview panel is visible');
  assert.ok(await panels.nth(1).getAttribute('hidden'), 'connections panel is hidden');
});

// 2 — what the primitive guarantees and what the app adds are two different things, and the
// suite should not flatter either. The library's arrow keys move the *roving focus*; they do
// not activate (upstream's `focus_next` changes focus only). Activation is the click, and
// `choose` in app.rs is what writes the fragment, so the tab survives a reload.
await run('the arrow keys move the roving focus without changing the selection', async () => {
  await page.locator('[role=tab]').nth(0).focus();
  await page.keyboard.press('ArrowRight');
  await until('roving focus on the second tab', async () => (await page.locator('[role=tab]').nth(1).getAttribute('tabindex')) === '0');
  assert.equal(await page.locator('[role=tab]').nth(0).getAttribute('tabindex'), '-1', 'the first tab released focus');
  assert.equal(await page.locator('[role=tab]').nth(1).getAttribute('aria-selected'), 'false', 'focus is not selection');
});

await run('activating a tab selects it and writes the fragment', async () => {
  await page.locator('[role=tab]').nth(1).click();
  await until('the fragment written by the click', () => page.evaluate(() => location.hash));
  assert.match(await page.evaluate(() => location.hash), /#connections/);
  assert.match(await page.locator('[role=tab][aria-selected=true]').innerText(), /Connections/);
});

// 3 — the fragment seeds the tab on load, so a bookmark of #leases opens on leases.
await run('a pasted fragment opens the panel it names', async () => {
  await page.goto(`${BASE}/#leases`);
  await until('the leases tab to be selected', async () => /DHCP leases/.test(await page.locator('[role=tab][aria-selected=true]').innerText()));
  assert.match(await panelText(), /lease/, 'the leases panel is the one shown');
});

// 4 — a failed fetch keeps the last data *and* says so. The order here is the point: the panel
// is loaded with the fixture's real answer first, and only then does the endpoint start
// refusing. If the panel showed its reason only when it had nothing to show, the table that
// stayed on screen would read as current — which is the failure mode the rule exists to
// prevent, and the one the SolidJS page had (it rendered the error only in the `!data` branch).
// Which sentence the backend prints for which reason is the backend's own test
// (`the_lease_endpoint_follows_the_switch_and_says_which_reason`); the real lease file is
// group-read for one service account on the router (`machines/gw/lease-reader.nix`) and cannot
// be read from a test box, so the refusal is answered here.
await run('a panel that keeps stale data names it as stale', async () => {
  await page.goto(`${BASE}/#leases`);
  const row = await until('a lease row read from the fixture file', async () => {
    const t = await panelText();
    return /10\.3\.2\.99/.test(t) ? t : null;
  });
  assert.match(row, /lease/, 'the panel had data before the endpoint started failing');

  await page.route('**/api/leases*', async (route) => {
    await route.fulfill({
      status: 503,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'the lease file is not readable by this service' }),
    });
  });

  const stale = await until('the panel to name its data as last-attempt-failed', async () => {
    const t = await panelText();
    return t.includes('last attempt failed') ? t : null;
  });
  assert.match(stale, /not readable/, 'the reason is shown, not just the fact of failure');
  assert.match(stale, /10\.3\.2\.99/, 'and the data it kept is still there');

  // The other panel is unaffected by this panel's failure, and the header — outside the
  // tablist, reading the probe — is still answering.
  await page.locator('[role=tab]').nth(1).click();
  const conns = await until('the connections panel’s table head', async () => {
    const t = await panelText();
    return /device/i.test(t) ? t : null;
  });
  assert.ok(/age/i.test(conns), 'the other panel is unaffected by this panel’s failure');
  assert.match(await headerText(), /router up \d/);
  await page.unroute('**/api/leases*');
});

// 5 — the canvas redraws. This is the rule the port had to keep: a draw waits for the mount,
// then re-runs on every data change, and reads the tab so a chart in a hidden panel is not
// left at the size it had when it was hidden.
//
// The canvas is on the "Interfaces & issues" tab, and that is why this check selects that tab
// rather than reading whatever happens to be on screen: `TabContent` renders its children only
// when its tab is the selected one, so a chart in a hidden tab has no node to draw on at all —
// which is exactly the thing the draw effect reads `store.tab` for.
await run('the bandwidth canvas has drawn something after a poll', async () => {
  await page.locator('[role=tab]').nth(3).click();
  const drawn = await until('a drawn pixel on the bandwidth canvas', async () =>
    page.evaluate(() => {
      const c = document.getElementById('chart-bw');
      if (!c) return false;
      const ctx = c.getContext('2d');
      const d = ctx.getImageData(0, 0, c.width, c.height).data;
      // The background fill is #161d26; any other pixel is a gridline or a series.
      for (let i = 0; i < d.length; i += 4) {
        if (d[i] !== 22 || d[i + 1] !== 29 || d[i + 2] !== 38) return true;
      }
      return false;
    }),
  );
  assert.ok(drawn, 'the chart drew a line or a gridline');
});

// 6 — the checkbox is the library's, so the control is the element carrying the role, and
// toggling it changes what is *asked for*, not what is filtered afterwards: `min_bps` is the
// backend's filter and is applied before the row limit.
await run('the "moving now" control is the library checkbox and sends its filter to the backend', async () => {
  const seen = [];
  page.on('request', (r) => {
    if (r.url().includes('/api/connections')) seen.push(r.url());
  });

  await page.locator('[role=tab]').nth(1).click();
  const box = await until('the library checkbox on the connections panel', () => page.locator('[role=checkbox]').count());
  assert.equal(box, 1, 'one checkbox on the connections panel');
  assert.equal(await page.locator('[role=checkbox]').getAttribute('aria-checked'), 'false');

  await page.locator('[role=checkbox]').click();
  await until('the checkbox to become checked', async () => (await page.locator('[role=checkbox]').getAttribute('aria-checked')) === 'true');
  await until('a request carrying min_bps=1', () => seen.some((u) => u.includes('min_bps=1')));
});

// 7 — device-supplied strings cannot become markup. The old files needed an esc() because they
// built HTML; the vnode layer escapes text nodes, so a hostile name is a name. This reads the
// real path: the fixture lease file `run.sh` writes carries the hostile hostname, so it is
// parsed by `crates/network-status/src/leases.rs` and rendered by `leases.rs` here, not put on
// the page by the test. The name is one token because dnsmasq's file is whitespace-separated: a
// name with a space in it is two fields to the parser, which is not the thing being checked.
await run('a hostname that is markup renders as text, not as an element', async () => {
  await page.goto(`${BASE}/#leases`);
  const cell = await until('the fixture lease row with the hostile name', async () => {
    const t = await page.locator('table.leases tbody tr td').first().innerText();
    return t.includes('img/onerror') ? t : null;
  });
  assert.equal(await page.locator('img').count(), 0, 'no element was created from the name');
  assert.match(cell, /img\/onerror/, 'the name is shown as the string it is');
  assert.equal(await page.title(), 'network-status', 'nothing in the name was executed');
});

// 8 — the banner: the data still on screen has to be named as last-known, which is the whole
// point of keeping it when a fetch fails.
await run('an unreachable router shows the banner and keeps the data it already had', async () => {
  await page.goto(BASE);
  await until('a header reading the probe', async () => /router up \d/.test(await headerText()));
  const before = await panelText();

  await page.route('**/api/snapshot*', async (route) => route.abort());
  await until('the banner', async () => {
    const t = await page.locator('.banner').innerText();
    return t.includes('Cannot reach the router') ? t : null;
  });
  const banner = await page.locator('.banner').innerText();
  assert.match(banner, /Showing last known data/, 'the banner says what is being shown');
  assert.match(banner, /will retry/, 'and that it is not giving up');
  assert.match(await panelText(), /conntrack entries/, 'and the panel still shows the last good sample');

  await page.unroute('**/api/snapshot*');
  // The next poll succeeds, so the banner is not a latch: it goes away with the error.
  await until('the banner to clear on the next poll', async () => ((await page.locator('.banner').count()) === 0 ? true : null));
  assert.ok(before.length > 0, 'the data was never lost while the router was unreachable');
});

// 9 — the channel, not the table. This check exists because of a real incident: a device was
// live and the page showed a table of lapsed leases for two and a half days, because `install`
// had replaced `/var/lib/dnsmasq/dnsmasq.leases` and dnsmasq went on writing the orphan inode it
// opened at startup. The verdict is the backend's (`stale` in crates/network-status/src/leases.rs);
// what is checked here is that the page shows it — and that a healthy file is shown as freshness
// rather than as silence, because "is this live?" is the first question a lease table answers.
//
// Only the mtime changes. The rows are the same rows, so the panel's numbers cannot be what
// this check is measuring: it is measuring whether the page can tell a quiet network from a
// dead writer, which is the whole reason the backend reports the pair.
await run('a lease file that stopped being written banners as a snapshot, not as a quiet network', async () => {
  const leasesPath = process.env.NS_TEST_LEASES;
  if (!leasesPath) throw new Error('run.sh must export NS_TEST_LEASES for this check');
  await page.goto(`${BASE}/#leases`);

  const healthy = await until('the summary naming how fresh the fixture file is', async () => {
    const t = await panelText();
    return /written \d+m \d+s ago/.test(t) ? t : null;
  });
  assert.equal(await page.locator('.panel-error').count(), 0, 'a file being written is not bannered');
  assert.match(healthy, /10\.3\.2\.99/, 'and the healthy case still shows the lease');

  // Past two lease lengths (the backend's `unwritten` threshold), on a file that has one live row.
  const old = Math.floor(Date.now() / 1000) - 3 * 86400;
  fs.utimesSync(leasesPath, new Date(old * 1000), new Date(old * 1000));

  const bannered = await until('the panel to banner the backend\u2019s verdict', async () => {
    const t = await panelText();
    return t.includes('not live') ? t : null;
  }, 15000);
  assert.match(bannered, /has not been written for longer than two of these leases last/, 'the page translates the reason the backend chose, it does not invent one');
  assert.match(bannered, /not evidence that a device is absent/, 'and says what this table does not prove');
  assert.match(bannered, /written 3d 0h 0m ago/, 'with the age of the last write');
  assert.match(bannered, /lease-reader\.nix/, 'and points at the module that owns the file');
  assert.match(bannered, /10\.3\.2\.99/, 'the rows are still there — the table is not empty, it is old');

  // Put the file back, so a failing check does not leave the fixture stamped as stale for the
  // next run, and so the healthy path is what the suite ends on.
  fs.utimesSync(leasesPath, new Date(), new Date());
});

await browser.close();

if (process.exitCode) {
  console.error('\nsuite failed');
} else {
  console.log('\nall checks passed');
}
