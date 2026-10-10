#!/usr/bin/env node
// Headless-browser test suite for the filestore web UI.
//
// Run with `just filestore-test` (or filestore/tests/run.sh).  The runner
// builds the binary, starts it with --no-auth against a scratch fixture, and
// drives it with headless Chromium.  Every test starts from a freshly rebuilt
// fixture and a fresh page load, so tests are order-independent.
//
// Python is deliberately not used here: Playwright is the only practical
// headless-browser driver, and its Rust bindings are not usable offline in
// this repo.

import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';

const PORT = Number(process.env.FS_TEST_PORT || 18097);
const ROOT = process.env.FS_TEST_ROOT || '/tmp/filestore-test-root';
// Scratch directory handed down by run.sh (logs, throwaway fixtures).  Falls
// back to a fresh one so `node suite.mjs` on its own still works.
const WORKDIR = process.env.FS_TEST_WORKDIR || fs.mkdtempSync(path.join(os.tmpdir(), 'filestore-suite.'));
// The thumbnail cache directory run.sh points the server at, so the cache tests can
// look inside it.
const THUMB_CACHE = process.env.FS_TEST_THUMB_CACHE || '';
const BASE = `http://127.0.0.1:${PORT}`;

// ---------------------------------------------------------------------------
// fixture

const PNG_1PX = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64',
);

// A 1 s silent PCM WAV: real enough that Chromium decodes it, so an audio test
// can check the element actually loaded metadata rather than just existing.
function wav(seconds = 1, rate = 8000) {
  const n = seconds * rate;
  const h = Buffer.alloc(44);
  h.write('RIFF', 0);
  h.writeUInt32LE(36 + n * 2, 4);
  h.write('WAVE', 8);
  h.write('fmt ', 12);
  h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20); // PCM
  h.writeUInt16LE(1, 22);
  h.writeUInt32LE(rate, 24);
  h.writeUInt32LE(rate * 2, 28);
  h.writeUInt16LE(2, 32);
  h.writeUInt16LE(16, 34);
  h.write('data', 36);
  h.writeUInt32LE(n * 2, 40);
  return Buffer.concat([h, Buffer.alloc(n * 2)]);
}

// A one-page PDF with a correct xref.  Headless Chromium does not draw it, but a
// valid file is what makes "the browser's viewer gets a real PDF" testable.
function pdf() {
  const objs = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>',
  ];
  let out = '%PDF-1.4\n';
  const offs = [];
  objs.forEach((o, i) => {
    offs.push(out.length);
    out += `${i + 1} 0 obj ${o} endobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n`;
  for (const o of offs) out += `${String(o).padStart(10, '0')} 00000 n \n`;
  out += `trailer << /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return Buffer.from(out, 'latin1');
}

// A real BMP of w x h.  The thumbnail tests need a decodable image big enough to
// show that the cache resizes, and BMP is the one format that can be written here
// without a compressor.
function bmp(w, h, rgb = [10, 128, 240]) {
  const rowLen = ((w * 3 + 3) & ~3);
  const row = Buffer.alloc(rowLen, 0);
  for (let x = 0; x < w; x++) {
    row[x * 3] = rgb[2]; // BMP stores BGR
    row[x * 3 + 1] = rgb[1];
    row[x * 3 + 2] = rgb[0];
  }
  const pixels = Buffer.concat(Array(h).fill(row));
  const head = Buffer.alloc(54, 0);
  head.write('BM', 0);
  head.writeUInt32LE(54 + pixels.length, 2);
  head.writeUInt32LE(54, 10);
  head.writeUInt32LE(40, 14);
  head.writeInt32LE(w, 18);
  head.writeInt32LE(h, 22);
  head.writeUInt16LE(1, 26);
  head.writeUInt16LE(24, 28);
  head.writeUInt32LE(pixels.length, 34);
  return Buffer.concat([head, pixels]);
}

// value null => directory
const FIXTURE = {
  'notes.txt': 'hello world\nsecond line\n',
  'data.json': '{"a": 1, "b": [1,2,3]}\n',
  'table.csv': 'name,value\na,1\nb,2\n',
  'pandas.csv': 'a,b\n1,2\n',
  'a b/x.txt': 'in the spaced folder\n',
  'binary.bin': Buffer.from([1, 2, 3, 0, 4]),
  'drawing.svg': '<svg xmlns="http://www.w3.org/2000/svg"><circle r="10"/></svg>',
  'Makefile': 'all:\n\techo hi\n',
  'big-truncated.txt': 'line\n'.repeat(200000),
  'move-me.txt': 'move me\n',
  'docs/readme.md': '# Title\n\nSome **markdown**.\n',
  'docs/notes.txt': 'docs copy\n',
  'photos/2024/cat.png': PNG_1PX,
  'photos/2024/dog.png': PNG_1PX,
  // Thumbnail fixtures.  A real BMP big enough that resizing is observable (the 1px
  // PNG cannot show it).  The undecodable image goes in its own folder so the only
  // test that renders it is the fallback test — a refused thumbnail is a failed
  // request, and the runner treats console errors as failures.
  'photos/big.bmp': bmp(256, 128),
  'thumbfail/broken.png': Buffer.from('this is not an image'),
  // Not a decodable video: the point of the video tests is which element the UI
  // picks and what the server sends, not that Chromium can play it.
  'media/clip.mp4': Buffer.from([0, 0, 0, 24, 0x66, 0x74, 0x79, 0x70]),
  'media/sound.wav': wav(),
  'docs/report.pdf': pdf(),
  'sub/a.txt': 'nested file\n',
  'sub/deep/deeper/needle-found-me.txt': 'x'.repeat(100),
  'weird dir with spaces/a file.txt': 'inside weird dir\n',
  'écho-η/ünïcode.txt': 'unicode name\n',
  'empty-dir': null,
  scratch: null,
};

function buildFixture() {
  fs.rmSync(ROOT, { recursive: true, force: true });
  for (const [rel, val] of Object.entries(FIXTURE)) {
    const p = path.join(ROOT, rel);
    if (val === null) fs.mkdirSync(p, { recursive: true });
    else {
      fs.mkdirSync(path.dirname(p), { recursive: true });
      fs.writeFileSync(p, val);
    }
  }
}

function runServer(args) {
  const r = spawnSync(process.env.FS_TEST_BIN || 'filestore', args, { encoding: 'utf8', timeout: 15000 });
  return { code: r.status, stderr: r.stderr || '', stdout: r.stdout || '', pid: r.pid };
}

async function startServer(args, port) {
  const p = spawn(process.env.FS_TEST_BIN || 'filestore', args, { stdio: 'ignore' });
  for (let i = 0; i < 60; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/api/whoami`);
      if (r.status !== 0) return p;
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 250));
  }
  p.kill();
  throw new Error(`server on ${port} did not start`);
}

const abs = (rel) => path.join(ROOT, rel);
const exists = (rel) => fs.existsSync(abs(rel));
const read = (rel) => fs.readFileSync(abs(rel), 'utf8');

