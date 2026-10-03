// Plugin UI (iframe): decoding, live preview, controls. Conversion runs in a worker.

import { createEngine, type Engine } from '../engine';
import {
  DEFAULT_OPTIONS,
  type ConvertOptions,
  type ConvertOutput,
  type InsertMode,
  type Preset,
  type ToCode,
  type ToUi,
} from '../shared';

// Injected by build.mjs.
declare const __WORKER_SRC__: string;
declare const __WASM_B64__: string;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const els = {
  preview: $('preview'),
  stage: $('stage'),
  original: $<HTMLImageElement>('img-original'),
  vector: $('img-vector'),
  handle: $('split-handle'),
  placeholderText: $('placeholder-text'),
  file: $<HTMLInputElement>('file'),
  busy: $('busy'),
  stats: $('stats'),
  palette: $('palette'),
  sourceName: $('source-name'),
  sourceSize: $('source-size'),
  message: $('message'),
  colorsAuto: $<HTMLInputElement>('colors-auto'),
  colors: $<HTMLInputElement>('colors'),
  detail: $<HTMLInputElement>('detail'),
  smoothness: $<HTMLInputElement>('smoothness'),
  corner: $<HTMLInputElement>('corner'),
  snap: $<HTMLInputElement>('snap'),
  shapes: $<HTMLInputElement>('shapes'),
  separate: $<HTMLInputElement>('separate'),
  speckle: $<HTMLInputElement>('speckle'),
  smoothing: $<HTMLInputElement>('smoothing'),
  smoothingValue: $('smoothing-value'),
  denoise: $<HTMLInputElement>('denoise'),
  depixelate: $<HTMLInputElement>('depixelate'),
  detected: $('detected'),
  speckleValue: $('speckle-value'),
  match: $<HTMLInputElement>('match'),
  beside: $<HTMLButtonElement>('insert-beside'),
  replace: $<HTMLButtonElement>('insert-replace'),
};

/** Outside Figma (the web version) the page is not framed by the plugin host. */
const IN_FIGMA = window.parent !== window;
const toCode = (msg: ToCode) => IN_FIGMA && parent.postMessage({ pluginMessage: msg }, '*');

// ---------------------------------------------------------------- state

interface Source {
  id: string | null;
  name: string;
  rgba: Uint8Array;
  width: number;
  height: number;
  url: string;
}

let source: Source | null = null;
let result: ConvertOutput | null = null;
const options: ConvertOptions = { ...DEFAULT_OPTIONS };
let view: 'original' | 'split' | 'vector' = 'split';
let docColors: string[] = [];
let split = 0.5;

// ---------------------------------------------------------------- engine (worker with in-thread fallback)

