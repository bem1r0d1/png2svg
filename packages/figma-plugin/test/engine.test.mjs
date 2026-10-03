// Runs the real WASM engine in Node on a synthetic anti-aliased image.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createEngine } from '../src/engine.ts';

const here = dirname(fileURLToPath(import.meta.url));
const wasm = readFileSync(resolve(here, '../../../target/wasm32-unknown-unknown/release/png2svg_wasm.wasm'));

function disc(size, r, fg) {
  const px = new Uint8Array(size * size * 4);
  const c = size / 2;
  for (let y = 0; y < size; y++)
    for (let x = 0; x < size; x++) {
      let cov = 0;
      for (let s = 0; s < 16; s++) {
        const sx = x + ((s % 4) + 0.5) / 4, sy = y + (Math.floor(s / 4) + 0.5) / 4;
        if ((sx - c) ** 2 + (sy - c) ** 2 <= r * r) cov++;
      }
      const i = (y * size + x) * 4;
      px.set([fg[0], fg[1], fg[2], Math.round((cov / 16) * 255)], i);
    }
  return px;
}

test('converts an anti-aliased disc to one smooth layer', async () => {
  const engine = await createEngine(await WebAssembly.compile(wasm));
  const out = engine.convert(disc(64, 24, [13, 153, 255]), 64, 64, { preset: 'auto', smoothness: 0.5 });
  assert.equal(out.width, 64);
  assert.equal(out.layers.length, 1);
  assert.equal(out.layers[0].hex, '#0d99ff');
  assert.ok(out.layers[0].segments <= 6, `segments ${out.layers[0].segments}`);
  assert.match(out.svg, /^<svg[^>]+viewBox="0 0 64 64"/);
});

test('reports invalid input as an error', async () => {
  const engine = await createEngine(await WebAssembly.compile(wasm));
  assert.throws(() => engine.convert(new Uint8Array(10), 2, 2, {}), /buffer/);
});

test('repeated conversions do not leak or corrupt memory', async () => {
  const engine = await createEngine(await WebAssembly.compile(wasm));
  const img = disc(128, 50, [200, 30, 60]);
  const first = engine.convert(img, 128, 128, {}).svg;
  for (let i = 0; i < 30; i++) assert.equal(engine.convert(img, 128, 128, {}).svg, first);
});