// ---------------------------------------------------------------------------
// helpers

const consoleErrors = [];
let browser, page;

async function reset() {
  consoleErrors.length = 0;
  buildFixture();
  await page.goto(BASE + '/');
  await waitLoaded();
}

async function waitLoaded() {
  await page.waitForFunction(() => {
    const c = document.getElementById('fs-content');
    return !!c && !c.innerText.startsWith('loading');
  }, null, { timeout: 15000 });
}

const sel = (name) => `[data-fs-name=${JSON.stringify(name)}]`;
const row = (name) => page.locator(sel(name));
const rowNames = () =>
  page.evaluate(() => [...document.querySelectorAll('[data-fs-name]')].map((e) => e.getAttribute('data-fs-name')));
const selectedNames = () =>
  page.evaluate(() =>
    [...document.querySelectorAll('[data-fs-name]')]
      .filter((e) => e.getAttribute('data-fs-sel') === '1')
      .map((e) => e.getAttribute('data-fs-name')),
  );
const statusText = () => page.evaluate(() => document.getElementById('fs-status').innerText);
const toastTexts = () =>
  page.evaluate(() => [...document.querySelectorAll('#fs-toasts > div')].map((e) => e.innerText.trim()));
const menuItems = () =>
  page.evaluate(() => [...document.querySelectorAll('#fs-menu .fs-menu-item')].map((e) => e.innerText.trim()));
const menuItem = (label) => page.locator('#fs-menu .fs-menu-item', { hasText: label });
const modalInput = () => page.locator('#fs-modal-input');
const modalButton = (label) => page.locator('#fs-modal button', { hasText: label });

async function clickButton(label) {
  await page.locator('#root button', { hasText: label }).first().click();
}

async function waitToast(match) {
  await page.waitForFunction(
    (m) => [...document.querySelectorAll('#fs-toasts > div')].some((e) => e.innerText.includes(m)),
    match,
    { timeout: 8000 },
  );
  return toastTexts();
}

async function openRow(name) {
  await row(name).dblclick();
  await waitLoaded();
}

// The preview overlay covers the app, so it has to be closed before the next
// click lands on a row.
async function closePreview() {
  await page.getByRole('button', { name: '✕' }).click();
  await page.waitForFunction(() => !document.getElementById('fs-preview'), null, { timeout: 8000 });
}

// Rows are not focusable, so Enter goes to whatever control still has focus.
// After clicking a toolbar button (or the preview's ✕) that control is the
// focused one, and Enter must activate it rather than open a row — so the row
// tests have to clear focus first.
async function blur() {
  await page.evaluate(() => document.activeElement.blur());
}

// In search mode the toolbar is replaced by the search bar, so the shallow/deep
// toggle lives inside #root as well.

async function api(p) {
  const r = await fetch(BASE + p);
  return { status: r.status, body: await r.text(), headers: Object.fromEntries(r.headers) };
}
async function apiJson(p) {
  const r = await fetch(BASE + p);
  return { status: r.status, json: await r.json() };
}

// Binary responses (the thumbnail cache serves JPEGs, so `api()`'s text body is
// not usable).
async function apiRaw(p, port = PORT) {
  const r = await fetch(`http://127.0.0.1:${port}${p}`);
  return {
    status: r.status,
    headers: Object.fromEntries(r.headers),
    body: Buffer.from(await r.arrayBuffer()),
  };
}

// The thumbnail URL the SPA builds for a row.  `v` is the file's identity
// (`<mtime>-<ctime>-<size>-<inode>`) exactly as the API reports it, which is what
// keeps the browser cache from ever showing a stale thumbnail.
function thumbUrlFor(rel) {
  const st = fs.statSync(abs(rel));
  const enc = rel.split('/').map(encodeURIComponent).join('/');
  const fp = `${Math.floor(st.mtimeMs / 1000)}-${Math.floor(st.ctimeMs / 1000)}-${st.size}-${st.ino}`;
  return `/api/thumb?path=${enc}&v=${fp}`;
}

// Width/height from the first SOF marker — enough to prove the thumbnail was
// resized rather than copied.
function jpegSize(buf) {
  if (buf.length < 4 || buf[0] !== 0xff || buf[1] !== 0xd8) return null;
  let i = 2;
  while (i + 9 < buf.length) {
    if (buf[i] !== 0xff) { i++; continue; }
    const m = buf[i + 1];
    if (m >= 0xc0 && m <= 0xcf && m !== 0xc8 && m !== 0xcc) {
      return { w: buf.readUInt16BE(i + 7), h: buf.readUInt16BE(i + 5) };
    }
    i += 2 + buf.readUInt16BE(i + 2);
  }
  return null;
}

// The src of the <img> a row renders, or null when it shows the emoji instead.
const rowThumbSrc = (name) =>
  page.evaluate((n) => {
    const r = document.querySelector(`[data-fs-name=${JSON.stringify(n)}]`);
    const img = r && r.querySelector('img');
    return img ? img.getAttribute('src') : null;
  }, name);

// ---------------------------------------------------------------------------
// tests

const tests = [];
const test = (name, fn) => tests.push([name, fn]);

test('boot: session + initial listing', async () => {
  const w = await apiJson('/api/whoami');
  assert.equal(w.json.username, 'local');
  const names = await rowNames();
  assert.ok(names.includes('docs'), 'fixture dirs are listed');
  assert.ok(names.includes('notes.txt'), 'fixture files are listed');
  assert.match(await statusText(), new RegExp(`${names.length} item\\(s\\)`));
});

test('navigation: double-click a folder, breadcrumb, back', async () => {
  await openRow('docs');
  assert.ok((await rowNames()).includes('readme.md'));
  assert.ok(await page.evaluate(() => [...document.querySelectorAll('#root button')].some((b) => b.innerText.includes('docs'))), 'breadcrumb shows the current folder');

  await clickButton('/');
  await waitLoaded();
  assert.ok((await rowNames()).includes('notes.txt'));

  await clickButton('←');
  await waitLoaded();
  assert.ok((await rowNames()).includes('readme.md'));
});

test('navigation: up button', async () => {
  await openRow('photos');
  await openRow('2024');
  assert.ok((await rowNames()).includes('cat.png'));
  await clickButton('↑');
  await waitLoaded();
  const bc = await page.evaluate(() => [...document.querySelectorAll('#root button')].map((b) => b.innerText));
  assert.ok(bc.some((b) => b.includes('photos')), 'breadcrumb is back on /photos');
  assert.ok((await rowNames()).includes('2024'));
});

