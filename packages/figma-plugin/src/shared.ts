// Types and helpers shared by the plugin sandbox (code.ts), the UI and the worker.

export type Preset = 'auto' | 'logo' | 'icon' | 'illustration';

export interface ConvertOptions {
  preset: Preset;
  colors: number | null;
  detail: number;
  smoothness: number;
  cornerThreshold: number;
  snapAxes: boolean;
  /** Detect circles, ellipses and (rounded) rectangles → native Figma shapes. */
  shapes: boolean;
  /** 'shape': one layer per shape grouped by colour; 'color': one layer per colour. */
  groupBy: 'shape' | 'color';
  /** Minimum region area in px (null = automatic). */
  speckleArea: number | null;
  /** Target colours (#rrggbb) that close colours snap to, e.g. document styles. */
  palette: string[];
  paletteTolerance: number;
  /** Edge smoothing σ in px (null = automatic). */
  smoothing: number | null;
  /** Remove noise / JPEG artefacts. */
  denoise: boolean;
  /** Rebuild smooth shapes from pixelated (nearest-upscaled) input. */
  depixelate: boolean;
}

export const DEFAULT_OPTIONS: ConvertOptions = {
  preset: 'auto',
  colors: null,
  detail: 0.5,
  smoothness: 0.5,
  cornerThreshold: 60,
  snapAxes: true,
  shapes: true,
  groupBy: 'shape',
  speckleArea: null,
  palette: [],
  paletteTolerance: 3,
  smoothing: null,
  denoise: true,
  depixelate: true,
};

export interface LayerInfo {
  id: string;
  color: [number, number, number, number];
  hex: string;
  area: number;
  subpaths: number;
  segments: number;
  elements: number;
}

export interface ConvertOutput {
  svg: string;
  width: number;
  height: number;
  layers: LayerInfo[];
  stats: {
    preset: string;
    colors: number;
    layers: number;
    subpaths: number;
    segments: number;
    primitives: number;
    bytes: number;
    noise: number;
    pixelGrid: number;
    smoothing: number;
  };
}

export type InsertMode = 'beside' | 'replace';

/** Messages from the plugin sandbox to the UI. */
export type ToUi =
  | { type: 'image'; id: string; name: string; bytes: Uint8Array; nodeWidth: number; nodeHeight: number }
  | { type: 'empty'; reason: string }
  | { type: 'docColors'; colors: string[] }
  | { type: 'inserted'; layers: number; matched: number }
  | { type: 'error'; message: string };

/** Messages from the UI to the plugin sandbox. */
export type ToCode =
  | { type: 'ready' }
  | {
      type: 'insert';
      svg: string;
      layers: LayerInfo[];
      sourceId: string | null;
      name: string;
      mode: InsertMode;
      matchStyles: boolean;
    }
  | { type: 'resize'; width: number; height: number };

// ---- colour helpers (OKLab), used to match colours to Figma styles/variables ----

function srgbToLinear(c: number): number {
  return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

/** OKLab from sRGB components in 0..1. */
export function oklab(r: number, g: number, b: number): [number, number, number] {
  const [lr, lg, lb] = [srgbToLinear(r), srgbToLinear(g), srgbToLinear(b)];
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

export function toHex(r: number, g: number, b: number): string {
  const h = (v: number) => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, '0');
  return `#${h(r)}${h(g)}${h(b)}`;
}

/** Perceptual distance (ΔE OKLab × 100) between two sRGB colours in 0..1. */
export function deltaE(a: [number, number, number], b: [number, number, number]): number {
  const x = oklab(...a);
  const y = oklab(...b);
  return Math.hypot(x[0] - y[0], x[1] - y[1], x[2] - y[2]) * 100;
}
