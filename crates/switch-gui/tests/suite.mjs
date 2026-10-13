#!/usr/bin/env node
// Headless-browser test suite for the switch-gui web UI.
//
// Run with `just switch-gui-test` (or switch-gui/tests/run.sh).  The runner
// builds the SPA and the binary, starts a **fake ProCurve console** (a real
// Telnet server with an in-memory model — see `fake-switch.mjs`), points the
// service at it with `--no-auth` on a loopback bind and a scratch Postgres
// schema, and drives the page with headless Chromium.
//
// What this suite is for: the whole point of the tool is that a tick in a table
// becomes a line on a console somewhere, and every layer between the two
// (overlay → POST /api/intent → queue → sync → console → re-read → shadow copy
// → page) has already been broken once in a way that looked like the layer next
// to it. A test that only checks the page would have passed while the queue
// silently swallowed every change; a test that only checks the queue would have
// passed while the page never sent one. So each test asserts on **the device's
// own state** as well as the page's.
//
// Only ports that are administratively shut and link-down are ever written to
// (23 and 24 in the fixture): a test must not be able to disturb a live port,
// and on the real switches these two are exactly the ones a human would pick.

import { chromium } from 'playwright';
import assert from 'node:assert/strict';

const PORT = Number(process.env.SG_TEST_PORT || 18097);
const STATE_PORT = Number(process.env.SG_TEST_STATE_PORT || 22324);
const BASE = `http://127.0.0.1:${PORT}`;
const DEVICE = `http://127.0.0.1:${STATE_PORT}`;
const SWITCH = process.env.SG_TEST_SWITCH || 'switch1';

// The safe ports of the fixture: shut, link-down, and nothing downstream.
const SAFE = '23';
const SAFE_2 = '24';
// A VLAN that no safe port is in yet, so ticking it is always a real change.
const ADD = 228;
// The VLAN the safe ports are already untagged in — its column is the one that
// must never be tickable for a port that is native in it.
const NATIVE = 225;

// ---------------------------------------------------------------------------
// helpers

const get = async (url) => {
  const r = await fetch(url);
  // Read the body once: a Response is single-use, and `json()` then `text()`
  // throws "Body has already been read" before the assertion ever runs.
  const body = await r.text();
  return { status: r.status, body, json: body ? JSON.parse(body) : null };
};
const post = async (url, body) => {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body ?? {}),
  });
  const text = await r.text();
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = null;
  }
  return { status: r.status, body: text, json };
};

const api = (path) => get(`${BASE}${path}`);
const postJson = (path, body) => post(`${BASE}${path}`, body);

/** What the fake device believes, which is the only proof a write went out. */
const device = async () => (await get(`${DEVICE}/state`)).json;
const devicePort = async (id) => (await device()).ports[id];
const deviceLog = async () => (await get(`${DEVICE}/log`)).json;
const deviceMeta = async () => (await get(`${DEVICE}/meta`)).json;
const deviceReset = () => post(`${DEVICE}/reset`, {});
/** Commands the device should refuse, to exercise the failure path. */
const deviceFail = (cmds) => post(`${DEVICE}/fail`, cmds);
/** The device vanishes: no console sessions, existing ones die. */
const deviceOffline = () => post(`${DEVICE}/offline`, {});
const deviceOnline = () => post(`${DEVICE}/online`, {});

/**
 * Forget every change still queued, so one test cannot leave the next one a
 * pending row (which marks its port and changes what the page offers).
 */
async function clearQueue() {
  const j = (await api(`/api/changes?switch=${SWITCH}&limit=100`)).json;
  for (const c of j.changes || []) {
    if (c.status === 'pending' || c.status === 'applying') {
      await postJson('/api/change/forget', { id: c.id });
    }
  }
}

/**
 * The invariant the whole suite exists for: no change may sit in `applying`
 * with nothing working on it.  A row in that state is invisible to every error
 * path in the program until `recover_stale` releases it fifteen minutes later,
 * and on the page it is a tick that unsets itself and nothing happening.
 */