test('url: the current folder is in the hash, so a hard refresh lands back in it', async () => {
  await openRow('photos');
  assert.equal(new URL(page.url()).hash, '#photos');

  await page.reload();
  await waitLoaded();
  assert.ok((await rowNames()).includes('2024'), 'the reload restored /photos');

  // the restored folder is the current history entry, not a step back to a root
  // the user never visited
  await blur();
  await page.keyboard.press('Backspace');
  await waitLoaded();
  assert.ok((await rowNames()).includes('2024'), 'back from the restored folder does not jump to the root');

  // percent-encoded, and decoded back on load
  await clickButton('/');
  await waitLoaded();
  await openRow('a b');
  assert.equal(new URL(page.url()).hash, '#a%20b');
  await page.reload();
  await waitLoaded();
  assert.ok((await rowNames()).includes('x.txt'), 'the encoded hash decodes to the right folder');
});

test('download through the context menu', async () => {
  const w = page.waitForEvent('download', { timeout: 8000 });
  await row('notes.txt').click({ button: 'right' });
  await menuItem('⬇ Download').click();
  const d = await w;
  assert.equal(d.suggestedFilename(), 'notes.txt');
});

test('zip through the toolbar archives the selection', async () => {
  const w = page.waitForEvent('download', { timeout: 8000 });
  await row('docs').click();
  await clickButton('Zip');
  const d = await w;
  assert.equal(d.suggestedFilename(), 'docs.zip');
  const buf = fs.readFileSync(await d.path());
  assert.ok(zipEntries(buf).some((e) => e.name === 'docs/readme.md'), 'the archive contains the folder');
});

test('details view + sorting', async () => {
  await clickButton('Details');
  const order = await rowNames();
  assert.ok(order.length > 0);
  const firstFile = order.findIndex((n) => fs.statSync(abs(n)).isFile());
  assert.ok(firstFile > 0, 'directories sort before files');

  const sizeCol = () =>
    page.evaluate(() => [...document.querySelectorAll('tbody tr')].map((r) => r.children[1].innerText));

  await page.locator('th', { hasText: 'Size' }).click();
  await page.waitForTimeout(300);
  const a = await sizeCol();
  assert.ok(a.some((s) => s !== '—'), 'the size column is populated');

  await page.locator('th', { hasText: 'Size' }).click();
  await page.waitForTimeout(300);
  const b = await sizeCol();
  assert.notDeepEqual(a, b, 'clicking the header again reverses the sort');

  await page.locator('th', { hasText: 'Modified' }).click();
  await page.waitForTimeout(300);
  const c = await page.evaluate(() => [...document.querySelectorAll('tbody tr')].map((r) => r.children[2].innerText));
  assert.ok(c.some((s) => /\d{4}-\d{2}-\d{2}/.test(s)), 'the Modified column shows dates');
});

test('selection: click, ctrl-click, shift-click, select all', async () => {
  const names = await rowNames();
  await row(names[0]).click();
  assert.match(await statusText(), /1 selected/);

  await page.keyboard.down('Control');
  await row(names[1]).click();
  await page.keyboard.up('Control');
  assert.match(await statusText(), /2 selected/);

  await page.keyboard.down('Shift');
  await row(names[3]).click();
  await page.keyboard.up('Shift');
  assert.match(await statusText(), /4 selected/);

  await page.keyboard.press('Control+a');
  assert.match(await statusText(), new RegExp(`${names.length} selected`));
});

test('selection: arrow keys move the selection, shift+arrow extends', async () => {
  // Details view keeps one row per arrow; in the icon grid an arrow spans a
  // whole row of columns, which depends on the viewport width.
  await clickButton('Details');
  const names = await rowNames();

  await row(names[0]).click();
  assert.deepEqual(await selectedNames(), [names[0]], 'the click seeds the anchor');

  await page.keyboard.press('ArrowDown');
  assert.deepEqual(await selectedNames(), [names[1]], 'ArrowDown moves to the next row');

  await page.keyboard.down('Shift');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.up('Shift');
  assert.deepEqual(await selectedNames(), [names[1], names[2]], 'shift+ArrowDown extends from the anchor');

  // the anchor is where the arrow last landed, so a shift-click ranges from
  // there rather than from the row that was clicked first (and, as with
  // shift+click everywhere in the app, it adds to the existing selection)
  await page.keyboard.down('Shift');
  await row(names[4]).click();
  await page.keyboard.up('Shift');
  assert.deepEqual(await selectedNames(), names.slice(1, 5), 'shift-click ranges from the arrow anchor');

  await page.keyboard.press('ArrowUp');
  assert.deepEqual(await selectedNames(), [names[3]], 'a plain arrow replaces the range');

  // the search box must keep its own arrow keys
  await page.fill('#fs-search', 'notes');
  await page.keyboard.press('ArrowDown');
  assert.deepEqual(await selectedNames(), [names[3]], 'arrows in the search box do not move the selection');
});

test('selection: arrow keys in the icon grid move by the visible column count', async () => {
  const names = await rowNames();
  const cols = await page.evaluate(() => {
    const a = document.getElementById('file-area');
    const t = getComputedStyle(a).gridTemplateColumns;
    return t === 'none' ? 1 : t.split(' ').length;
  });
  assert.ok(names.length > cols, 'the fixture spans more than one grid row');

  await row(names[0]).click();
  await page.keyboard.press('ArrowDown');
  assert.deepEqual(await selectedNames(), [names[cols]], `ArrowDown steps a whole row (${cols} columns)`);
});

test('context menu: row', async () => {
  await row('notes.txt').click({ button: 'right' });
  const items = await menuItems();
  for (const want of ['Open', 'Preview', 'Download', 'Delete (1)', 'Rename']) {
    assert.ok(items.some((i) => i.includes(want)), `menu has ${want} (got ${JSON.stringify(items)})`);
  }
});

test('context menu: background (empty area)', async () => {
  const box = await page.locator('#file-area').boundingBox();
  // avoid the toast/upload overlays pinned to the bottom corners
  await page.mouse.click(box.x + box.width / 2, box.y + box.height - 60, { button: 'right' });
  const items = await menuItems();
  assert.ok(items.length > 0, 'background right-click opens the menu');
  assert.ok(items.some((i) => i.includes('New folder')), 'background menu offers New folder');
});

test('context menu: clamped inside the viewport near the edges', async () => {
  const vp = await page.evaluate(() => ({ w: window.innerWidth, h: window.innerHeight }));
  const area = await page.locator('#file-area').boundingBox();

  // Bottom-right: painted at the click point the menu would hang off both edges.
  await page.mouse.click(area.x + area.width - 10, area.y + area.height - 40, { button: 'right' });
  // The menu can only be measured after the first paint, so the position is
  // corrected on the following render.
  await page.waitForTimeout(300);
  let box = await page.locator('#fs-menu').boundingBox();
  assert.ok(box, 'the menu is open');
  assert.ok(
    box.x + box.width <= vp.w + 1,
    `menu fits horizontally (x=${Math.round(box.x)}, w=${Math.round(box.width)}, viewport=${vp.w})`,
  );
  assert.ok(
    box.y + box.height <= vp.h + 1,
    `menu fits vertically (y=${Math.round(box.y)}, h=${Math.round(box.height)}, viewport=${vp.h})`,
  );

  // Top-left: clamping must not push it off the opposite edge either.
  await page.mouse.click(area.x + 4, area.y + 4, { button: 'right' });
  await page.waitForTimeout(300);
  box = await page.locator('#fs-menu').boundingBox();
  assert.ok(box, 'the menu is open');
  assert.ok(box.x >= 0 && box.y >= 0, `menu stays on screen from the top-left (x=${Math.round(box.x)}, y=${Math.round(box.y)})`);
});

