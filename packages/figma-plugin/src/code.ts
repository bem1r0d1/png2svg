// Plugin sandbox (main thread): talks to the Figma document.
// Reads the selected image, and inserts the vector result exactly in its place.

import { deltaE, type ToCode, type ToUi } from './shared';

/** Longest side (px) sent for conversion; keeps the UI responsive. */
const MAX_SIDE = 2048;
/** Colours closer than this (ΔE OKLab × 100) are bound to a style/variable. */
const MATCH_DELTA_E = 2.0;

figma.showUI(__html__, { width: 420, height: 680, themeColors: true });

const post = (msg: ToUi) => figma.ui.postMessage(msg);

type ImageNode = SceneNode & GeometryMixin & LayoutMixin;

function imageFillOf(node: SceneNode): ImagePaint | null {
  if (!('fills' in node) || !Array.isArray(node.fills)) return null;
  const fills = node.fills as readonly Paint[];
  for (let i = fills.length - 1; i >= 0; i--) {
    const f = fills[i];
    if (f.type === 'IMAGE' && f.visible !== false && f.imageHash) return f;
  }
  return null;
}

let selectionToken = 0;

async function sendSelection() {
  const token = ++selectionToken;
  const node = figma.currentPage.selection.find((n) => imageFillOf(n) !== null) as ImageNode | undefined;
  if (!node) {
    post({
      type: 'empty',
      reason: figma.currentPage.selection.length
        ? 'В выделении нет слоя с изображением'
        : 'Выделите слой с изображением или перетащите PNG сюда',
    });
    return;
  }
  try {
    const fill = imageFillOf(node)!;
    const image = figma.getImageByHash(fill.imageHash!);
    if (!image) throw new Error('Изображение не найдено');
    const size = await image.getSizeAsync();
    // Export what the user sees (crop / fit applied) at the image's native density.
    let scale = Math.max(size.width / node.width, size.height / node.height, 1);
    scale = Math.min(scale, MAX_SIDE / Math.max(node.width, node.height));
    const bytes = await node.exportAsync({ format: 'PNG', constraint: { type: 'SCALE', value: scale } });
    if (token !== selectionToken) return; // selection changed meanwhile
    post({ type: 'image', id: node.id, name: node.name, bytes, nodeWidth: node.width, nodeHeight: node.height });
  } catch (e) {
    post({ type: 'error', message: e instanceof Error ? e.message : String(e) });
  }
}

figma.on('selectionchange', () => void sendSelection());

figma.ui.onmessage = async (msg: ToCode) => {
  try {
    if (msg.type === 'ready') await sendSelection();
    else if (msg.type === 'resize') figma.ui.resize(msg.width, msg.height);
    else if (msg.type === 'insert') await insert(msg);
  } catch (e) {
    post({ type: 'error', message: e instanceof Error ? e.message : String(e) });
  }
};

async function insert(msg: Extract<ToCode, { type: 'insert' }>) {
  const frame = figma.createNodeFromSvg(msg.svg);
  frame.name = `${msg.name} · vector`;
  frame.fills = [];
  frame.clipsContent = false;

  // One vector per <path>, bottom → top; name them after the SVG ids.
  const vectors = frame.children.filter((c) => c.type === 'VECTOR') as VectorNode[];
  if (vectors.length === msg.layers.length) {
    vectors.forEach((v, i) => (v.name = msg.layers[i].id));
  }
  const matched = msg.matchStyles ? await bindColors(vectors) : 0;

  const source = msg.sourceId ? ((await figma.getNodeByIdAsync(msg.sourceId)) as SceneNode | null) : null;
  if (source && source.parent && 'width' in source) {
    const parent = source.parent as BaseNode & ChildrenMixin;
    const index = parent.children.indexOf(source);
    // The SVG is in exported pixels; scale it to the node's size.
    frame.rescale(source.width / frame.width);
    if (msg.mode === 'replace') {
      parent.insertChild(index + 1, frame);
      frame.relativeTransform = source.relativeTransform;
      source.visible = false; // keep the original (hidden) for comparison / undo
    } else {
      parent.insertChild(index + 1, frame);
      // In auto layout the frame simply takes the next slot.
      const inAutoLayout = 'layoutMode' in parent && (parent as FrameNode).layoutMode !== 'NONE';
      if (!inAutoLayout) {
        frame.relativeTransform = source.relativeTransform;
        frame.x = source.x + source.width + 24;
      }
    }
  } else {
    // Dropped file: place at the centre of the viewport.
    figma.currentPage.appendChild(frame);
    const c = figma.viewport.center;
    frame.x = Math.round(c.x - frame.width / 2);
    frame.y = Math.round(c.y - frame.height / 2);
  }
  figma.currentPage.selection = [frame];
  post({ type: 'inserted', layers: vectors.length, matched });
  figma.notify(
    `Вектор вставлен: ${vectors.length} слоёв` + (matched ? `, привязано к стилям: ${matched}` : ''),
  );
}

interface NamedColor {
  rgb: [number, number, number];
  apply(v: VectorNode, paint: SolidPaint): Promise<void>;
}

/** Binds solid fills to local colour variables or paint styles with (almost) the same colour. */
async function bindColors(vectors: VectorNode[]): Promise<number> {
  const candidates: NamedColor[] = [];

  for (const variable of await figma.variables.getLocalVariablesAsync('COLOR')) {
    const coll = await figma.variables.getVariableCollectionByIdAsync(variable.variableCollectionId);
    const value = coll ? variable.valuesByMode[coll.defaultModeId] : undefined;
    if (!value || typeof value !== 'object' || !('r' in value)) continue;
    candidates.push({
      rgb: [value.r, value.g, value.b],
      apply: async (v, paint) => {
        v.fills = [figma.variables.setBoundVariableForPaint(paint, 'color', variable)];
      },
    });
  }
  for (const style of await figma.getLocalPaintStylesAsync()) {
    const p = style.paints.length === 1 ? style.paints[0] : null;
    if (!p || p.type !== 'SOLID' || (p.opacity ?? 1) < 1) continue;
    candidates.push({
      rgb: [p.color.r, p.color.g, p.color.b],
      apply: (v) => v.setFillStyleIdAsync(style.id),
    });
  }
  if (!candidates.length) return 0;

  let matched = 0;
  for (const v of vectors) {
    const fills = v.fills;
    if (!Array.isArray(fills) || fills.length !== 1 || fills[0].type !== 'SOLID') continue;
    const paint = fills[0] as SolidPaint;
    const rgb: [number, number, number] = [paint.color.r, paint.color.g, paint.color.b];
    let best: NamedColor | null = null;
    let bestD = MATCH_DELTA_E;
    for (const c of candidates) {
      const d = deltaE(rgb, c.rgb);
      if (d < bestD) {
        bestD = d;
        best = c;
      }
    }
    if (best) {
      await best.apply(v, paint);
      matched++;
    }
  }
  return matched;
}