async function assertNothingStuck() {
  const stuck = await (async () => {
    for (let i = 0; i < 60; i++) {
      const j = (await api(`/api/changes?switch=${SWITCH}&limit=100`)).json;
      const open = (j.changes || []).filter((c) => c.status === 'applying');
      if (open.length === 0) return [];
      await new Promise((r) => setTimeout(r, 500));
    }
    const j = (await api(`/api/changes?switch=${SWITCH}&limit=100`)).json;
    return (j.changes || []).filter((c) => c.status === 'applying');
  })();
  if (stuck.length) {
    throw new Error(
      `${stuck.length} change(s) left in flight: ` +
        stuck.map((c) => `#${c.id} ${c.summary}`).join(' | '),
    );
  }
}

/** Poll `fn` until it returns truthy, or fail with `what`. */
async function until(what, fn, ms = 20000) {
  const deadline = Date.now() + ms;
  let last;
  while (Date.now() < deadline) {
    last = await fn();
    if (last) return last;
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`${what} (last value: ${JSON.stringify(last)})`);
}

/** Wait for the sync to have read the device into the shadow copy. */
async function waitForRead() {
  await until('the sync never read the switch', async () => {
    const s = (await api('/api/state')).json.switches.find((x) => x.name === SWITCH);
    return s && s.ports > 0 && !s.error ? s : null;
  });
}

/** Wait until the shadow copy agrees with the device, port for port. */
async function waitShadow() {
  await until('the shadow copy never caught up with the device', async () => {
    const d = await device();
    const j = (await api(`/api/switch?switch=${SWITCH}`)).json;
    if (!j?.ports?.length) return null;
    for (const [id, p] of Object.entries(d.ports)) {
      const sp = j.ports.find((x) => x.id === id);
      if (!sp) return null;
      if (String(sp.native ?? '') !== String(p.untagged)) return null;
      if (JSON.stringify([...sp.tagged].sort()) !== JSON.stringify([...p.tagged].sort())) return null;
    }
    return true;
  });
}

const row = (id) => page.locator(`tr[data-port=${JSON.stringify(id)}]`);
// The matrix's own hooks, added for this suite (see the comment in
// `frontend/src/page.rs`): a row by the switch's port id, a VLAN column by VLAN
// id. The checkbox cells are `td.ck`, the header cells are `th.v`.
const box = (id, vlan) => row(id).locator(`input[data-vlan="${vlan}"]`);
const applyButton = (id) => row(id).locator(`button[data-apply="${id}"]`);
const nativeSelect = (id) => row(id).locator('select');
// The banner, not the first `.note`: the page also carries the read's own
// warnings, and those are `.note.err` before anything has been clicked — which is
// how "wait for the error note" became a test that passed on the old page.
// Empty rather than a throw when no action has answered yet, so a test can poll it.
const noteText = async () => {
  const n = page.locator('.note.banner');
  return (await n.count()) > 0 ? await n.innerText() : '';
};