test('context menu: closes when clicking whitespace', async () => {
  const gone = () => page.waitForFunction(() => !document.getElementById('fs-menu'), null, { timeout: 5000 });
  const area = await page.locator('#file-area').boundingBox();
  // open it in the far corner so the menu never covers the thing we click next
  const openFar = async () => {
    await page.mouse.click(area.x + area.width - 10, area.y + area.height - 40, { button: 'right' });
    assert.ok((await menuItems()).length > 0, 'the menu is open');
  };

  await openFar();
  await page.mouse.click(area.x + 40, area.y + 40);
  await gone();

  // a click on a row closes it too, so it cannot linger over the grid
  await openFar();
  await row('notes.txt').click();
  await gone();

  // Escape still works
  await openFar();
  await page.keyboard.press('Escape');
  await gone();
});

test('new folder / new file via modal', async () => {
  await clickButton('New folder');
  await modalInput().fill('newdir');
  await modalButton('OK').click();
  await waitToast('ok: newdir');
  assert.ok(exists('newdir'), 'the folder was created on disk');

  await clickButton('New file');
  await modalInput().fill('newfile.txt');
  await modalButton('OK').click();
  await waitToast('ok: newfile.txt');
  assert.ok(exists('newfile.txt'), 'the file was created on disk');
});

test('rename via modal', async () => {
  await row('notes.txt').click({ button: 'right' });
  await menuItem('Rename').click();
  await modalInput().fill('renamed.txt');
  await modalButton('OK').click();
  await waitToast('ok: renamed.txt');
  assert.ok(exists('renamed.txt'));
  assert.ok(!exists('notes.txt'));
});

test('delete from a subdirectory', async () => {
  await openRow('docs');
  await row('notes.txt').click();
  await clickButton('Delete');
  await modalButton('Delete').click();
  await waitToast('deleted 1 item(s)');
  assert.ok(!exists('docs/notes.txt'), 'the file is gone from disk');
  assert.ok(exists('docs/readme.md'), 'the other file is untouched');
});

test('preview: text', async () => {
  await openRow('notes.txt');
  await page.waitForSelector('#fs-preview pre', { timeout: 8000 });
  const text = await page.evaluate(() => document.querySelector('#fs-preview pre').innerText);
  assert.ok(text.includes('hello world'), `preview shows the file text (got ${JSON.stringify(text)})`);
  await closePreview();
  assert.ok((await rowNames()).includes('notes.txt'), 'the grid is still there after closing the preview');
});

test('preview: truncated text', async () => {
  await openRow('big-truncated.txt');
  await page.waitForSelector('#fs-preview pre', { timeout: 15000 });
  const text = await page.evaluate(() => document.querySelector('#fs-preview pre').innerText);
  assert.ok(text.includes('truncated'), 'the truncation notice is shown');
});

test('preview: image', async () => {
  await openRow('photos');
  await openRow('2024');
  await row('cat.png').dblclick();
  await page.waitForSelector('#fs-preview img', { timeout: 8000 });
  const w = await page.evaluate(() => document.querySelector('#fs-preview img').naturalWidth);
  assert.equal(w, 1, 'the image actually decodes');
});

test('preview: each kind renders in the element the browser can actually show', async () => {
  // One probe per kind in common-rs/preview.  The table is shared by the server
  // and the SPA, so this checks the two halves agree on both the kind and the
  // element it is rendered in.
  const probes = [
    ['probe.png', 'image', 'img', 'image/png'],
    ['probe.mp4', 'video', 'video', 'video/mp4'],
    ['probe.opus', 'audio', 'audio', 'audio/opus'],
    ['probe.pdf', 'pdf', 'iframe', 'application/pdf'],
    ['probe.py', 'text', 'pre', 'text/plain; charset=utf-8'],
  ];
  for (const [name] of probes) fs.writeFileSync(abs(name), 'x');
  await page.reload();
  await waitLoaded();

  for (const [name, kind, el, ct] of probes) {
    await row(name).dblclick();
    await page.waitForSelector(`#fs-preview ${el}`, { timeout: 8000 });
    const label = await page.evaluate(() => document.getElementById('fs-preview-kind').innerText.trim());
    assert.equal(label, kind, `${name} renders as a ${el} and is labelled ${kind}`);
    const r = await api('/api/preview?path=' + name);
    if (kind === 'text') {
      assert.equal(JSON.parse(r.body).kind, 'text', `${name} is served as text`);
    } else {
      assert.equal(r.headers['content-type'], ct, `${name} is served with its table content type`);
    }
    await closePreview();
  }
});

test('preview: audio really decodes', async () => {
  await row('media').dblclick();
  await row('sound.wav').dblclick();
  await page.waitForSelector('#fs-preview audio', { timeout: 8000 });
  const m = await page.evaluate(async () => {
    const a = document.querySelector('#fs-preview audio');
    if (a.readyState < 1) {
      await new Promise((r) => {
        a.addEventListener('loadedmetadata', r, { once: true });
        setTimeout(r, 6000);
      });
    }
    return { duration: a.duration, error: a.error ? a.error.code : null };
  });
  assert.equal(m.error, null, 'the audio element loaded the file');
  assert.ok(Math.abs(m.duration - 1) < 0.01, `the fixture is 1 s long (got ${m.duration})`);
  await closePreview();
});

test('preview: media answers a Range request, so a seek does not re-download the file', async () => {
  const r = await fetch(BASE + '/api/preview?path=media/clip.mp4', { headers: { Range: 'bytes=0-3' } });
  assert.equal(r.status, 206, 'a single range is a partial response');
  assert.equal(r.headers.get('content-range'), 'bytes 0-3/8');
  assert.equal(r.headers.get('accept-ranges'), 'bytes');
  assert.equal((await r.arrayBuffer()).byteLength, 4, 'only the requested slice is sent');

  // open-ended range, and a nonsense one (which serves the whole file)
  const open = await fetch(BASE + '/api/preview?path=media/clip.mp4', { headers: { Range: 'bytes=4-' } });
  assert.equal(open.status, 206);
  assert.equal((await open.arrayBuffer()).byteLength, 4);
  const bad = await fetch(BASE + '/api/preview?path=media/clip.mp4', { headers: { Range: 'bytes=0-3,4-7' } });
  assert.equal(bad.status, 200, 'multiple ranges are not supported, so the whole file is served');
});

