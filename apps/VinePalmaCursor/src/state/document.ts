export interface Layer {
  id: string;
  name: string;
  visible: boolean;
  opacity: number;
  pixels: Uint8Array;
}

export interface Frame {
  id: string;
  layers: Layer[];
  hotspotX: number;
  hotspotY: number;
  delayMs: number;
}

export interface DocumentModel {
  width: number;
  height: number;
  frames: Frame[];
  activeFrame: number;
  activeLayer: number;
  currentColor: string;
  currentTool: ToolType;
}

export type ToolType =
  | 'brush'
  | 'eraser'
  | 'fill'
  | 'line'
  | 'eyedropper'
  | 'hotspot'
  | 'move'
  | 'rect'
  | 'ellipse';

export function createEmptyLayer(id: string, name: string, pixelCount: number): Layer {
  return {
    id,
    name,
    visible: true,
    opacity: 1,
    pixels: new Uint8Array(pixelCount * 4),
  };
}

export function createEmptyFrame(id: string, width: number, height: number): Frame {
  const layer = createEmptyLayer(id + '-layer0', 'Layer 1', width * height);
  return {
    id,
    layers: [layer],
    hotspotX: Math.floor(width / 2),
    hotspotY: Math.floor(height / 2),
    delayMs: 100,
  };
}

export function createDocument(width: number, height: number): DocumentModel {
  const frame = createEmptyFrame('frame-0', width, height);
  return {
    width,
    height,
    frames: [frame],
    activeFrame: 0,
    activeLayer: 0,
    currentColor: '#000000',
    currentTool: 'brush',
  };
}

export function snapshotLayer(layer: Layer): Uint8Array {
  return new Uint8Array(layer.pixels);
}

export function cloneLayer(layer: Layer): Layer {
  return {
    ...layer,
    pixels: new Uint8Array(layer.pixels),
  };
}

export function cloneFrame(frame: Frame): Frame {
  return {
    ...frame,
    layers: frame.layers.map(cloneLayer),
  };
}

export function cloneDocument(doc: DocumentModel): DocumentModel {
  return {
    ...doc,
    frames: doc.frames.map(cloneFrame),
  };
}

export function createId(prefix: string): string {
  return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

export function mergeFramePixels(frame: Frame, width: number, height: number): Uint8Array {
  const out = new Uint8Array(width * height * 4);

  for (const layer of frame.layers) {
    if (!layer.visible || layer.opacity <= 0) continue;

    for (let i = 0; i < width * height; i++) {
      const si = i * 4;

      const sa = (layer.pixels[si + 3] / 255) * layer.opacity;
      if (sa === 0) continue;

      const da = out[si + 3] / 255;
      const oa = sa + da * (1 - sa);

      const sr = layer.pixels[si];
      const sg = layer.pixels[si + 1];
      const sb = layer.pixels[si + 2];
      const dr = out[si];
      const dg = out[si + 1];
      const db = out[si + 2];

      out[si] = Math.round((sr * sa + dr * da * (1 - sa)) / oa);
      out[si + 1] = Math.round((sg * sa + dg * da * (1 - sa)) / oa);
      out[si + 2] = Math.round((sb * sa + db * da * (1 - sa)) / oa);
      out[si + 3] = Math.round(oa * 255);
    }
  }

  return out;
}

export function fitPixelsToSize(
  source: Uint8Array,
  srcW: number,
  srcH: number,
  dstW: number,
  dstH: number,
): Uint8Array {
  const out = new Uint8Array(dstW * dstH * 4);
  const offsetX = Math.floor((dstW - srcW) / 2);
  const offsetY = Math.floor((dstH - srcH) / 2);

  for (let y = 0; y < srcH; y++) {
    const dy = y + offsetY;
    if (dy < 0 || dy >= dstH) continue;
    for (let x = 0; x < srcW; x++) {
      const dx = x + offsetX;
      if (dx < 0 || dx >= dstW) continue;
      const si = (y * srcW + x) * 4;
      const di = (dy * dstW + dx) * 4;
      out[di] = source[si];
      out[di + 1] = source[si + 1];
      out[di + 2] = source[si + 2];
      out[di + 3] = source[si + 3];
    }
  }

  return out;
}

export function translateLayerPixels(
  pixels: Uint8Array,
  width: number,
  height: number,
  dx: number,
  dy: number,
): Uint8Array {
  if (dx === 0 && dy === 0) return pixels;

  const out = new Uint8Array(width * height * 4);
  for (let iy = 0; iy < height; iy++) {
    for (let ix = 0; ix < width; ix++) {
      const sx = ix - dx;
      const sy = iy - dy;
      if (sx < 0 || sx >= width || sy < 0 || sy >= height) continue;

      const si = (sy * width + sx) * 4;
      const di = (iy * width + ix) * 4;
      out[di] = pixels[si];
      out[di + 1] = pixels[si + 1];
      out[di + 2] = pixels[si + 2];
      out[di + 3] = pixels[si + 3];
    }
  }

  return out;
}

export function flipLayerPixels(
  pixels: Uint8Array,
  width: number,
  height: number,
  horizontal: boolean,
): Uint8Array {
  const out = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const si = (y * width + x) * 4;
      const nx = horizontal ? width - 1 - x : x;
      const ny = horizontal ? y : height - 1 - y;
      const di = (ny * width + nx) * 4;
      out[di] = pixels[si];
      out[di + 1] = pixels[si + 1];
      out[di + 2] = pixels[si + 2];
      out[di + 3] = pixels[si + 3];
    }
  }
  return out;
}

