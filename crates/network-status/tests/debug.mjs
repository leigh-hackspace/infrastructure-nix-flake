#!/usr/bin/env node
// One-shot debug helper: load the page, print console errors and the rendered tree.
// Not part of the suite — it is what to run when a check fails and the assertion text is
// not enough to say why.
import { chromium } from 'playwright';

const PORT = Number(process.env.NS_TEST_PORT || 18091);
const browser = await chromium.launch({ executablePath: process.env.NS_TEST_BROWSER || undefined });
const page = await browser.newPage();
page.on('console', (m) => console.log('console:', m.type(), m.text()));
page.on('pageerror', (e) => console.log('pageerror:', e.message));
// Which URL answered 404 is the difference between "the bundle is not embedded" and "the
// page asked for something it does not need".
page.on('response', (r) => {
  if (r.status() >= 400) console.log('http', r.status(), r.url());
});
await page.goto(`http://127.0.0.1:${PORT}/`);
await new Promise((r) => setTimeout(r, 3000));
console.log('--- body html (first 1200 chars):');
console.log(await page.evaluate(() => document.body.innerHTML.slice(0, 1200)));
await browser.close();