test('preview: media responses are not executable documents', async () => {
  // The store's content is served from the app's own origin.  An <img>/<video>/
  // <audio> cannot run script, but the same URL opened as a document can, so the
  // response carries nosniff and a CSP that allows nothing.
  const h = (await api('/api/preview?path=drawing.svg')).headers;
  assert.equal(h['content-type'], 'image/svg+xml', 'svg is previewable as an image');
  assert.equal(h['x-content-type-options'], 'nosniff');
  assert.equal(h['content-security-policy'], "default-src 'none'");

  await row('drawing.svg').dblclick();
  await page.waitForSelector('#fs-preview img', { timeout: 8000 });
  await closePreview();
});

test('preview: a well-known name with no extension is still text', async () => {
  await row('Makefile').dblclick();
  await page.waitForSelector('#fs-preview pre', { timeout: 8000 });
  const text = await page.evaluate(() => document.querySelector('#fs-preview pre').innerText);
  assert.ok(text.includes('echo hi'), 'Makefile previews as text');
  await closePreview();
});

test('preview: a file the table does not know is not advertised, but the API still tries it as text', async () => {
  fs.writeFileSync(abs('odd-thing.unknown'), 'plain text with no known extension\n');
  await page.reload();
  await waitLoaded();
  await row('odd-thing.unknown').click({ button: 'right' });
  const items = await menuItems();
  assert.ok(
    !items.some((i) => i.includes('Preview')),
    `an unknown extension must not promise a preview (got ${JSON.stringify(items)})`,
  );
  await page.keyboard.press('Escape');
  const r = await apiJson('/api/preview?path=odd-thing.unknown');
  assert.equal(r.json.kind, 'text', 'fetching it directly still gives the text');

  // and a real binary is refused, not garbled
  const b = await apiJson('/api/preview?path=binary.bin');
  assert.match(b.json.error, /binary/, 'binary content is refused as text');
});

test('preview: left/right step through the previewable files in the folder', async () => {
  const pname = () => page.evaluate(() => document.getElementById('fs-preview-name').innerText.trim());
  const waitName = (n) =>
    page.waitForFunction((name) => document.getElementById('fs-preview-name').innerText.trim() === name, n, { timeout: 8000 });

  await openRow('photos');
  await openRow('2024');
  await row('cat.png').dblclick();
  await page.waitForSelector('#fs-preview img', { timeout: 8000 });
  assert.equal(await pname(), 'cat.png');

  await page.keyboard.press('ArrowRight');
  await waitName('dog.png');
  // the selection follows, so closing the popup leaves you on the last file
  assert.deepEqual(await selectedNames(), ['dog.png']);

  // clamped at the end rather than wrapping
  await page.keyboard.press('ArrowRight');
  await page.waitForTimeout(300);
  assert.equal(await pname(), 'dog.png', 'the last file does not wrap to the first');

  await page.keyboard.press('ArrowLeft');
  await waitName('cat.png');
  await closePreview();
});

test('search: shallow and deep', async () => {
  // deep search from the root must find a file several levels down
  await page.fill('#fs-search', 'needle');
  await page.keyboard.press('Enter');
  await page.waitForFunction(
    () => [...document.querySelectorAll('#file-area tbody tr td')].some((t) => t.innerText.includes('needle-found-me')),
    null,
    { timeout: 10000 },
  );
  assert.ok(await page.evaluate(() => document.body.innerText.includes('(deep)'), 'deep mode is the default'));

  // toggle to shallow: the nested file must no longer appear
  await page.locator('#root button', { hasText: 'deep' }).click();
  await page.locator('#root button', { hasText: '🔍' }).click();
  await page.waitForFunction(
    () => document.body.innerText.includes('0 result(s) for "needle" in / (shallow)'),
    null,
    { timeout: 10000 },
  );
});

test('search: shallow mode reports the entries it actually scanned', async () => {
  const r = await apiJson('/api/search?path=&q=notes&deep=false');
  assert.equal(r.json.results.hits.length, 1);
  const entries = Object.keys(FIXTURE).filter((k) => !k.includes('/')).length;
  assert.ok(r.json.results.scanned >= entries, `scanned should count the listing (got ${r.json.results.scanned})`);
});

test('download: headers and body', async () => {
  const r = await api('/api/download?path=notes.txt');
  assert.equal(r.status, 200);
  assert.match(r.headers['content-disposition'] || '', /attachment; filename="notes\.txt"/);
  assert.equal(r.body, 'hello world\nsecond line\n');
});

test('zip: parses as a real archive', async () => {
  const r = await fetch(`${BASE}/api/zip?path=docs&path=notes.txt`);
  assert.equal(r.status, 200);
  const buf = Buffer.from(await r.arrayBuffer());
  const entries = zipEntries(buf).map((e) => e.name);
  assert.ok(entries.includes('docs/'), 'directory entry present');
  assert.ok(entries.includes('docs/readme.md'), 'nested file entry present');
  assert.ok(entries.includes('notes.txt'), 'top-level file entry present');
});

test('zip: entries actually inflate', async () => {
  const r = await fetch(`${BASE}/api/zip?path=docs`);
  const buf = Buffer.from(await r.arrayBuffer());
  const local = findLocal(buf, 'docs/readme.md');
  assert.ok(local >= 0, 'local header found');
  const nameLen = buf.readUInt16LE(local + 26);
  const extraLen = buf.readUInt16LE(local + 28);
  const method = buf.readUInt16LE(local + 8);
  const body = inflateEntry(buf, local + 30 + nameLen + extraLen, method, buf.readUInt32LE(local + 18) || 30);
  assert.equal(body, '# Title\n\nSome **markdown**.\n', 'the entry inflates back to the original bytes');
});

test('zip: central directory sizes and CRCs match the data', async () => {
  const r = await fetch(`${BASE}/api/zip?path=docs&path=notes.txt`);
  const buf = Buffer.from(await r.arrayBuffer());
  const entries = zipEntries(buf);
  assert.ok(entries.length >= 3, 'central directory has the entries');
  for (const e of entries) {
    if (e.name.endsWith('/')) continue;
    const local = findLocal(buf, e.name);
    assert.ok(local >= 0, `local header for ${e.name}`);
    const nl = buf.readUInt16LE(local + 26);
    const el = buf.readUInt16LE(local + 28);
    const data = inflateEntry(buf, local + 30 + nl + el, buf.readUInt16LE(local + 8), e.compressed);
    assert.equal(data.length, e.uncompressed, `${e.name}: uncompressed size`);
    assert.equal(crc32(Buffer.from(data)), e.crc, `${e.name}: crc32`);
  }
});

test('upload: raw body API', async () => {
  const r = await fetch(`${BASE}/api/upload?path=scratch&name=sub%2Ffile.txt`, {
    method: 'POST',
    body: 'uploaded content',
  });
  assert.equal(r.status, 200);
  assert.equal(read('scratch/sub/file.txt'), 'uploaded content');
});

test('upload: file picker', async () => {
  const tmp = path.join(os_tmp, 'filestore-test-upload.txt');
  fs.writeFileSync(tmp, 'picked content');
  await page.setInputFiles('#fs-upload-input', tmp);
  await page.waitForFunction(() => document.getElementById('fs-uploads'), null, { timeout: 8000 });
  await page.waitForFunction(
    () => [...document.querySelectorAll('#fs-uploads div div')].some((e) => e.innerText.includes('✓')),
    null,
    { timeout: 8000 },
  );
  assert.ok(exists('filestore-test-upload.txt'), 'the picker uploaded the file to the current dir');
});