export function scalePixelsToSquare(
  pixels: Uint8Array,
  srcW: number,
  srcH: number,
  dstSize: number,
): Uint8Array {
  const out = new Uint8Array(dstSize * dstSize * 4);
  const scale = Math.min(dstSize / srcW, dstSize / srcH);
  const dw = Math.max(1, Math.round(srcW * scale));
  const dh = Math.max(1, Math.round(srcH * scale));
  const offX = Math.floor((dstSize - dw) / 2);
  const offY = Math.floor((dstSize - dh) / 2);

  for (let sy = 0; sy < srcH; sy++) {
    const dy = Math.min(dh - 1, Math.floor(sy * scale)) + offY;
    if (dy < 0 || dy >= dstSize) continue;
    for (let sx = 0; sx < srcW; sx++) {
      const dx = Math.min(dw - 1, Math.floor(sx * scale)) + offX;
      if (dx < 0 || dx >= dstSize) continue;
      const si = (sy * srcW + sx) * 4;
      const di = (dy * dstSize + dx) * 4;
      out[di] = pixels[si];
      out[di + 1] = pixels[si + 1];
      out[di + 2] = pixels[si + 2];
      out[di + 3] = pixels[si + 3];
    }
  }

  return out;
}

export function rgbaToBase64(pixels: Uint8Array): string {
  let bin = '';
  for (let i = 0; i < pixels.length; i += 4096) {
    bin += String.fromCharCode(...pixels.subarray(i, i + 4096));
  }
  return btoa(bin);
}

export function base64ToRgba(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) {
    out[i] = bin.charCodeAt(i);
  }
  return out;
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

export function serializeDocument(doc: DocumentModel): string {
  const payload = {
    version: 1,
    width: doc.width,
    height: doc.height,
    currentColor: doc.currentColor,
    currentTool: doc.currentTool,
    activeFrame: doc.activeFrame,
    activeLayer: doc.activeLayer,
    frames: doc.frames.map((f) => ({
      hotspotX: f.hotspotX,
      hotspotY: f.hotspotY,
      delayMs: f.delayMs,
      layers: f.layers.map((l) => ({
        name: l.name,
        visible: l.visible,
        opacity: l.opacity,
        pixels: rgbaToBase64(l.pixels),
      })),
    })),
  };
  return JSON.stringify(payload);
}

export function deserializeDocument(json: string): DocumentModel | null {
  try {
    const p = JSON.parse(json);
    if (
      !p ||
      p.version !== 1 ||
      !Number.isInteger(p.width) ||
      !Number.isInteger(p.height) ||
      !Array.isArray(p.frames) ||
      p.frames.length === 0
    ) {
      return null;
    }

    const width = clamp(Math.round(p.width), 1, 128);
    const height = clamp(Math.round(p.height), 1, 128);
    const expectedPixels = width * height * 4;

    const frames: Frame[] = p.frames.slice(0, 256).map((f: Record<string, unknown>) => {
      const rawLayers = Array.isArray(f.layers) ? f.layers : [];
      const layers = rawLayers.slice(0, 64).map((raw: Record<string, unknown>) => {
        let pixels: Uint8Array = new Uint8Array(expectedPixels);
        if (typeof raw.pixels === 'string') {
          const decoded = base64ToRgba(raw.pixels);
          if (decoded.length === expectedPixels) {
            pixels = decoded;
          } else if (decoded.length > expectedPixels) {
            pixels = decoded.slice(0, expectedPixels);
          }
        }
        return {
          id: createId('layer'),
          name: String(raw.name ?? '图层'),
          visible: raw.visible !== false,
          opacity: typeof raw.opacity === 'number' ? clamp(raw.opacity, 0, 1) : 1,
          pixels,
        };
      });

      return {
        id: createId('frame'),
        layers,
        hotspotX: clamp(Math.round(Number(f.hotspotX ?? 0)), 0, width - 1),
        hotspotY: clamp(Math.round(Number(f.hotspotY ?? 0)), 0, height - 1),
        delayMs: clamp(Math.round(Number(f.delayMs ?? 100)), 0, 60000),
      };
    });

    const activeFrame = clamp(Math.round(Number(p.activeFrame ?? 0)), 0, frames.length - 1);
    const activeLayer = clamp(
      Math.round(Number(p.activeLayer ?? 0)),
      0,
      frames[activeFrame].layers.length - 1,
    );

    const tool = p.currentTool as ToolType;
    const validTools: ToolType[] = [
      'brush', 'eraser', 'fill', 'line', 'eyedropper', 'hotspot', 'move', 'rect', 'ellipse',
    ];

    return {
      width,
      height,
      frames,
      activeFrame,
      activeLayer,
      currentColor: typeof p.currentColor === 'string' ? p.currentColor : '#000000',
      currentTool: validTools.indexOf(tool) >= 0 ? tool : 'brush',
    };
  } catch (err) {
    console.error('deserializeDocument:', err);
    return null;
  }
}