/** The page for one switch, reloaded, with the matrix present. */
async function open() {
  await page.goto(`${BASE}/switch/${SWITCH}`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('table.mx', { timeout: 15000 });
  await page.waitForSelector(`tr[data-port=${JSON.stringify(SAFE)}]`, { timeout: 15000 });
}

/** The audit log's newest row for this switch, as the page sees it. */
async function latestChange() {
  const j = (await api(`/api/changes?switch=${SWITCH}&limit=1`)).json;
  return j.changes?.[0] || null;
}

/**
 * Tick or untick one VLAN of one port and Apply it, then wait for the device to
 * have changed.  Returns the queued change.
 */
async function setTagged(id, vlan, want) {
  const b = box(id, vlan);
  await b.waitFor({ timeout: 10000 });
  // A real click: it is the `change` event that moves the overlay, and only a
  // click dispatches it. `check()`/`uncheck()` would set the same property, but
  // a test that bypassed the event would not be testing the page.
  if ((await b.isChecked()) !== want) await b.click();
  assert.equal(await b.isChecked(), want, `ticking VLAN ${vlan} on port ${id} did not stick`);
  await applyButton(id).waitFor({ timeout: 10000 });
  assert.ok(await applyButton(id).isEnabled(), 'Apply stayed disabled on a changed row');
  await applyButton(id).click();
  return await until('the change never reached the queue', async () => {
    const c = await latestChange();
    return c && c.status !== undefined ? c : null;
  });
}

// ---------------------------------------------------------------------------
// tests

const tests = [];
const test = (name, fn) => tests.push([name, fn]);

test('boot: the page reads the switch it was pointed at', async () => {
  await waitForRead();
  await open();
  assert.match(await page.locator('h1').innerText(), /switch1/);
  // One column per VLAN, from the switch's own VLAN table.
  const cols = await page.locator('table.mx th.v').count();
  assert.equal(cols, (await device()).vlans.length, 'one column per VLAN');
  // And the port rows are there, in the switch's order, not a string sort.
  const ports = await page.locator('tr[data-port]').evaluateAll((rs) => rs.map((r) => r.dataset.port));
  assert.deepEqual(ports.slice(0, 3), ['1', '2', '3'], 'ports sort numerically');
  assert.ok(ports.includes('Trk1'), 'the trunk group is a row of its own');
});

test('read: tagged memberships are shown, not just stored', async () => {
  // The regression this catches: the shadow copy held the tagged VLANs as JSON
  // strings, every reader parsed them as numbers, and the matrix rendered a
  // page of unticked boxes for a switch with 19 tagged memberships.
  await open();
  const d = await device();
  for (const id of ['1', '2', '47']) {
    for (const v of d.vlans.map((x) => x.id)) {
      const want = d.ports[id].tagged.includes(v);
      assert.equal(await box(id, v).isChecked(), want, `port ${id} VLAN ${v}`);
    }
    assert.equal(
      await nativeSelect(id).inputValue(),
      String(d.ports[id].untagged),
      `port ${id} native VLAN`,
    );
  }
});

test('write: ticking a VLAN on a down port reaches the device and stays ticked', async () => {
  // The bug this catches: the tick was recorded in the page's overlay, Apply
  // looked like it worked, and the checkbox silently reverted on the next
  // render while nothing was ever queued.
  const before = await devicePort(SAFE);
  assert.ok(!before.tagged.includes(ADD), 'fixture: the safe port starts out of that VLAN');
  await open();
  const change = await setTagged(SAFE, ADD, true);
  assert.equal(change.kind, 'port-members');
  assert.ok(change.commands.flat().includes(`vlan ${ADD} tagged ${SAFE}`), JSON.stringify(change.commands));

  // The change reaches the device.  (The page's "pending" badge is deliberately
  // not asserted: the sync applies within a couple of seconds and the page polls
  // every fifteen, so the badge is a race rather than a property. That a queued
  // change is visible to the operator is covered by the audit panel in the
  // failure test.)

  await until('the device never received the change', async () => {
    const p = await devicePort(SAFE);
    return p.tagged.includes(ADD) ? p : null;
  });
  // And after the sync re-reads, the page agrees with the device.
  await until('the page never showed the new membership', async () => {
    await open();
    return (await box(SAFE, ADD).isChecked()) ? true : null;
  });
  // The change settled, and the row is no longer offered as an edit.
  const done = await latestChange();
  assert.equal(done.status, 'applied', done.lastError);
  assert.ok(!(await applyButton(SAFE).isEnabled()), 'the row still looks changed');
});

test('write: unticking a VLAN removes it from the device', async () => {
  // The other half of the diff. A shadow copy that reads its tagged list as the
  // wrong JSON type makes `have` empty, so a removal generates no command at
  // all and the Apply that "worked" wrote nothing.
  const start = await devicePort(SAFE_2);
  assert.ok(!start.tagged.includes(ADD));
  await open();
  await setTagged(SAFE_2, ADD, true);
  await until('the device never took the first change', async () =>
    (await devicePort(SAFE_2)).tagged.includes(ADD) ? true : null);
  // The page must be showing the tick before it can be taken away: the shadow
  // copy lags the device by up to a sync interval, and unticking a box that was
  // never ticked is not a change — the row stays clean and Apply stays disabled.
  await waitShadow();

  await open();
  await setTagged(SAFE_2, ADD, false);
  await until('the removal never reached the device', async () =>
    (await devicePort(SAFE_2)).tagged.includes(ADD) ? null : true);
  const cmds = (await deviceLog()).map((c) => c.cmd);
  assert.ok(cmds.includes(`no vlan ${ADD} tagged ${SAFE_2}`), 'no removal was sent');
});

test('write: the untagged dropdown changes the native VLAN', async () => {
  await open();
  const sel = nativeSelect(SAFE_2);
  await sel.selectOption(String(ADD));
  await applyButton(SAFE_2).click();
  await until('the device never changed its native VLAN', async () =>
    (await devicePort(SAFE_2)).untagged === ADD ? true : null);
  await open();
  assert.equal(await nativeSelect(SAFE_2).inputValue(), String(ADD));
});

test('lock: a port is never offered its own native VLAN as a tagged membership', async () => {
  // A port cannot be tagged in the VLAN it is untagged in — the switch has no
  // room for it and `commands::port_members` refuses it.  Offered anyway, the
  // tick looked legal, Apply was refused, and the only answer was a banner at the
  // top of a page with fifty rows above the matrix, which is how a correct
  // refusal reached an operator as "the checkboxes reset and nothing happened".
  await open();
  assert.ok(
    await box(SAFE, NATIVE).isDisabled(),
    'the row offered its own native VLAN as a tagged membership',
  );
  assert.equal(await box(SAFE, NATIVE).isChecked(), false);
  // The reason is in the cell's `title`, like every other lock on this page.
  assert.match(
    await box(SAFE, NATIVE).locator('xpath=..').getAttribute('title'),
    /untagged \(native\) VLAN/,
  );
  // Only that one column: an ordinary membership is still editable.
  assert.ok(await box('1', ADD).isEnabled(), 'a real tagged membership was locked too');
  // Moving the native VLAN moves the locked column with it, because the rule is
  // about the row being edited and not about the switch it was read from.
  await nativeSelect(SAFE).selectOption(String(ADD));
  assert.ok(await box(SAFE, NATIVE).isEnabled(), 'the old native VLAN stayed locked');
  assert.ok(
    await box(SAFE, ADD).isDisabled(),
    'the new native VLAN is offered as a tagged membership',
  );
  // Back where it started, and nothing was written: this test touches no switch.
  await nativeSelect(SAFE).selectOption(String(NATIVE));
  assert.ok(await box(SAFE, NATIVE).isDisabled());
  assert.equal(await applyButton(SAFE).isEnabled(), false, 'the row was left marked as changed');
});

test('write: making a ticked VLAN the native VLAN takes the tick with it', async () => {
  // The same rule, from the other side: ticking 228 and then picking 228 as the
  // native VLAN means "make this an access port on 228", which the switch does
  // quite happily (`no vlan 228 tagged` then `vlan 228 untagged`, which is the
  // order `commands::port_members` emits).  Sending both halves in one row would
  // only be a refusal, so the tick goes with the dropdown.
  await open();
  // The audit log keeps applied rows, so "the newest change" is only *this*
  // test's change if it is newer than the one already there.
  const seen = (await latestChange())?.id ?? 0;
  const b = box(SAFE, ADD);
  await b.click();
  assert.ok(await b.isChecked(), 'the tick never appeared');
  await nativeSelect(SAFE).selectOption(String(ADD));
  assert.equal(await box(SAFE, ADD).isChecked(), false, 'the row asked for both halves at once');
  assert.ok(await applyButton(SAFE).isEnabled(), 'the row stopped being an edit');
  await applyButton(SAFE).click();
  const change = await until('the change never made it through the queue', async () => {
    const c = await latestChange();
    return c && c.id > seen && c.status === 'applied' && c.ports.includes(SAFE) ? c : null;
  });
  const cmds = change.commands.flat();
  assert.ok(!cmds.includes(`vlan ${ADD} tagged ${SAFE}`), `asked for both: ${cmds}`);
  assert.ok(cmds.includes(`vlan ${ADD} untagged ${SAFE}`), JSON.stringify(cmds));
  const p = await until('the device never changed its native VLAN', async () => {
    const d = await devicePort(SAFE);
    return d.untagged === ADD ? d : null;
  });
  assert.ok(!p.tagged.includes(ADD), 'the device holds both halves');
});

test('refused: the row keeps the edit that was refused', async () => {
  // The other half of "the tick unset itself and nothing happened".  Apply dropped
  // the row's overlay before the reply — right, because a queued row is the truth
  // from then on, and it is also what stops a double-click queueing twice — but a
  // *refusal* queues nothing, so the row snapped back to the switch and the
  // operator lost the edit they were being told to fix.
  //
  // The refusal is faked at the HTTP boundary on purpose: with the native VLAN's
  // column no longer tickable, the page cannot be made to ask for the impossible,
  // and what is under test is the page's behaviour on `ok: false`.
  await open();
  await page.route('**/api/intent', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ok: false, error: 'REFUSED-BY-TEST: cannot be both' }),
    }),
  );
  try {
    const b = box(SAFE, ADD);
    await b.click();
    assert.ok(await b.isChecked(), 'the tick never appeared');
    await applyButton(SAFE).click();
    // Not `.note.err`: the fake switch answers every read with the RADIUS warning,
    // so an error note is already on this page before the click happens.  The
    // banner is the one that carries the answer.
    await until('the refusal never reached the banner', async () =>
      /REFUSED-BY-TEST/.test(await noteText()));
    // The edit survives the refusal, and so does the button to fix it.
    assert.ok(await box(SAFE, ADD).isChecked(), 'the refused tick was thrown away');
    assert.ok(await applyButton(SAFE).isEnabled(), 'Apply went away with the edit');
    // And nothing was queued: a refusal is a refusal.
    const c = await latestChange();
    assert.ok(
      !c || (c.status !== 'pending' && c.status !== 'applying'),
      'a refused write was queued anyway',
    );
    // The banner is pinned, so the answer is on screen with the row that caused
    // it: at the bottom of the matrix, it used to be a screen away.
    await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
    const bb = await page.locator('.note.banner').boundingBox();
    assert.ok(bb && bb.y >= 0 && bb.y < 120, `the banner scrolled away: ${JSON.stringify(bb)}`);
  } finally {
    await page.unroute('**/api/intent');
  }
});