test('drag: internal move into a folder', async () => {
  await row('move-me.txt').dragTo(row('scratch'));
  await waitToast('moved 1 item(s)');
  assert.ok(exists('scratch/move-me.txt'), 'the file moved into scratch/');
  assert.ok(!exists('move-me.txt'), 'the source is gone');
});

test('drag: internal copy with ctrl', async () => {
  await page.keyboard.down('Control');
  await row('move-me.txt').dragTo(row('scratch'));
  await page.keyboard.up('Control');
  await waitToast('copied 1 item(s)');
  assert.ok(exists('scratch/move-me.txt'), 'the copy landed');
  assert.ok(exists('move-me.txt'), 'the source is still there (copy, not move)');
});

test('drag out: dragstart publishes text/uri-list for targets outside the page', async () => {
  // Playwright cannot drop onto the OS, so drive dragstart with a real
  // DataTransfer and read back what the row put on it.  A page can only hand an
  // OS target a URL or bytes it already holds, so this is the whole observable
  // half of the drag-out path.
  const grab = (name) =>
    page.evaluate((n) => {
      const r = document.querySelector(`[data-fs-name=${JSON.stringify(n)}]`);
      const dt = new DataTransfer();
      r.dispatchEvent(new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer: dt }));
      return {
        types: [...dt.types],
        uri: dt.getData('text/uri-list'),
        html: dt.getData('text/html'),
        internal: dt.getData('application/x-filestore'),
      };
    }, name);

  const f = await grab('notes.txt');
  assert.ok(
    f.types.includes('text/uri-list'),
    `the drag carries a format an external drop target can read (got ${JSON.stringify(f.types)})`,
  );
  assert.match(f.uri, /\/api\/download\?path=notes\.txt$/, `a file drag publishes its download URL (got ${f.uri})`);
  assert.match(f.internal, /notes\.txt/, 'the internal format is still there for internal drops');

  // Finder names the .webloc from the link text, so the anchor has to be there
  assert.ok(f.types.includes('text/html'), 'the drag also carries an anchor for Finder');
  assert.match(
    f.html,
    /<a href="[^"]*\/api\/download\?path=notes\.txt">notes\.txt<\/a>/,
    `the link text is the file name, not the host (got ${f.html})`,
  );

  // a folder has no single file, so it publishes its streaming ZIP instead
  const d = await grab('docs');
  assert.match(d.uri, /\/api\/zip\?path=docs$/, `a folder drag publishes its ZIP (got ${d.uri})`);
});

test('keyboard: Escape closes menu, modal and preview', async () => {
  await row('notes.txt').click({ button: 'right' });
  await page.waitForSelector('#fs-menu', { timeout: 8000 });
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.getElementById('fs-menu'), null, { timeout: 8000 });

  await clickButton('New folder');
  await page.waitForSelector('#fs-modal', { timeout: 8000 });
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.getElementById('fs-modal'), null, { timeout: 8000 });

  await row('notes.txt').dblclick();
  await page.waitForSelector('#fs-preview pre', { timeout: 8000 });
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => !document.getElementById('fs-preview'), null, { timeout: 8000 });
});

test('keyboard: a letter jumps the selection to the entry starting with it', async () => {
  await blur();
  await page.keyboard.press('p');
  assert.deepEqual(await selectedNames(), ['photos'], 'p selects the Photos folder');

  // repeat presses cycle through the matches, in the order the grid shows them
  // (directories first, so Photos before pandas.csv)
  await page.keyboard.press('p');
  assert.deepEqual(await selectedNames(), ['pandas.csv']);
  await page.keyboard.press('p');
  assert.deepEqual(await selectedNames(), ['photos'], 'and wrap back to the first match');

  // a different letter starts that letter's matches
  await page.keyboard.press('n');
  assert.deepEqual(await selectedNames(), ['notes.txt']);

  // the search box keeps its own typing
  await page.locator('#fs-search').focus();
  await page.keyboard.type('p');
  assert.deepEqual(await selectedNames(), ['notes.txt'], 'typing in the search box must not move the selection');
});

test('keyboard: Enter opens the focused row', async () => {
  // a file that cannot be previewed falls back to a download
  const w = page.waitForEvent('download', { timeout: 8000 });
  await row('binary.bin').click();
  await page.keyboard.press('Enter');
  assert.equal((await w).suggestedFilename(), 'binary.bin', 'Enter downloads a file it cannot preview');

  await row('notes.txt').click();
  await page.keyboard.press('Enter');
  await page.waitForSelector('#fs-preview pre', { timeout: 8000 });
  await closePreview();
  await blur();

  await row('docs').click();
  await page.keyboard.press('Enter');
  await waitLoaded();
  assert.ok((await rowNames()).includes('readme.md'), 'Enter opens the selected folder');

  // the arrow-key anchor is what Enter opens, not only the row clicked
  await clickButton('/');
  await blur();
  await clickButton('Details');
  await blur();
  const names = await rowNames();
  await row(names[0]).click();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await waitLoaded();
  const bc = await page.evaluate(() => [...document.querySelectorAll('#root button')].map((b) => b.innerText));
  assert.ok(bc.some((b) => b.includes(names[1])), `Enter opened the row the arrow moved to (${names[1]})`);

  // Enter in a text field belongs to that field (the search box runs the search)
  await clickButton('/');
  await page.fill('#fs-search', 'needle');
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => document.body.innerText.includes('needle-found-me'), null, { timeout: 10000 });
  assert.ok(
    await page.evaluate(() => document.body.innerText.includes('result(s) for "needle"')),
    'Enter in the search box still runs the search',
  );
});

test('search result double-click jumps to the containing folder', async () => {
  await page.fill('#fs-search', 'needle');
  await page.keyboard.press('Enter');
  await page.waitForFunction(
    () => [...document.querySelectorAll('#file-area tbody tr td')].some((t) => t.innerText.includes('needle-found-me')),
    null,
    { timeout: 10000 },
  );
  await page.locator('#file-area tbody tr').first().dblclick();
  await waitLoaded();
  assert.ok((await rowNames()).includes('needle-found-me.txt'), 'landed in the containing folder');
});

test('delete through the context menu', async () => {
  await row('notes.txt').click({ button: 'right' });
  await menuItem('Delete (1)').click();
  await page.waitForSelector('#fs-modal', { timeout: 8000 });
  await modalButton('Delete').click();
  await waitToast('deleted 1 item(s)');
  assert.ok(!exists('notes.txt'));
});

