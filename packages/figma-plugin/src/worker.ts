// Web Worker: runs conversions off the UI thread.

import { createEngine, type Engine } from './engine';

let engine: Engine | null = null;

type In =
  | { type: 'init'; module: WebAssembly.Module }
  | { type: 'convert'; id: number; rgba: Uint8Array; width: number; height: number; options: object };

self.onmessage = async (e: MessageEvent<In>) => {
  const m = e.data;
  if (m.type === 'init') {
    try {
      engine = await createEngine(m.module);
      self.postMessage({ type: 'ready' });
    } catch (err) {
      self.postMessage({ type: 'fatal', message: String(err) });
    }
    return;
  }
  if (m.type === 'convert') {
    const t0 = performance.now();
    try {
      if (!engine) throw new Error('engine not initialised');
      const out = engine.convert(m.rgba, m.width, m.height, m.options);
      self.postMessage({ type: 'result', id: m.id, out, ms: performance.now() - t0 });
    } catch (err) {
      self.postMessage({ type: 'error', id: m.id, message: err instanceof Error ? err.message : String(err) });
    }
  }
};