function wasmBytes(): Uint8Array {
  const bin = atob(__WASM_B64__);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

type Job = { id: number; rgba: Uint8Array; width: number; height: number; options: ConvertOptions };
type Done = (r: { id: number; out?: ConvertOutput; ms: number; error?: string }) => void;

class Runner {
  private worker: Worker | null = null;
  private engine: Engine | null = null;
  private busy = false;
  private pending: Job | null = null;
  private ready: Promise<void>;

  constructor(private done: Done) {
    this.ready = this.init();
  }

  private async init() {
    const module = await WebAssembly.compile(wasmBytes() as BufferSource);
    try {
      const url = URL.createObjectURL(new Blob([__WORKER_SRC__], { type: 'text/javascript' }));
      const w = new Worker(url);
      await new Promise<void>((resolve, reject) => {
        w.onmessage = (e) => (e.data.type === 'ready' ? resolve() : reject(new Error(e.data.message)));
        w.onerror = (e) => reject(new Error(e.message));
        w.postMessage({ type: 'init', module });
      });
      w.onmessage = (e) => this.finish(e.data);
      this.worker = w;
    } catch (err) {
      console.warn('png2svg: worker unavailable, converting on the UI thread', err);
      this.engine = await createEngine(module);
    }
  }

  /** Queues a job; only the latest pending job is kept. */
  async run(job: Job) {
    await this.ready;
    if (this.busy) {
      this.pending = job;
      return;
    }
    this.start(job);
  }

  private start(job: Job) {
    this.busy = true;
    if (this.worker) {
      this.worker.postMessage({ type: 'convert', ...job });
      return;
    }
    // Let the spinner paint before blocking.
    setTimeout(() => {
      const t0 = performance.now();
      try {
        const out = this.engine!.convert(job.rgba, job.width, job.height, job.options);
        this.finish({ type: 'result', id: job.id, out, ms: performance.now() - t0 });
      } catch (err) {
        this.finish({ type: 'error', id: job.id, message: String(err), ms: 0 });
      }
    }, 16);
  }

  private finish(m: { type: string; id: number; out?: ConvertOutput; ms: number; message?: string }) {
    this.busy = false;
    this.done({ id: m.id, out: m.out, ms: m.ms, error: m.type === 'error' ? m.message : undefined });
    const next = this.pending;
    this.pending = null;
    if (next) this.start(next);
  }
}

let jobId = 0;
const runner = new Runner(({ id, out, ms, error }) => {
  if (id !== jobId) return; // stale
  els.busy.hidden = true;
  if (error || !out) {
    showMessage(error ?? 'Ошибка конвертации', true);
    return;
  }
  result = out;
  renderResult(ms);
});

let debounce: number | undefined;
function schedule(delay = 150) {
  if (!source) return;
  clearTimeout(debounce);
  debounce = window.setTimeout(() => {
    if (!source) return;
    els.busy.hidden = false;
    void runner.run({ id: ++jobId, rgba: source.rgba, width: source.width, height: source.height, options: { ...options } });
  }, delay);
}

// ---------------------------------------------------------------- decoding

async function decode(bytes: Uint8Array, type = 'image/png') {
  const blob = new Blob([bytes as BlobPart], { type });
  let bmp: ImageBitmap;
  try {
    bmp = await createImageBitmap(blob, { premultiplyAlpha: 'none', colorSpaceConversion: 'none' });
  } catch {
    bmp = await createImageBitmap(blob);
  }
  const canvas = document.createElement('canvas');
  canvas.width = bmp.width;
  canvas.height = bmp.height;
  const ctx = canvas.getContext('2d', { willReadFrequently: true })!;
  ctx.drawImage(bmp, 0, 0);
  const data = ctx.getImageData(0, 0, bmp.width, bmp.height).data;
  return { rgba: new Uint8Array(data.buffer), width: bmp.width, height: bmp.height, url: URL.createObjectURL(blob) };
}

async function setSource(id: string | null, name: string, bytes: Uint8Array, type?: string) {
  try {
    const d = await decode(bytes, type);
    if (source) URL.revokeObjectURL(source.url);
    source = { id, name, ...d };
    result = null;
    els.original.src = d.url;
    els.vector.innerHTML = '';
    els.sourceName.textContent = name;
    els.sourceSize.textContent = `${d.width}×${d.height} px`;
    els.preview.classList.remove('empty');
    updateView();
    showMessage('');
    schedule(0);
  } catch (e) {
    showMessage('Не удалось прочитать изображение', true);
  }
}

function clearSource(reason: string) {
  source = null;
  result = null;
  els.preview.classList.add('empty');
  els.placeholderText.textContent = reason;
  els.sourceName.textContent = 'Нет изображения';
  els.sourceSize.textContent = '';
  els.stats.textContent = '';
  els.detected.textContent = '';
  els.palette.innerHTML = '';
  setActionsEnabled(false);
}

// ---------------------------------------------------------------- rendering

function renderResult(ms: number) {
  if (!result) return;
  els.vector.innerHTML = result.svg;
  const svg = els.vector.querySelector('svg');
  if (svg) {
    svg.setAttribute('preserveAspectRatio', 'xMidYMid meet');
    svg.removeAttribute('width');
    svg.removeAttribute('height');
  }
  const s = result.stats;
  const kb = (s.bytes / 1024).toFixed(1);
  const prims = s.primitives ? ` · ${s.primitives} фигур` : '';
  els.stats.textContent = `${s.layers} цв. · ${s.subpaths} контуров · ${s.segments} сегм.${prims} · ${kb} КБ · ${Math.round(ms)} мс · ${presetName(s.preset)}`;
  const found: string[] = [];
  if (s.pixelGrid > 1) found.push(`пикселизация ×${s.pixelGrid}`);
  if (s.noise > 0.01) found.push('шум / JPEG');
  if (s.smoothing > 0) found.push(`сглаживание ${s.smoothing.toFixed(1)} px`);
  els.detected.textContent = found.length ? `Исправлено: ${found.join(' · ')}` : '';
  els.palette.innerHTML = '';
  for (const l of result.layers) {
    const chip = document.createElement('span');
    chip.className = 'chip';
    chip.style.background = l.hex;
    chip.title = `${l.hex} · ${l.subpaths} контуров`;
    els.palette.appendChild(chip);
  }
  setActionsEnabled(true);
  updateView();
}

function presetName(p: string) {
  return ({ logo: 'логотип', icon: 'иконка', illustration: 'иллюстрация' } as Record<string, string>)[p] ?? p;
}

/** Rect of the image inside the stage (object-fit: contain). */
function contentRect() {
  const box = els.stage.getBoundingClientRect();
  if (!source) return { x: 0, y: 0, w: box.width, h: box.height };
  const s = Math.min(box.width / source.width, box.height / source.height);
  const w = source.width * s;
  const h = source.height * s;
  return { x: (box.width - w) / 2, y: (box.height - h) / 2, w, h };
}

function updateView() {
  els.original.style.visibility = view === 'vector' ? 'hidden' : 'visible';
  els.vector.style.visibility = view === 'original' ? 'hidden' : 'visible';
  els.handle.hidden = view !== 'split' || !result;
  if (view === 'split' && result) {
    const r = contentRect();
    const x = r.x + r.w * split;
    els.vector.style.clipPath = `inset(0 0 0 ${x}px)`;
    els.handle.style.left = `${x}px`;
  } else {
    els.vector.style.clipPath = '';
  }
}

function showMessage(text: string, error = false) {
  els.message.textContent = text;
  els.message.classList.toggle('error', error);
}

function setActionsEnabled(on: boolean) {
  els.beside.disabled = !on;
  els.replace.disabled = !on || (IN_FIGMA && !source?.id);
}

// ---------------------------------------------------------------- events

document.querySelectorAll<HTMLButtonElement>('#view-mode button').forEach((b) =>
  b.addEventListener('click', () => {
    view = b.dataset.view as typeof view;
    document.querySelectorAll('#view-mode button').forEach((x) => x.classList.toggle('on', x === b));
    updateView();
  }),
);

document.querySelectorAll<HTMLButtonElement>('#preset button').forEach((b) =>
  b.addEventListener('click', () => {
    options.preset = b.dataset.preset as Preset;
    document.querySelectorAll('#preset button').forEach((x) => x.classList.toggle('on', x === b));
    schedule(0);
  }),
);

const readControls = () => {
  options.colors = els.colorsAuto.checked ? null : Math.max(1, Math.min(64, Number(els.colors.value) || 1));
  els.colors.disabled = els.colorsAuto.checked;
  options.detail = Number(els.detail.value);
  options.smoothness = Number(els.smoothness.value);
  options.cornerThreshold = Number(els.corner.value);
  options.snapAxes = els.snap.checked;
  options.shapes = els.shapes.checked;
  options.groupBy = els.separate.checked ? 'shape' : 'color';
  const speckle = Number(els.speckle.value);
  options.speckleArea = speckle > 0 ? speckle : null;
  els.speckleValue.textContent = speckle > 0 ? `${speckle} px²` : 'авто';
  const sm = Number(els.smoothing.value);
  options.smoothing = sm < 0 ? null : sm;
  els.smoothingValue.textContent = sm < 0 ? 'авто' : `${sm.toFixed(1)} px`;
  options.denoise = els.denoise.checked;
  options.depixelate = els.depixelate.checked;
  // Snap extracted colours to the document's styles / variables.
  options.palette = els.match.checked ? docColors : [];
  schedule();
};
for (const el of [
  els.colorsAuto,
  els.colors,
  els.detail,
  els.smoothness,
  els.corner,
  els.snap,
  els.shapes,
  els.separate,
  els.speckle,
  els.match,
  els.smoothing,
  els.denoise,
  els.depixelate,
]) {
  el.addEventListener('input', readControls);
}

// Split handle dragging.
let dragging = false;
els.preview.addEventListener('pointerdown', (e) => {
  if (view !== 'split' || !result) return;
  dragging = true;
  els.preview.setPointerCapture(e.pointerId);
  moveSplit(e);
});
els.preview.addEventListener('pointermove', (e) => dragging && moveSplit(e));
els.preview.addEventListener('pointerup', () => (dragging = false));
function moveSplit(e: PointerEvent) {
  const box = els.stage.getBoundingClientRect();
  const r = contentRect();
  split = Math.max(0, Math.min(1, (e.clientX - box.left - r.x) / r.w));
  updateView();
}
new ResizeObserver(updateView).observe(els.stage);

function insert(mode: InsertMode) {
  if (!result || !source) return;
  toCode({
    type: 'insert',
    svg: result.svg,
    layers: result.layers,
    sourceId: source.id,
    name: source.name,
    mode,
    matchStyles: els.match.checked,
  });
}
function download() {
  if (!result || !source) return;
  const url = URL.createObjectURL(new Blob([result.svg], { type: 'image/svg+xml' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = `${source.name || 'image'}.svg`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  showMessage('SVG сохранён');
}

async function copySvg() {
  if (!result) return;
  try {
    await navigator.clipboard.writeText(result.svg);
    showMessage('SVG скопирован — вставьте в Figma через Ctrl/Cmd+V');
  } catch {
    showMessage('Не удалось скопировать', true);
  }
}

if (IN_FIGMA) {
  els.beside.addEventListener('click', () => insert('beside'));
  els.replace.addEventListener('click', () => insert('replace'));
} else {
  // Web version: download / copy instead of inserting into a document.
  document.body.classList.add('web');
  els.beside.textContent = 'Копировать SVG';
  els.replace.textContent = 'Скачать SVG';
  els.beside.addEventListener('click', () => void copySvg());
  els.replace.addEventListener('click', download);
  els.match.checked = false;
  els.match.parentElement!.hidden = true;
  options.palette = [];
  els.placeholderText.textContent = 'Перетащите PNG / JPG сюда, вставьте из буфера (Ctrl+V) или выберите файл';
}

// Paste an image from the clipboard.
document.addEventListener('paste', (e) => {
  const item = [...(e.clipboardData?.items ?? [])].find((i) => i.type.startsWith('image/'));
  const f = item?.getAsFile();
  if (f) void openFile(f);
});

// Files: picker and drag & drop.
async function openFile(f: File) {
  await setSource(null, f.name.replace(/\.[^.]+$/, ''), new Uint8Array(await f.arrayBuffer()), f.type || undefined);
}
els.file.addEventListener('change', () => els.file.files?.[0] && openFile(els.file.files[0]));
els.preview.addEventListener('dragover', (e) => {
  e.preventDefault();
  els.preview.classList.add('dragover');
});
els.preview.addEventListener('dragleave', () => els.preview.classList.remove('dragover'));
els.preview.addEventListener('drop', (e) => {
  e.preventDefault();
  els.preview.classList.remove('dragover');
  const f = e.dataTransfer?.files[0];
  if (f) void openFile(f);
});

// Messages from the plugin sandbox.
window.onmessage = (e: MessageEvent) => {
  const msg = e.data?.pluginMessage as ToUi | undefined;
  if (!msg || typeof msg !== 'object') return;
  switch (msg.type) {
    case 'image':
      void setSource(msg.id, msg.name, msg.bytes);
      break;
    case 'empty':
      // Keep a dropped file open when the selection is cleared.
      if (!source || source.id) clearSource(msg.reason);
      break;
    case 'docColors':
      docColors = msg.colors;
      if (els.match.checked) {
        options.palette = docColors;
        schedule(0);
      }
      break;
    case 'inserted':
      showMessage(`Готово: ${msg.layers} фигур` + (msg.matched ? `, привязано к стилям: ${msg.matched}` : ''));
      break;
    case 'error':
      showMessage(msg.message, true);
      break;
  }
};

toCode({ type: 'ready' });
