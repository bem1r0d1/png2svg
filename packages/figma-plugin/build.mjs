// Builds dist/code.js and a single-file dist/ui.html (Figma requires one HTML file):
// the worker source and the WASM binary are inlined into the UI script.
//
//   node build.mjs           # build (expects the wasm already built, see `npm run build`)
//   node build.mjs --watch   # rebuild on change

import { readFileSync, writeFileSync, mkdirSync, watch } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const wasmPath = resolve(root, 'target/wasm32-unknown-unknown/release/png2svg_wasm.wasm');
const dist = resolve(here, 'dist');

async function bundle(entry, opts = {}) {
  const r = await esbuild.build({
    entryPoints: [resolve(here, entry)],
    bundle: true,
    write: false,
    format: 'iife',
    target: 'es2020',
    minify: true,
    legalComments: 'none',
    ...opts,
  });
  return r.outputFiles[0].text;
}

async function build() {
  mkdirSync(dist, { recursive: true });
  const wasm = readFileSync(wasmPath);
  const worker = await bundle('src/worker.ts');
  const ui = await bundle('src/ui/ui.ts', {
    define: {
      __WORKER_SRC__: JSON.stringify(worker),
      __WASM_B64__: JSON.stringify(wasm.toString('base64')),
    },
  });
  const css = readFileSync(resolve(here, 'src/ui/ui.css'), 'utf8');
  const html = readFileSync(resolve(here, 'src/ui/ui.html'), 'utf8')
    .replace('/*__CSS__*/', () => css)
    // Guard against a literal "</script>" inside the bundle.
    .replace('/*__JS__*/', () => ui.replace(/<\/script/gi, '<\\/script'));
  writeFileSync(resolve(dist, 'ui.html'), html);
  // The same page works standalone as the web version.
  mkdirSync(resolve(dist, 'web'), { recursive: true });
  writeFileSync(resolve(dist, 'web/index.html'), html);

  const code = await bundle('src/code.ts', { target: 'es2017' });
  writeFileSync(resolve(dist, 'code.js'), code);
  console.log(
    `built dist/code.js (${(code.length / 1024).toFixed(1)} KB), dist/ui.html (${(html.length / 1024).toFixed(1)} KB, wasm ${(wasm.length / 1024).toFixed(0)} KB)`,
  );
}

await build();
if (process.argv.includes('--watch')) {
  let timer;
  for (const dir of ['src', '../../target/wasm32-unknown-unknown/release']) {
    watch(resolve(here, dir), { recursive: true }, () => {
      clearTimeout(timer);
      timer = setTimeout(() => build().catch((e) => console.error(e.message)), 100);
    });
  }
  console.log('watching…');
}
