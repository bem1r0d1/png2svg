// Thin JS wrapper over the png2svg WebAssembly C ABI (see crates/wasm).

import type { ConvertOptions, ConvertOutput } from './shared';

interface Exports {
  memory: WebAssembly.Memory;
  p2s_alloc(len: number): number;
  p2s_free(ptr: number, len: number): void;
  p2s_convert(rgba: number, rgbaLen: number, w: number, h: number, opts: number, optsLen: number): number;
  p2s_result_ptr(): number;
  p2s_result_len(): number;
}

export interface Engine {
  convert(rgba: Uint8Array, width: number, height: number, options: Partial<ConvertOptions>): ConvertOutput;
}

export async function createEngine(module: WebAssembly.Module): Promise<Engine> {
  const instance = await WebAssembly.instantiate(module, {});
  const ex = instance.exports as unknown as Exports;

  const put = (data: Uint8Array): number => {
    const ptr = ex.p2s_alloc(data.length);
    // Create the view after allocating: allocation may grow (detach) memory.
    new Uint8Array(ex.memory.buffer, ptr, data.length).set(data);
    return ptr;
  };

  return {
    convert(rgba, width, height, options) {
      const opts = new TextEncoder().encode(JSON.stringify(toCoreOptions(options)));
      const pr = put(rgba);
      const po = put(opts);
      let ok: number;
      try {
        ok = ex.p2s_convert(pr, rgba.length, width, height, po, opts.length);
      } finally {
        ex.p2s_free(pr, rgba.length);
        ex.p2s_free(po, opts.length);
      }
      const json = new TextDecoder().decode(
        new Uint8Array(ex.memory.buffer, ex.p2s_result_ptr(), ex.p2s_result_len()),
      );
      const result = JSON.parse(json);
      if (!ok) throw new Error(result.error ?? 'conversion failed');
      return result as ConvertOutput;
    },
  };
}

/** Maps UI options to the core's serde representation (omits unset values). */
function toCoreOptions(o: Partial<ConvertOptions>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (o.preset) out.preset = o.preset;
  if (o.colors != null && o.colors > 0) out.colors = Math.round(o.colors);
  if (o.detail != null) out.detail = o.detail;
  if (o.smoothness != null) out.smoothness = o.smoothness;
  if (o.cornerThreshold != null) out.cornerThreshold = o.cornerThreshold;
  if (o.snapAxes != null) out.snapAxes = o.snapAxes;
  return out;
}
