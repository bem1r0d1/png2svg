// UI smoke test: loads dist/ui.html in an iframe inside a harness page that
// plays the role of Figma (posts selection messages, records plugin messages).
import { chromium } from 'playwright';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const here = dirname(fileURLToPath(import.meta.url));
const out = resolve(here, '../test-results');
mkdirSync(out, { recursive: true });
const harness = resolve(out, 'harness.html');
writeFileSync(
  harness,
  `<!doctype html><body style="margin:0">
<iframe id="ui" src="${pathToFileURL(resolve(here, '../dist/ui.html'))}" style="width:420px;height:680px;border:0"></iframe>
<script>
  window.received = [];
  addEventListener('message', (e) => { if (e.data && e.data.pluginMessage) received.push(e.data.pluginMessage); });
  window.send = (msg) => document.getElementById('ui').contentWindow.postMessage({ pluginMessage: msg }, '*');
</script></body>`,
);

const png = readFileSync(resolve(here, '../../../corpus/png/logo-circles.png'));
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined });
const page = await browser.newPage({ viewport: { width: 420, height: 680 } });
const errors = [];
page.on('console', (m) => (m.type() === 'error' || m.type() === 'warning') && errors.push(m.text()));
page.on('pageerror', (e) => errors.push(String(e)));

await page.goto(pathToFileURL(harness).href);
await page.waitForFunction(() => window.received.some((m) => m.type === 'ready'));
const ui = page.frameLocator('#ui');

// Figma sends the document colours, then the selected image.
await page.evaluate(() => send({ type: 'docColors', colors: ['#e63946', '#1d3557'] }));
await page.evaluate((bytes) => send({ type: 'image', id: '1:2', name: 'Logo', bytes: new Uint8Array(bytes), nodeWidth: 128, nodeHeight: 128 }), [...png]);
await ui.locator('#stats').filter({ hasText: 'контуров' }).waitFor({ timeout: 15000 });
assert.equal(await ui.locator('#img-vector svg > *').count(), 4, 'logo-circles → 4 colour layers');
assert.equal(await ui.locator('#img-vector svg rect').count(), 1, 'background → native rectangle');
assert.equal(await ui.locator('#img-vector svg circle').count(), 1, 'outer ring → native circle');
assert.match(await ui.locator('#stats').textContent(), /фигур/);
assert.equal(await ui.locator('#palette .chip').count(), 4);
await page.screenshot({ path: resolve(out, 'ui-split.png') });

// Changing a control re-runs the conversion.
const before = await ui.locator('#stats').textContent();
await ui.locator('#smoothness').fill('1');
await ui.locator('#stats').filter({ hasNotText: before }).waitFor({ timeout: 15000 });

// "Replace" sends the insert request to the sandbox.
await ui.locator('#insert-replace').click();
await page.waitForFunction(() => window.received.some((m) => m.type === 'insert'));
const insert = await page.evaluate(() => window.received.find((m) => m.type === 'insert'));
assert.equal(insert.mode, 'replace');
assert.equal(insert.sourceId, '1:2');
assert.equal(insert.layers.length, 4);
assert.match(insert.svg, /<circle id="color-1b2a4a"/);
assert.match(insert.svg, /fill="#e63946"/, 'brand colour preserved exactly');

// Turning shapes off yields only paths.
await ui.locator('#shapes').uncheck();
await ui.locator('#img-vector svg circle').waitFor({ state: 'detached', timeout: 15000 });

// Sandbox reports success; empty selection clears the preview.
await page.evaluate(() => send({ type: 'inserted', layers: 4, matched: 2 }));
await ui.locator('#message').filter({ hasText: 'Готово' }).waitFor();
await page.evaluate(() => send({ type: 'empty', reason: 'Выделите слой' }));
await ui.locator('#preview.empty').waitFor();
assert.equal(await ui.locator('#insert-replace').isDisabled(), true);

await browser.close();
assert.deepEqual(errors, [], `console errors/warnings: ${errors.join('\n')}`);
console.log('ui smoke test passed; screenshot:', resolve(out, 'ui-split.png'));