test('upload: oversized body is refused with a JSON 413 that names the limit', async () => {
  // The default limit is 2G, which the suite cannot exercise; the limit is a
  // flag, so start a second server with a tiny one.  The upload route takes the
  // raw body, so axum's DefaultBodyLimit does not apply to it — this is the only
  // server-side limit (nginx has its own, see machines/services1/services/filestore.nix).
  const port = 18099;
  const p = await startServer(['--root', ROOT, '--no-auth', '--port', String(port), '--max-upload', '1024'], port);
  try {
    const r = await fetch(`http://127.0.0.1:${port}/api/upload?path=&name=too-big.bin`, {
      method: 'POST',
      body: 'x'.repeat(2048),
    });
    assert.equal(r.status, 413, 'over the limit is refused');
    const j = await r.json();
    assert.match(j.error, /exceeds the 1\.0 KiB limit/, `the error names the limit (got ${JSON.stringify(j)})`);
    assert.ok(!exists('too-big.bin'), 'nothing was written');

    const ok = await fetch(`http://127.0.0.1:${port}/api/upload?path=&name=small.bin`, {
      method: 'POST',
      body: 'x'.repeat(512),
    });
    assert.equal(ok.status, 200, 'under the limit still uploads');
  } finally {
    p.kill();
  }
});

test('path traversal is rejected', async () => {
  const r = await apiJson('/api/list?path=../etc');
  assert.equal(r.status, 400);
  assert.match(r.json.error, /invalid path/);
});

test('auth guard: --no-auth is refused on a non-loopback bind', async () => {
  const out = runServer(['--root', ROOT, '--no-auth', '--bind', '0.0.0.0', '--port', '18099']);
  assert.equal(out.code, 1, 'the escape hatch must not be usable on a routable bind');
  assert.match(out.stderr, /loopback/);
});

test('auth guard: without --no-auth the API is still session-gated', async () => {
  const env = path.join(WORKDIR, 'fake.env');
  fs.writeFileSync(env, 'FILESTORE_OIDC_CLIENT_ID=x\nFILESTORE_OIDC_CLIENT_SECRET=y\n');
  const port = 18098;
  const p = await startServer(['--root', ROOT, '--env-file', env, '--port', String(port)], port);
  try {
    const r = await fetch(`http://127.0.0.1:${port}/api/list?path=`);
    assert.equal(r.status, 401, 'an unauthenticated request is rejected');
    const d = await fetch(`http://127.0.0.1:${port}/auth/dev-login`);
    assert.equal(d.status, 404, 'the dev-login mint is off without --dev-user');
  } finally {
    p.kill();
  }
});

test('api edge cases', async () => {
  const cases = [
    ['/api/download?path=docs', 400, 'a directory is not downloadable'],
    ['/api/zip?path=nope', 404, 'missing path'],
    ['/api/preview?path=binary.bin', 400, 'binary is not previewable'],
    ['/api/list?path=nope', 404, 'missing dir'],
  ];
  for (const [p, want, what] of cases) {
    const r = await apiJson(p);
    assert.equal(r.status, want, `${p} should ${what} (got ${r.status})`);
  }
  const up = await fetch(`${BASE}/api/upload?path=scratch&name=..%2Fescape.txt`, { method: 'POST', body: 'x' });
  assert.equal(up.status, 400, 'an upload name cannot escape the store root');
});

test('api: rename and copy across directories', async () => {
  const post = async (url, body) => {
    const r = await fetch(BASE + url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
    return { status: r.status, json: await r.json() };
  };
  assert.equal((await post('/api/mkdir', { path: 'scratch/incoming' })).status, 200);
  assert.equal((await post('/api/rename', { from: 'sub/a.txt', to: 'scratch/incoming/a.txt' })).status, 200);
  assert.ok(exists('scratch/incoming/a.txt') && !exists('sub/a.txt'));
  assert.equal((await post('/api/copy', { from: 'scratch/incoming/a.txt', to: 'scratch/copy.txt' })).status, 200);
  assert.ok(exists('scratch/incoming/a.txt') && exists('scratch/copy.txt'));
  assert.equal((await post('/api/copy', { from: 'scratch/copy.txt', to: '../escape' })).status, 400, 'copy cannot escape the root');
});

test('empty folder message', async () => {
  await openRow('empty-dir');
  await page.waitForFunction(
    () => document.getElementById('fs-content').innerText.includes('Empty folder'),
    null,
    { timeout: 8000 },
  );
});

test('unicode and spaced names survive the round trip', async () => {
  await openRow('weird dir with spaces');
  assert.ok((await rowNames()).includes('a file.txt'));
  await clickButton('/');
  await waitLoaded();
  await openRow('écho-η');
  assert.ok((await rowNames()).includes('ünïcode.txt'));
});

// ---------------------------------------------------------------------------
// zip helpers (no deps)

function zipEntries(buf) {
  let eocd = -1;
  for (let i = buf.length - 22; i >= 0; i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) { eocd = i; break; }
  }
  assert.ok(eocd >= 0, 'no end-of-central-directory record — not a valid zip');
  const count = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16);
  const out = [];
  for (let i = 0; i < count; i++) {
    assert.equal(buf.readUInt32LE(p), 0x02014b50, `bad central directory entry at ${p}`);
    const nameLen = buf.readUInt16LE(p + 28);
    const extraLen = buf.readUInt16LE(p + 30);
    const commentLen = buf.readUInt16LE(p + 32);
    out.push({
      name: buf.subarray(p + 46, p + 46 + nameLen).toString('utf8'),
      crc: buf.readUInt32LE(p + 16),
      compressed: buf.readUInt32LE(p + 20),
      uncompressed: buf.readUInt32LE(p + 24),
      localOffset: buf.readUInt32LE(p + 42),
    });
    p += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

function crc32(buf) {
  const table = crc32.t || (crc32.t = (() => {
    const t = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
      let c = i;
      for (let j = 0; j < 8; j++) c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1);
      t[i] = c;
    }
    return t;
  })());
  let c = 0xFFFFFFFF;
  for (const b of buf) c = table[(c ^ b) & 0xFF] ^ (c >>> 8);
  return (c ^ 0xFFFFFFFF) >>> 0;
}

function findLocal(buf, name) {
  for (let i = 0; i + 30 <= buf.length; i++) {
    if (buf.readUInt32LE(i) !== 0x04034b50) continue;
    const nameLen = buf.readUInt16LE(i + 26);
    if (buf.subarray(i + 30, i + 30 + nameLen).toString('utf8') === name) return i;
  }
  return -1;
}

function inflateEntry(buf, dataStart, method, size) {
  const raw = buf.subarray(dataStart, dataStart + size);
  if (method === 0) return raw.toString('utf8');
  // flate2 emits raw deflate (no zlib header) because the data-descriptor form
  // cannot carry a CRC in the local header
  try {
    return zlib.inflateRawSync(raw).toString('utf8');
  } catch {
    for (let n = 1; n <= size; n++) {
      try {
        return zlib.inflateRawSync(buf.subarray(dataStart, dataStart + n)).toString('utf8');
      } catch { /* not complete yet */ }
    }
  }
  return '';
}

const os_tmp = fs.realpathSync(os.tmpdir());