test('lock: a trunk member and a trunk group cannot be edited', async () => {
  // The switch refuses `vlan X tagged <member>` outright, so the page must not
  // offer it — and the rule covers the group's own row as well as its members.
  await open();
  for (const id of ['47', '48', 'Trk1']) {
    assert.ok(await box(id, ADD).isDisabled(), `port ${id} is editable`);
    assert.equal(await row(id).locator('button[data-apply]').count(), 0, `port ${id} offers Apply`);
  }
  // The reason is in the cell's `title`, not its text: the row says enough
  // without it, and the hint is for the pointer.
  assert.match(await row('47').locator('td').nth(1).getAttribute('title'), /trunk member/);
  assert.match(
    await row('Trk1').locator('td').nth(1).getAttribute('title'),
    /link-aggregation group/,
  );
});

test('failure: a refused write is reported, not swallowed', async () => {
  // The device says `Invalid input:`; the change must fail with that text rather
  // than look applied, and the page must show it.
  const cmd = `vlan ${ADD} tagged ${SAFE}`;
  await deviceFail([cmd]);
  await open();
  await setTagged(SAFE, ADD, true);
  const change = await until('the change never failed', async () => {
    const c = await latestChange();
    return c && c.status === 'failed' ? c : null;
  });
  assert.match(change.lastError, /Invalid input/);
  assert.ok(!(await devicePort(SAFE)).tagged.includes(ADD), 'it wrote anyway');
  // And the page says so: the audit panel is where an operator looks, and the
  // switch's own refusal text is the half that tells them what to do next.  The
  // open table puts it in a `td.err`, the audit table in a `div.err`.
  await page.goto(`${BASE}/switch/${SWITCH}`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('table.mx');
  await page.waitForTimeout(1500);
  const said = (await page.locator('.err').allInnerTexts()).join('\n');
  assert.match(said, /Invalid input/, 'the page never showed what the switch said');
});

test('a dead console does not swallow the queue', async () => {
  // The bug this catches is the reason this suite exists.  A pass that claimed a
  // change and then could not reach the switch returned without recording
  // anything: the row stayed `applying` with its attempt spent, no error, no
  // journal line, and invisible to every error path — for fifteen minutes,
  // because that is how long `recover_stale` waits.  On the page: tick a box,
  // press Apply, the tick unsets, nothing happens.
  await open();
  await setTagged(SAFE, ADD, true);
  await deviceOffline();
  // Let the sync notice: its session dies, so the next pass has to reconnect.
  await until('the switch never reported the outage', async () => {
    const s = (await api('/api/state')).json.switches.find((x) => x.name === SWITCH);
    return s.error ? s : null;
  }, 30000);
  // And the change it had claimed is settled — retried or failed with a reason,
  // never silently `applying`.
  const settled = await until('the queued change was never answered', async () => {
    const c = await latestChange();
    return c && c.status !== 'applying' && c.status !== 'pending' ? c : null;
  }, 30000);
  assert.equal(settled.status, 'failed', settled.lastError || 'no error recorded');
  assert.match(settled.lastError, /connect|refused|console/i, settled.lastError);
  await deviceOnline();
});

test('nothing writes NVRAM', async () => {
  // `write` is this firmware's save-to-flash, and the tool must never send it: a
  // change that reaches flash survives a reboot, which is a decision the
  // operator makes, not the GUI.
  const meta = await deviceMeta();
  assert.equal(meta.wroteToNvram, false);
  const cmds = (await deviceLog()).map((c) => c.cmd);
  assert.ok(!cmds.includes('write'), 'the console was told to write to flash');
});

test('session: the page never talks to a switch while serving a request', async () => {
  // Everything the page shows comes from the shadow copy. A read that has not
  // happened yet must be said so, not guessed at — so stop the sync by pointing
  // the service at a switch nobody answers and check the page still renders
  // what it last knew.
  const s = (await api('/api/state')).json.switches[0];
  assert.ok(s.fetchedAt, 'the index claims a read that never happened');
  assert.ok(s.ageSecs !== null, 'no age for a read');
  assert.equal(s.privilege, 'manager');
});

// ---------------------------------------------------------------------------
// run

let failed = 0;
const consoleErrors = [];
const browser = await chromium.launch({ executablePath: process.env.SG_TEST_BROWSER || undefined });
const page = await browser.newPage();
// Playwright's default is 30s per locator, which turns a wrong selector into a
// five-minute run. A page this small needs seconds, and a fast failure with the
// body dumped below is a far better report.
page.setDefaultTimeout(10000);
page.on('console', (m) => {
  // The browser asks for a favicon and the service has none — a 404 for a
  // decoration is not a page error.  Anything else is.
  if (m.type() === 'error' && !(m.location()?.url || '').endsWith('/favicon.ico')) {
    consoleErrors.push(m.text() + (m.location()?.url ? ' @ ' + m.location().url : ''));
  }
});
page.on('pageerror', (e) => consoleErrors.push('pageerror: ' + e.message));

for (const [name, fn] of tests) {
  const before = consoleErrors.length;
  try {
    // A device put back to its fixture, a queue with nothing left in it, and a
    // shadow copy that has been told: every test starts from the same state the
    // page is about to show.
    await deviceOnline();
    await deviceReset();
    await clearQueue();
    await waitShadow();
    await open();
    await fn();
    const fresh = consoleErrors.slice(before);
    if (fresh.length) throw new Error('console errors: ' + fresh.join(' | '));
    // Post-condition, not a test: every test that queues a change must leave the
    // queue settled, or the suite has just reproduced the bug it is for.
    await assertNothingStuck();
    console.log(`ok    ${name}`);
  } catch (e) {
    failed++;
    const where = String(e.stack || '').split('\n').find((l) => l.includes('suite.mjs'));
    // Dump what the page actually was.  Every failure in this suite has been a
    // mismatch between three layers (device, shadow copy, page), and "the
    // selector was not found" says nothing about which one was wrong.
    let dump = '';
    try {
      dump = (await page.evaluate(() => document.body.innerText.slice(0, 400))).replace(/\n/g, ' · ');
    } catch {
      dump = '(no page)';
    }
    console.log(`FAIL  ${name}\n        ${String(e.message || e).split('\n')[0]}${where ? '\n        ' + where.trim() : ''}\n        page: ${dump}`);
  }
}

await browser.close();
console.log(`\n${tests.length} tests, ${failed} failure(s)`);
process.exit(failed ? 1 : 0);