// ---------------------------------------------------------------------------
// thumbnails

test('thumbnail: an image row renders a cached thumbnail, and the next request is a hit', async () => {
  // A file this test creates, so its identity is definitely not in the cache yet.
  fs.writeFileSync(abs('photos/fresh.bmp'), bmp(256, 128));
  const url = thumbUrlFor('photos/fresh.bmp');

  const first = await apiRaw(url);
  assert.equal(first.status, 200);
  assert.equal(first.headers['content-type'], 'image/jpeg');
  assert.equal(first.headers['x-thumb-cache'], 'miss', 'the first request generates the entry');
  assert.deepEqual(jpegSize(first.body), { w: 128, h: 64 }, 'the thumbnail is resized, not copied');

  const second = await apiRaw(url);
  assert.equal(second.headers['x-thumb-cache'], 'hit', 'the second is served from the cache');
  assert.equal(second.body.length, first.body.length);

  await openRow('photos');
  assert.equal(await rowThumbSrc('fresh.bmp'), url, 'the row renders the thumbnail');

  // a non-image row keeps the emoji
  assert.equal(await rowThumbSrc('notes.txt'), null, 'only image rows get a thumbnail');
});

test('thumbnail: a changed file can never be served the old thumbnail', async () => {
  const urlBefore = thumbUrlFor('photos/big.bmp');
  const before = await apiRaw(urlBefore);
  assert.equal(before.status, 200);
  assert.deepEqual(jpegSize(before.body), { w: 128, h: 64 });

  // Change the file.  Its identity changes, so the entry served is generated from
  // the file as it is now — there is nothing to invalidate.
  fs.writeFileSync(abs('photos/big.bmp'), bmp(64, 64));
  const urlAfter = thumbUrlFor('photos/big.bmp');
  assert.notEqual(urlAfter, urlBefore, 'a changed file is a different URL, so the browser cannot show the old one');

  const after = await apiRaw(urlAfter);
  assert.deepEqual(jpegSize(after.body), { w: 64, h: 64 }, 'the served thumbnail is the new file');
  assert.notEqual(after.headers.etag, before.headers.etag, 'a changed file is a different entry');

  // A hand-built URL with an old fingerprint still gets the current file: `v` only
  // busts the browser cache, it is never trusted for the key.
  const stale = await apiRaw('/api/thumb?path=photos%2Fbig.bmp&v=1-1-1-1');
  assert.deepEqual(jpegSize(stale.body), { w: 64, h: 64 }, 'a stale v regenerates');
});

test('thumbnail: the response is cacheable forever because the URL is the file identity', async () => {
  const url = thumbUrlFor('photos/big.bmp');
  const r = await apiRaw(url);
  assert.match(r.headers['cache-control'], /immutable/);
  assert.ok(r.headers.etag, 'the ETag is the hash of the file identity');

  // Revalidating with the current ETag is a 304: the file has not changed.
  const re = await fetch(BASE + url, { headers: { 'if-none-match': r.headers.etag } });
  assert.equal(re.status, 304);

  // An ETag from a different file state must not be reused.
  const other = await fetch(BASE + url, { headers: { 'if-none-match': 'not-the-current-file' } });
  assert.equal(other.status, 200);

  // Non-images are refused, so the SPA keeps the emoji.
  const t = await apiJson('/api/thumb?path=notes.txt');
  assert.equal(t.status, 400);
  assert.match(t.json.error, /thumbnailable/);
});

test('thumbnail: a file image cannot decode falls back to the emoji, and the failure is cached', async () => {
  const markers = () =>
    fs.readdirSync(THUMB_CACHE, { recursive: true }).filter((f) => String(f).endsWith('.err')).length;

  const before = markers();
  const r = await apiJson('/api/thumb?path=thumbfail%2Fbroken.png');
  assert.equal(r.status, 400);
  assert.equal(markers(), before + 1, 'the failure is stored in the cache');

  const second = await apiJson('/api/thumb?path=thumbfail%2Fbroken.png');
  assert.equal(second.status, 400, 'the failure is cached, not re-decoded');
  assert.equal(markers(), before + 1, 'the cached failure is reused, not regenerated');

  await openRow('thumbfail');
  // The row renders the img first; the failed request is what switches it to the
  // emoji, so the fallback has to be waited for.
  await page.waitForFunction(
    (n) => {
      const r = document.querySelector(`[data-fs-name=${JSON.stringify(n)}]`);
      return !!r && !r.querySelector('img');
    },
    'broken.png',
    { timeout: 8000 },
  );
  assert.equal(await rowThumbSrc('broken.png'), null, 'the row falls back to the emoji');
});

test('thumbnail: the temporary cache stays under --thumb-cache-max', async () => {
  const dir = path.join(WORKDIR, 'cap-cache');
  const port = 18100;
  const p = await startServer(
    ['--root', ROOT, '--no-auth', '--port', String(port), '--thumb-cache', dir, '--thumb-cache-max', '1200'],
    port,
  );
  try {
    for (let i = 0; i < 5; i++) {
      fs.writeFileSync(abs(`photos/cap${i}.bmp`), bmp(200, 200));
      const r = await apiRaw(`/api/thumb?path=photos%2Fcap${i}.bmp`, port);
      assert.equal(r.status, 200, `cap${i}: ${r.body.toString('utf8')}`);
    }
    const files = fs.readdirSync(dir, { recursive: true }).filter((f) => String(f).endsWith('.jpg'));
    const total = files.reduce((n, f) => n + fs.statSync(path.join(dir, f)).size, 0);
    assert.ok(total > 0, 'nothing was cached at all');
    assert.ok(total <= 1200, `cache is ${total} bytes, over the 1200 cap`);
    assert.ok(files.length < 5, `no entry was pruned (${files.length} still present)`);
  } finally {
    p.kill();
  }
});

// ---------------------------------------------------------------------------
// run

let failed = 0;
browser = await chromium.launch({ executablePath: process.env.FS_TEST_BROWSER || undefined });
page = await browser.newPage();
page.on('console', (m) => {
  // A refused thumbnail is a normal outcome (the UI falls back to the emoji), so a
  // failed /api/thumb request is not treated as a page error.
  if (m.type() === 'error' && !(m.location()?.url || '').includes('/api/thumb')) {
    consoleErrors.push(m.text() + (m.location()?.url ? ' @ ' + m.location().url : ''));
  }
});
page.on('pageerror', (e) => consoleErrors.push('pageerror: ' + e.message));

for (const [name, fn] of tests) {
  try {
    await reset();
    await fn();
    if (consoleErrors.length) throw new Error('console errors: ' + consoleErrors.join(' | '));
    console.log(`ok    ${name}`);
  } catch (e) {
    failed++;
    console.log(`FAIL  ${name}\n        ${String(e.message || e).split('\n')[0]}`);
  }
}

await browser.close();
console.log(`\n${tests.length} tests, ${failed} failure(s)`);
process.exit(failed ? 1 : 0);
